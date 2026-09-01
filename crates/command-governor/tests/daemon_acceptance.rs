//! Daemon and command-line acceptance tests, driving the real binary.
//!
//! # Coverage
//!
//! | Test | `docs/testing.md` | What it proves |
//! | --- | --- | --- |
//! | [`db_005_two_daemon_authority_rejected`] | DB-005 | two real processes, one state root; the second fails closed with a typed class |
//! | [`db_005_the_refusal_is_not_sqlite_serialization`] | DB-005 | the second process refuses *before* it has opened the database at all |
//! | [`a_stale_lock_is_reclaimed_after_the_holder_is_killed`] | DB-005 | the kernel-held lock is released by process death, and the next start says it reclaimed |
//! | [`a_clean_stop_releases_the_lock_and_removes_the_socket`] | — | `SIGTERM` shuts down cleanly |
//! | [`startup_refuses_when_a_pinned_artifact_is_missing`] | DB-008 | a failing step in the startup order refuses readiness and leaves the durable condition behind |
//! | [`sec_001_the_command_line_and_the_log_carry_no_forbidden_bytes`] | SEC-001 | the sweep, extended to the two surfaces Phase 1 has just created |
//! | [`sec_007_doctor_states_the_trust_model_without_overclaiming`] | SEC-007 | the trust model is reported as data, and claims no same-user containment |
//! | [`the_control_socket_and_its_directory_are_owner_only`] | local IPC abuse | the enforced half of the IPC boundary |
//! | [`doctor_works_offline_and_fails_legibly_on_a_broken_root`] | — | no daemon, no authority taken, legible exit codes |
//! | [`status_and_obligations_report_the_seeded_lifecycle`] | — | the CLI smoke test against seeded content |
//! | [`status_without_a_daemon_exits_not_running`] | — | the distinct exit code |
//! | [`the_daemon_makes_the_durable_authority_owner_only`] | SEC-007, ART-005 shape | the database and its sidecars end up `0600` whatever the host umask was |
//!
//! # Why these spawn real processes
//!
//! DB-005 is about two *operating-system processes* contending for one state
//! root. A same-process test can only prove that two calls into one lock module
//! disagree, which is the weaker claim `governor-daemon`'s unit tests already
//! make. Everything here runs `command-governor` itself, through
//! `CARGO_BIN_EXE_command-governor`, which is why this suite lives in the
//! binary's own crate: that variable is only set for its integration tests.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use governor_core::fence::SafeToken;
use governor_testkit::harness::Harness;
use governor_testkit::scenario::{
    FINAL_RESULT, accepted_work, open_turn, record_failure, start_worker,
};
use governor_testkit::sentinels::{
    FINAL_RESULT_SENTINEL, FORBIDDEN, assert_no_forbidden_bytes, assert_result_sentinel_confined,
    contains, sweep,
};

/// The binary under test.
const BINARY: &str = env!("CARGO_BIN_EXE_command-governor");

/// Exit codes, as `command-governor --help` documents them.
const EXIT_OK: i32 = 0;
const EXIT_REFUSED: i32 = 2;
const EXIT_UNHEALTHY: i32 = 3;
const EXIT_NOT_RUNNING: i32 = 4;

/// How long a daemon has to become ready before a test gives up.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a stopping daemon has to exit.
const STOP_TIMEOUT: Duration = Duration::from_secs(20);

// --- Driving the real binary -------------------------------------------------

/// Runs one command-line invocation to completion.
fn run(state_root: &Path, arguments: &[&str]) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .arg("--state-root")
        .arg(state_root)
        .output()
        .expect("running command-governor")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        stdout_of(output),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn code_of(output: &Output) -> i32 {
    output.status.code().expect("the process exited normally")
}

/// A daemon running in its own process, cleaned up however the test ends.
struct DaemonProcess {
    child: Option<Child>,
    state_root: PathBuf,
}

impl DaemonProcess {
    /// Starts `command-governor daemon` and waits until it answers.
    fn start(state_root: &Path) -> Self {
        let child = Command::new(BINARY)
            .arg("daemon")
            .arg("--state-root")
            .arg(state_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawning the daemon");
        let daemon = Self {
            child: Some(child),
            state_root: state_root.to_path_buf(),
        };
        daemon.wait_until_answering();
        daemon
    }

    /// Starts a daemon that is expected to refuse, and collects its output.
    fn start_expecting_refusal(state_root: &Path) -> Output {
        Command::new(BINARY)
            .arg("daemon")
            .arg("--state-root")
            .arg(state_root)
            .output()
            .expect("running the daemon")
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().expect("a running child").id()
    }

    fn wait_until_answering(&self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if code_of(&run(&self.state_root, &["status"])) == EXIT_OK {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the daemon never answered on {}", self.state_root.display());
    }

    /// Sends `SIGTERM` and waits for a clean exit.
    fn stop(mut self) -> Output {
        let pid = self.pid();
        assert!(signal(pid, "TERM"), "could not send SIGTERM to {pid}");
        let child = self.child.take().expect("a running child");
        wait_with_timeout(child, STOP_TIMEOUT)
    }

    /// Kills the process outright, simulating a crash.
    fn kill(mut self) {
        let mut child = self.child.take().expect("a running child");
        child.kill().expect("killing the daemon");
        child.wait().expect("reaping the daemon");
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Sends a signal by name, through the system `kill`.
///
/// `std::process::Child::kill` sends `SIGKILL`, which is the opposite of the
/// clean shutdown under test here, and the workspace denies `unsafe_code` so
/// there is no in-process `kill(2)`.
fn signal(pid: u32, name: &str) -> bool {
    for candidate in ["/bin/kill", "/usr/bin/kill"] {
        if !Path::new(candidate).exists() {
            continue;
        }
        return Command::new(candidate)
            .arg(format!("-{name}"))
            .arg(pid.to_string())
            .status()
            .is_ok_and(|status| status.success());
    }
    panic!("no `kill` binary to signal with");
}

/// Waits for a child, failing the test rather than hanging forever.
fn wait_with_timeout(mut child: Child, timeout: Duration) -> Output {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait().expect("polling the child") {
            Some(_) => {
                return child
                    .wait_with_output()
                    .expect("collecting the daemon's output");
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    let _ = child.kill();
    panic!("the daemon did not exit within {timeout:?}");
}

/// Every line of a `key=value` report, as a set for containment assertions.
fn has_line(text: &str, needle: &str) -> bool {
    text.lines().any(|line| line == needle)
}

fn has_field(text: &str, needle: &str) -> bool {
    text.lines().any(|line| line.contains(needle))
}

// --- Seeding -----------------------------------------------------------------

/// A state root carrying one accepted, unprocessed result and one failure.
///
/// Uses the deterministic harness to write the content, then closes it, so the
/// daemon opens a state root exactly as it would find one after a restart.
fn seeded_root() -> Harness {
    let harness = Harness::new();
    {
        let store = harness.open().expect("opening the seeded store");
        let mut artifacts = harness.open_artifacts();
        let _accepted = accepted_work(&store, &mut artifacts, "conv-A");

        let failed = open_turn(&store);
        start_worker(&store, failed.obligation, "run-2");
        record_failure(&store, failed.obligation, "run-2").expect("recording a worker failure");

        // A third obligation left in `created`, so the status summary has more
        // than one state to report.
        let _created = open_turn(&store);
    }
    harness
}

// --- DB-005 ------------------------------------------------------------------

#[test]
fn db_005_two_daemon_authority_rejected() {
    let harness = Harness::new();
    let root = harness.state_root();

    let first = DaemonProcess::start(root);
    let second = DaemonProcess::start_expecting_refusal(root);

    assert_eq!(
        code_of(&second),
        EXIT_REFUSED,
        "the second daemon must fail closed: {}",
        combined(&second)
    );
    let text = combined(&second);
    assert!(
        has_line(&text, "error class=authority_held"),
        "the refusal must be machine-classifiable: {text}"
    );
    assert!(
        text.contains(&format!("{}", first.pid())),
        "the refusal must name the holder so an operator can act: {text}"
    );
    assert!(
        !text.contains("daemon.state=ready"),
        "the second process must never have become ready: {text}"
    );

    // The first daemon is unharmed: it is still the authority and still
    // answering.
    assert_eq!(code_of(&run(root, &["status"])), EXIT_OK);
    let stopped = first.stop();
    assert_eq!(code_of(&stopped), EXIT_OK);
}

#[test]
fn db_005_the_refusal_is_not_sqlite_serialization() {
    // `docs/testing.md` DB-005: "SQLite writer serialization alone is not
    // accepted as daemon election". The observable difference is *when* the
    // second process gives up. Election happens at the state root before the
    // database is touched, so a second daemon started against a state root that
    // has no database yet must still refuse — there is no writer to serialize
    // against.
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("fresh");
    std::fs::create_dir(&root).expect("a bare state root");

    let first = DaemonProcess::start(&root);
    let second = DaemonProcess::start_expecting_refusal(&root);
    assert!(has_line(&combined(&second), "error class=authority_held"));

    // And the database the first daemon created was never advanced twice: the
    // epoch a second authoritative open would have bumped is still the first's.
    let status = run(&root, &["status"]);
    assert!(
        has_line(&stdout_of(&status), "daemon.epoch=1"),
        "a second authority would have advanced the epoch: {}",
        stdout_of(&status)
    );
    first.stop();
}

#[test]
fn a_stale_lock_is_reclaimed_after_the_holder_is_killed() {
    let harness = Harness::new();
    let root = harness.state_root();

    let first = DaemonProcess::start(root);
    let killed_pid = first.pid();
    first.kill();

    // The lock file is still there, still saying `held` — a killed process
    // writes no release marker. What lets the next daemon in is the kernel
    // having dropped the advisory lock when the process died.
    assert!(root.join("daemon.lock").exists());
    let record = std::fs::read_to_string(root.join("daemon.lock")).expect("the lock record");
    assert!(record.contains("state=held"), "record was {record}");
    assert!(record.contains(&format!("slot={killed_pid}")));

    let second = DaemonProcess::start(root);
    assert_ne!(second.pid(), killed_pid);
    let doctor = run(root, &["doctor"]);
    assert!(
        has_line(&stdout_of(&doctor), "live.daemon.reclaimed_stale_lock=true"),
        "the reclaim must be reported: {}",
        stdout_of(&doctor)
    );
    second.stop();
}

#[test]
fn a_clean_stop_releases_the_lock_and_removes_the_socket() {
    let harness = Harness::new();
    let root = harness.state_root();

    let daemon = DaemonProcess::start(root);
    assert!(root.join("ipc").join("d.sock").exists());
    let output = daemon.stop();

    assert_eq!(code_of(&output), EXIT_OK, "{}", combined(&output));
    assert!(has_line(&stdout_of(&output), "daemon.state=stopped"));
    assert!(
        !root.join("ipc").join("d.sock").exists(),
        "a clean stop must remove the socket"
    );

    let record = std::fs::read_to_string(root.join("daemon.lock")).expect("the lock record");
    assert!(
        record.contains("state=released"),
        "a clean stop must mark the lock released: {record}"
    );

    // And the next start is unremarkable.
    let again = DaemonProcess::start(root);
    again.stop();
}

// --- Startup order -----------------------------------------------------------

#[test]
fn startup_refuses_when_a_pinned_artifact_is_missing() {
    let harness = seeded_root();
    let root = harness.state_root();

    // Remove the bytes an open obligation pins. Everything else about the state
    // root is intact, so the only thing that can refuse is step 8.
    let objects = harness.artifact_root().join("objects");
    let published: Vec<PathBuf> = std::fs::read_dir(&objects)
        .expect("the objects directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    assert!(!published.is_empty(), "the seed must have published one");
    for path in &published {
        std::fs::remove_file(path).expect("removing the pinned artifact");
    }

    let refused = DaemonProcess::start_expecting_refusal(root);
    assert_eq!(
        code_of(&refused),
        EXIT_REFUSED,
        "readiness must be refused: {}",
        combined(&refused)
    );
    let text = combined(&refused);
    assert!(
        has_line(&text, "error class=result_artifact_missing"),
        "the refusal must be classified: {text}"
    );
    assert!(
        !text.contains("ipc.socket=bound"),
        "a refused daemon must never have bound its socket: {text}"
    );

    // The health condition is durable: it survives the refusal, which is what
    // makes the reason discoverable after the process is gone.
    let doctor = run(root, &["doctor"]);
    assert_eq!(code_of(&doctor), EXIT_UNHEALTHY);
    assert!(
        has_field(
            &stdout_of(&doctor),
            "check name=pinned_artifacts result=result_artifact_missing"
        ),
        "{}",
        stdout_of(&doctor)
    );
    assert!(has_field(
        &stdout_of(&doctor),
        "health.kind.result_artifact_missing="
    ));
}

// --- CLI smoke ---------------------------------------------------------------

#[test]
fn status_and_obligations_report_the_seeded_lifecycle() {
    let harness = seeded_root();
    let root = harness.state_root();
    let daemon = DaemonProcess::start(root);

    let status = stdout_of(&run(root, &["status"]));
    assert!(has_line(&status, "daemon.state=running"), "{status}");
    assert!(has_line(&status, "store.schema_epoch=1"), "{status}");
    assert!(has_line(&status, "obligations.open=3"), "{status}");
    assert!(has_line(&status, "obligations.attention=2"), "{status}");
    assert!(
        has_line(&status, "obligations.state.completed_unprocessed=1"),
        "{status}"
    );
    assert!(has_line(&status, "obligations.state.failed=1"), "{status}");
    assert!(has_line(&status, "obligations.state.created=1"), "{status}");
    assert!(has_line(&status, "artifacts.verified=1"), "{status}");
    assert!(has_line(&status, "health.open=0"), "{status}");

    let obligations = stdout_of(&run(root, &["obligations"]));
    assert_eq!(
        obligations.lines().count(),
        3,
        "one line per open obligation: {obligations}"
    );
    assert!(
        obligations
            .lines()
            .all(|line| line.starts_with("obligation id="))
    );
    assert!(has_field(&obligations, "state=completed_unprocessed"));
    assert!(has_field(&obligations, "state=failed"));
    assert!(has_field(&obligations, "kind=worker_turn"));

    daemon.stop();
}

#[test]
fn status_without_a_daemon_exits_not_running() {
    let harness = seeded_root();
    let root = harness.state_root();
    for command in [["status"], ["obligations"]] {
        let output = run(root, &command);
        assert_eq!(code_of(&output), EXIT_NOT_RUNNING, "{}", combined(&output));
        assert!(has_line(
            &combined(&output),
            "error class=daemon_not_running"
        ));
    }
}

// --- doctor ------------------------------------------------------------------

#[test]
fn doctor_works_offline_and_fails_legibly_on_a_broken_root() {
    use std::os::unix::fs::PermissionsExt as _;

    // A directory that does not exist at all.
    let dir = tempfile::tempdir().expect("temp dir");
    let absent = dir.path().join("nowhere");
    let output = run(&absent, &["doctor"]);
    assert_eq!(code_of(&output), EXIT_UNHEALTHY, "{}", combined(&output));
    assert!(has_line(&stdout_of(&output), "doctor.result=unhealthy"));
    assert!(has_field(
        &stdout_of(&output),
        "check name=state_root_writable result=not_writable_by_this_user"
    ));
    assert!(
        !absent.exists(),
        "doctor must not create the root it examines"
    );

    // A bare directory: nothing is wrong, there is simply nothing there.
    let bare = dir.path().join("bare");
    std::fs::create_dir(&bare).expect("a bare root");
    // Owner-only, because a state root that group or other can traverse is a
    // finding in its own right — asserted separately below.
    std::fs::set_permissions(&bare, std::fs::Permissions::from_mode(0o700))
        .expect("a private bare root");
    let output = run(&bare, &["doctor"]);
    assert_eq!(code_of(&output), EXIT_OK, "{}", combined(&output));
    assert!(has_field(
        &stdout_of(&output),
        "name=database result=absent"
    ));
    assert!(has_field(
        &stdout_of(&output),
        "name=instance_lock result=absent"
    ));
    assert!(
        !bare.join("governor.sqlite3").exists(),
        "doctor must take no authority and create no database"
    );

    // A database that is not one.
    let broken = dir.path().join("broken");
    std::fs::create_dir(&broken).expect("a root");
    std::fs::write(broken.join("governor.sqlite3"), b"not a database").expect("staged");
    let output = run(&broken, &["doctor"]);
    assert_eq!(code_of(&output), EXIT_UNHEALTHY, "{}", combined(&output));
    assert!(
        has_field(&stdout_of(&output), "check name=database result=")
            && !has_field(&stdout_of(&output), "check name=database result=ok"),
        "{}",
        stdout_of(&output)
    );
    assert_eq!(
        std::fs::read(broken.join("governor.sqlite3")).expect("still there"),
        b"not a database",
        "doctor must not repair what it diagnoses"
    );
}

#[test]
fn doctor_reports_a_running_daemon_and_reaches_it() {
    let harness = seeded_root();
    let root = harness.state_root();
    let daemon = DaemonProcess::start(root);

    let output = run(root, &["doctor"]);
    assert_eq!(code_of(&output), EXIT_OK, "{}", combined(&output));
    let text = stdout_of(&output);
    assert!(has_line(&text, "doctor.daemon_running=true"), "{text}");
    assert!(
        has_field(&text, "check name=instance_lock result=held"),
        "{text}"
    );
    // The seeded root was already opened once, so the epoch is whatever this
    // process advanced it to; what matters is that the *daemon* supplied it.
    assert!(has_field(&text, "live.daemon.epoch="), "{text}");
    assert!(has_field(&text, "live.artifacts.verified="), "{text}");
    assert!(
        has_field(&text, "check name=projection_replay result="),
        "{text}"
    );

    daemon.stop();

    // With the daemon gone the same command still works, and says so.
    let output = run(root, &["doctor"]);
    assert_eq!(code_of(&output), EXIT_OK, "{}", combined(&output));
    assert!(has_line(&stdout_of(&output), "doctor.daemon_running=false"));
}

#[test]
fn sec_007_doctor_states_the_trust_model_without_overclaiming() {
    let harness = seeded_root();
    let root = harness.state_root();
    let text = stdout_of(&run(root, &["doctor"]));

    assert!(has_line(&text, "trust.model=os_user_account"), "{text}");
    assert!(
        has_line(&text, "trust.owner_only_file_modes=true"),
        "{text}"
    );
    assert!(
        has_line(&text, "trust.protects_from_other_os_users=true"),
        "{text}"
    );
    // The claim SEC-007 exists to forbid.
    assert!(
        has_line(&text, "trust.same_user_containment=false"),
        "doctor must not imply a same-user sandbox: {text}"
    );
    assert!(
        has_line(&text, "trust.ipc_peer_credential_check=false"),
        "the IPC boundary must be described as what it is: {text}"
    );
    assert!(
        has_line(&text, "trust.ipc_boundary=owner_only_directory_mode"),
        "{text}"
    );
    assert!(
        !text.to_lowercase().contains("sandbox"),
        "no output may suggest containment: {text}"
    );
}

// --- Local IPC ---------------------------------------------------------------

#[test]
fn the_control_socket_and_its_directory_are_owner_only() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let harness = Harness::new();
    let root = harness.state_root();
    let daemon = DaemonProcess::start(root);

    let directory = root.join("ipc");
    let socket = directory.join("d.sock");
    let me = std::fs::metadata(root.join("daemon.lock"))
        .expect("the lock this user's daemon created")
        .uid();

    for path in [&directory, &socket] {
        let metadata = std::fs::symlink_metadata(path).expect("metadata");
        assert_eq!(
            metadata.permissions().mode() & 0o077,
            0,
            "{} is reachable by group or other",
            path.display()
        );
        assert_eq!(metadata.uid(), me, "{} has a foreign owner", path.display());
        assert!(!metadata.file_type().is_symlink());
    }
    assert_eq!(
        std::fs::metadata(&directory)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    // A wrong-UID peer cannot be produced without another OS account, so the
    // enforced boundary is asserted the only way a same-user test can: the
    // directory's mode and owner are what stop another principal reaching the
    // socket at all, and `doctor` states that this is the whole of it.
    assert!(has_line(
        &stdout_of(&run(root, &["doctor"])),
        "trust.ipc_peer_credential_check=false"
    ));

    daemon.stop();
}

#[test]
fn the_daemon_makes_the_durable_authority_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    // The harness writes the database through `rusqlite`, which creates it
    // under the host umask — `0644` on a default account. The state root's
    // `0700` keeps other principals out either way, but the daemon must not
    // leave the durable authority as the one file in the layout relying on its
    // parent for that.
    let harness = seeded_root();
    let root = harness.state_root();
    let database = root.join("governor.sqlite3");
    let before = std::fs::metadata(&database)
        .expect("the seeded database")
        .permissions()
        .mode()
        & 0o777;

    let offline = run(root, &["doctor"]);
    if before & 0o077 != 0 {
        assert!(
            has_field(&stdout_of(&offline), "check name=database_file_private"),
            "doctor must report a world-readable database: {}",
            stdout_of(&offline)
        );
    }

    let daemon = DaemonProcess::start(root);
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", database.display()));
        if !path.exists() {
            continue;
        }
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "{} is still reachable by group or other: {mode:o}",
            path.display()
        );
    }
    let online = run(root, &["doctor"]);
    assert!(
        !has_field(&stdout_of(&online), "check name=database_file_private"),
        "{}",
        stdout_of(&online)
    );
    daemon.stop();
}

// --- SEC-001, extended to the new surfaces -----------------------------------

#[test]
fn sec_001_the_command_line_and_the_log_carry_no_forbidden_bytes() {
    // The structural half first: most of the corpus cannot reach any API,
    // because every string-shaped value the daemon accepts is a `SafeToken`.
    for sentinel in FORBIDDEN {
        assert_eq!(
            SafeToken::new(sentinel.value).is_ok(),
            sentinel.token_shaped,
            "{}: the corpus disagrees with the charset",
            sentinel.label
        );
    }

    let harness = Harness::new();
    let root = harness.state_root();
    {
        let store = harness.open().expect("opening the seeded store");
        let mut artifacts = harness.open_artifacts();
        // The one place a sentinel is allowed to become durable: the bounded
        // final assistant result.
        let work = accepted_work(&store, &mut artifacts, "conv-A");
        assert!(contains(FINAL_RESULT, FINAL_RESULT_SENTINEL.as_bytes()));
        let failed = open_turn(&store);
        start_worker(&store, failed.obligation, "run-2");
        record_failure(&store, failed.obligation, "run-2").expect("a worker failure");
        let _ = work;
    }

    let daemon = DaemonProcess::start(root);
    let mut surfaces: Vec<(String, Vec<u8>)> = Vec::new();
    for command in [
        vec!["status"],
        vec!["obligations"],
        vec!["doctor"],
        vec!["help"],
        vec!["version"],
    ] {
        let output = run(root, &command);
        surfaces.push((
            format!("cli:{}:stdout", command.join(" ")),
            output.stdout.clone(),
        ));
        surfaces.push((format!("cli:{}:stderr", command.join(" ")), output.stderr));
    }
    let stopped = daemon.stop();
    surfaces.push(("cli:daemon:stdout".to_owned(), stopped.stdout.clone()));
    surfaces.push(("cli:daemon:stderr".to_owned(), stopped.stderr));

    assert_no_forbidden_bytes(&surfaces, "command-line output");
    // Nor may the *allowed* sentinel reach the command line: the result artifact
    // holds it, and no status surface quotes an artifact's contents.
    assert!(
        sweep(
            &surfaces,
            &[governor_testkit::sentinels::Sentinel {
                label: "final assistant result",
                value: FINAL_RESULT_SENTINEL,
                token_shaped: true,
            }]
        )
        .is_empty(),
        "the command line must never echo result content"
    );

    // Then every file the daemon left behind, which now includes `logs/`.
    let files = harness.all_files();
    assert!(
        files
            .iter()
            .any(|(name, bytes)| name.starts_with("logs/") && !bytes.is_empty()),
        "the daemon must have written diagnostics for the sweep to mean anything: {:?}",
        files.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert_no_forbidden_bytes(&files, "the state root after a daemon lifecycle");
    assert_result_sentinel_confined(
        &files,
        "artifacts/objects/",
        "the state root after a daemon lifecycle",
    );
}
