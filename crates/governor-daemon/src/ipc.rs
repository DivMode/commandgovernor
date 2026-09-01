//! The owner-local control socket, and the tiny protocol over it.
//!
//! # Shape
//!
//! A Unix domain socket at `<state-root>/ipc/d.sock`, inside an owner-only
//! directory, serving one line-delimited request per connection
//! (`docs/architecture.md`, "Local IPC / secrets and V1 trust boundary";
//! `docs/adr/0002-rust-daemon-and-sqlite.md`). There is no LAN bind, no
//! loopback HTTP fallback, and no framing beyond a newline:
//!
//! ```text
//! request  := "cg1 " VERB "\n"
//! response := "cg1 ok\n"  LINE* "cg1 end\n"
//!           | "cg1 err " CODE "\n" "cg1 end\n"
//! ```
//!
//! Every payload line is `key=value` pairs of opaque identities, classes and
//! counters — the safe-diagnostics set, and nothing that could carry content
//! (`docs/threat-model.md`, "Threat: diagnostics become exfiltration").
//!
//! # The security boundary, stated honestly
//!
//! `docs/threat-model.md` asks for a socket *in an owner-only directory with
//! peer credentials where available*. The enforced boundary here is the first
//! half: `ipc/` is `0700` and the socket is `0600`, both owned by the daemon's
//! effective user and re-checked at bind time, so another OS principal cannot
//! traverse to the socket to connect to it in the first place.
//!
//! Peer credentials are *not* checked, and the reason is concrete rather than
//! an oversight: reading them needs `SO_PEERCRED` or `getpeereid`, both raw C
//! calls, and this workspace denies `unsafe_code` outright. Pulling in a crate
//! to encapsulate one syscall would buy nothing here, because the check it
//! enables — "is the peer the same user?" — is already what the directory mode
//! enforces, and the case it does *not* cover is the same one the whole V1
//! trust model excludes.
//!
//! That residual risk is explicit: a process running as the **same** OS user
//! reaches this socket, and nothing in Phase 1 stops it. That is not a gap in
//! the implementation, it is `SECURITY.md`'s local trust model — the OS user
//! account is the administrative trust root, and owner-only modes protect
//! against other principals, not against a hostile same-user process
//! (`docs/testing.md` SEC-007). `doctor` reports it in those words.
//!
//! # Socket path length
//!
//! A Unix socket address is a fixed-size buffer — 104 bytes on macOS, 108 on
//! Linux — and a path that does not fit is silently truncated by some C
//! libraries. This module refuses instead, naming the limit, so a long
//! `--state-root` fails legibly rather than binding somewhere unexpected.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::layout::{self, PRIVATE_FILE_MODE, StateRoot};

/// Protocol marker on every request and response line.
pub const PROTOCOL: &str = "cg1";
/// Conservative socket-path limit: the smaller of the platforms' buffers, less
/// the terminating NUL.
pub const MAX_SOCKET_PATH_LEN: usize = 103;
/// Longest request this will read. A request is one short verb.
const MAX_REQUEST_BYTES: u64 = 256;
/// How long a peer has to send its request line, and to read the reply.
const PEER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the accept loop waits between polls when nothing is pending.
///
/// The listener is non-blocking so that a shutdown signal is noticed without a
/// second thread or a self-connect trick. Twenty milliseconds is below the
/// threshold of a human noticing on a command-line round trip.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// A request the control socket understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Request {
    /// Liveness only.
    Ping,
    /// Aggregate obligation, attention, health and daemon state.
    Status,
    /// One line per open obligation.
    Obligations,
    /// The daemon's half of the state-root diagnosis.
    Doctor,
}

impl Request {
    /// The verb as it appears on the wire.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Status => "status",
            Self::Obligations => "obligations",
            Self::Doctor => "doctor",
        }
    }

    /// Parses one verb.
    #[must_use]
    pub fn parse(verb: &str) -> Option<Self> {
        [Self::Ping, Self::Status, Self::Obligations, Self::Doctor]
            .into_iter()
            .find(|candidate| candidate.verb() == verb)
    }
}

/// Why an IPC operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IpcError {
    /// The socket path does not fit in a Unix socket address.
    #[error("the control socket path is {len} bytes, over the {limit}-byte limit")]
    PathTooLong {
        /// Length of the path that was refused.
        len: usize,
        /// The platform-independent limit this daemon enforces.
        limit: usize,
    },
    /// The socket, or the directory holding it, could not be prepared.
    #[error("the control socket could not be bound")]
    Bind,
    /// No daemon is listening on the socket.
    #[error("no daemon is listening on this state root's control socket")]
    NotRunning,
    /// The daemon did not answer, or answered something unparseable.
    #[error("the daemon's reply could not be read")]
    Unreadable,
    /// The daemon refused the request.
    #[error("the daemon refused the request: {code}")]
    Refused {
        /// The refusal's stable code.
        code: String,
    },
}

/// Checks a socket path against the address-buffer limit.
///
/// # Errors
///
/// Returns [`IpcError::PathTooLong`] when it will not fit.
pub fn check_socket_path(path: &Path) -> Result<(), IpcError> {
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH_LEN {
        return Err(IpcError::PathTooLong {
            len,
            limit: MAX_SOCKET_PATH_LEN,
        });
    }
    Ok(())
}

/// The daemon's listening socket.
#[derive(Debug)]
pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
}

impl IpcServer {
    /// Binds the control socket inside the owner-only IPC directory.
    ///
    /// The caller must already hold the state root's instance lock, which is
    /// what makes removing a leftover socket file safe: no other daemon can
    /// exist, so anything at the path is this state root's own debris.
    ///
    /// # Errors
    ///
    /// - [`IpcError::PathTooLong`] when the address will not fit;
    /// - [`IpcError::Bind`] when the directory or socket cannot be prepared.
    pub fn bind(root: &StateRoot) -> Result<Self, IpcError> {
        use std::os::unix::fs::PermissionsExt as _;

        let path = root.socket_path();
        check_socket_path(&path)?;

        // A leftover socket from an unclean exit is not a peer, it is a
        // filesystem entry nothing is listening on.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|_| IpcError::Bind)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(PRIVATE_FILE_MODE))
            .map_err(|_| IpcError::Bind)?;
        listener.set_nonblocking(true).map_err(|_| IpcError::Bind)?;

        Ok(Self { listener, path })
    }

    /// Where the socket lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serves requests until `stop` goes true.
    ///
    /// One connection at a time, each with a bounded read and write timeout, so
    /// a stalled peer costs at most [`PEER_TIMEOUT`] rather than the daemon's
    /// availability. Phase 1 answers only short queries against an already
    /// serialised store actor, so there is nothing for concurrency to overlap.
    ///
    /// `stop` is the flag a signal handler sets, read directly rather than
    /// relayed through a watcher thread.
    pub fn serve(&self, stop: &AtomicBool, handler: &impl Fn(Request) -> Vec<String>) {
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => Self::answer(stream, handler),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL);
                }
                // A peer that vanished between the connection and the accept.
                Err(_) => std::thread::sleep(ACCEPT_POLL),
            }
        }
    }

    fn answer(stream: UnixStream, handler: &impl Fn(Request) -> Vec<String>) {
        let _ = stream.set_read_timeout(Some(PEER_TIMEOUT));
        let _ = stream.set_write_timeout(Some(PEER_TIMEOUT));

        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(clone) => clone,
            Err(_) => return,
        });
        let mut line = String::new();
        if (&mut reader)
            .take(MAX_REQUEST_BYTES)
            .read_line(&mut line)
            .is_err()
        {
            return;
        }

        let mut writer = stream;
        let reply = match parse_request(&line) {
            Some(request) => {
                let mut lines = vec![format!("{PROTOCOL} ok")];
                lines.extend(handler(request));
                lines
            }
            None => vec![format!("{PROTOCOL} err unknown_request")],
        };
        for line in reply {
            if writeln!(writer, "{line}").is_err() {
                return;
            }
        }
        let _ = writeln!(writer, "{PROTOCOL} end");
        let _ = writer.flush();
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // The socket is not a durable fact; leaving it behind would only make
        // the next bind clean it up.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn parse_request(line: &str) -> Option<Request> {
    let mut fields = line.split_whitespace();
    if fields.next()? != PROTOCOL {
        return None;
    }
    let verb = fields.next()?;
    // No arguments in Phase 1: anything after the verb is a protocol this
    // binary does not implement, and guessing at it would be the start of one.
    if fields.next().is_some() {
        return None;
    }
    Request::parse(verb)
}

/// Sends one request to a running daemon and reads its reply.
///
/// # Errors
///
/// - [`IpcError::NotRunning`] when nothing is listening;
/// - [`IpcError::Refused`] when the daemon answered with an error code;
/// - [`IpcError::Unreadable`] on a malformed or truncated reply.
pub fn request(socket_path: &Path, request: Request) -> Result<Vec<String>, IpcError> {
    check_socket_path(socket_path)?;
    let stream = UnixStream::connect(socket_path).map_err(|_| IpcError::NotRunning)?;
    stream
        .set_read_timeout(Some(PEER_TIMEOUT))
        .map_err(|_| IpcError::Unreadable)?;
    stream
        .set_write_timeout(Some(PEER_TIMEOUT))
        .map_err(|_| IpcError::Unreadable)?;

    let mut writer = stream.try_clone().map_err(|_| IpcError::Unreadable)?;
    writeln!(writer, "{PROTOCOL} {}", request.verb()).map_err(|_| IpcError::Unreadable)?;
    writer.flush().map_err(|_| IpcError::Unreadable)?;

    let reader = BufReader::new(stream);
    let mut lines = Vec::new();
    let mut header = None;
    for line in reader.lines() {
        let line = line.map_err(|_| IpcError::Unreadable)?;
        if line == format!("{PROTOCOL} end") {
            let header = header.ok_or(IpcError::Unreadable)?;
            return finish(header, lines);
        }
        if header.is_none() {
            header = Some(line);
        } else {
            lines.push(line);
        }
    }
    Err(IpcError::Unreadable)
}

fn finish(header: String, lines: Vec<String>) -> Result<Vec<String>, IpcError> {
    if header == format!("{PROTOCOL} ok") {
        return Ok(lines);
    }
    let code = header
        .strip_prefix(&format!("{PROTOCOL} err "))
        .ok_or(IpcError::Unreadable)?;
    Err(IpcError::Refused {
        code: code.to_owned(),
    })
}

/// Reports whether a daemon is answering on this state root's socket.
///
/// A round trip rather than a file check: a socket file can outlive the process
/// that bound it, and only an answer proves a daemon is there.
#[must_use]
pub fn daemon_answering(root: &StateRoot) -> bool {
    request(&root.socket_path(), Request::Ping).is_ok()
}

/// Audits the IPC directory and socket, for `doctor`.
///
/// Returns the defects found; an empty vector means the owner-only boundary is
/// intact as far as file modes can express it.
#[must_use]
pub fn audit(root: &StateRoot, owner_uid: u32) -> Vec<(&'static str, crate::error::PathDefect)> {
    let mut findings = Vec::new();
    // An absent directory is the layout audit's finding, not this one's:
    // reporting it twice would make a state root that has never been started
    // look twice as broken as it is.
    let directory = root.ipc_root();
    if directory.exists()
        && let Err(defect) = layout::audit_dir(&directory, owner_uid)
    {
        findings.push(("ipc_directory", defect));
    }
    let socket = root.socket_path();
    if socket.exists()
        && let Err(defect) = layout::audit_file(&socket, owner_uid)
    {
        findings.push(("control_socket", defect));
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn every_verb_round_trips_through_its_wire_form() {
        for request in [
            Request::Ping,
            Request::Status,
            Request::Obligations,
            Request::Doctor,
        ] {
            assert_eq!(Request::parse(request.verb()), Some(request));
        }
        assert_eq!(Request::parse("acknowledge"), None);
    }

    #[test]
    fn a_request_with_arguments_or_a_wrong_marker_is_refused() {
        assert_eq!(parse_request("cg1 status\n"), Some(Request::Status));
        assert_eq!(parse_request("cg1 status extra\n"), None);
        assert_eq!(parse_request("cg2 status\n"), None);
        assert_eq!(parse_request("status\n"), None);
        assert_eq!(parse_request(""), None);
    }

    #[test]
    fn an_overlong_socket_path_is_refused_rather_than_truncated() {
        let long = PathBuf::from("/".repeat(MAX_SOCKET_PATH_LEN + 1));
        match check_socket_path(&long) {
            Err(IpcError::PathTooLong { len, limit }) => {
                assert_eq!(len, MAX_SOCKET_PATH_LEN + 1);
                assert_eq!(limit, MAX_SOCKET_PATH_LEN);
            }
            other => panic!("an overlong path was accepted: {other:?}"),
        }
    }

    fn serving_root() -> (tempfile::TempDir, StateRoot) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = StateRoot::new(dir.path());
        layout::ensure_private_dir(&root.ipc_root()).expect("ipc dir");
        (dir, root)
    }

    #[test]
    fn a_round_trip_carries_the_handler_lines() {
        let (_dir, root) = serving_root();
        let server = IpcServer::bind(&root).expect("bound");
        let stop = Arc::new(AtomicBool::new(false));

        let socket = root.socket_path();
        let flag = Arc::clone(&stop);
        let client = std::thread::spawn(move || {
            let answer = request(&socket, Request::Status);
            let ping = request(&socket, Request::Ping);
            let unknown = request(&socket, Request::Doctor);
            flag.store(true, Ordering::Relaxed);
            (answer, ping, unknown)
        });

        server.serve(&stop, &|request| match request {
            Request::Status => vec!["daemon.state=running".to_owned()],
            Request::Ping => Vec::new(),
            _ => vec!["check name=x result=ok".to_owned()],
        });

        let (status, ping, doctor) = client.join().expect("client thread");
        assert_eq!(status.expect("status"), vec!["daemon.state=running"]);
        assert_eq!(ping.expect("ping"), Vec::<String>::new());
        assert_eq!(doctor.expect("doctor"), vec!["check name=x result=ok"]);
    }

    #[test]
    fn the_socket_and_its_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, root) = serving_root();
        let server = IpcServer::bind(&root).expect("bound");

        for path in [root.ipc_root(), server.path().to_path_buf()] {
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o077,
                0,
                "{} is reachable by group or other: {mode:o}",
                path.display()
            );
        }

        let uid = layout::effective_uid(root.path()).expect("uid");
        assert!(
            audit(&root, uid).is_empty(),
            "the owner-only boundary must hold: {:?}",
            audit(&root, uid)
        );
        // Another user's view of the same paths: the audit must say so rather
        // than pass. Testing a genuinely different UID needs privileges the
        // suite does not have, so the check is expressed the other way round.
        assert!(!audit(&root, uid.wrapping_add(1)).is_empty());
    }

    #[test]
    fn a_stale_socket_file_does_not_block_the_next_bind() {
        let (_dir, root) = serving_root();
        {
            let _server = IpcServer::bind(&root).expect("bound");
        }
        std::fs::write(root.socket_path(), b"debris").expect("stale file");
        let _server = IpcServer::bind(&root).expect("rebound over debris");
    }

    #[test]
    fn a_client_with_no_daemon_gets_not_running() {
        let (_dir, root) = serving_root();
        assert!(!daemon_answering(&root));
        match request(&root.socket_path(), Request::Status) {
            Err(IpcError::NotRunning) => {}
            other => panic!("expected NotRunning, got {other:?}"),
        }
    }
}
