//! The state root's shape, and the ownership/permission check over it.
//!
//! # Layout
//!
//! ```text
//! <root>/governor.sqlite3[-wal][-shm]   the durable authority
//! <root>/daemon.lock                    the single-daemon instance lock
//! <root>/artifacts/objects/             published immutable results
//! <root>/artifacts/incoming/            publication staging
//! <root>/artifacts/quarantine/          orphans the sweep set aside
//! <root>/ipc/d.sock                     the owner-local control socket
//! <root>/logs/daemon.log                safe diagnostics
//! ```
//!
//! This matches the layout `governor-testkit`'s harness already creates, so the
//! SEC-001 sweep over "every file under the state root" covers the daemon's
//! surfaces without being widened (`crates/governor-testkit/src/harness.rs`).
//!
//! # What the check does and does not claim
//!
//! Step 2 of `docs/architecture.md`'s startup order is *validate filesystem
//! ownership/permissions*. That is what [`audit`] does: every directory must
//! exist, be a directory rather than a symbolic link, be owned by the effective
//! user, and grant nothing to group or other.
//!
//! This protects the state root **from other OS principals**. It is not a
//! hostile same-user sandbox and must never be described as one
//! (`docs/testing.md` SEC-007). A worker process running as the same user as
//! the daemon is not contained by `0700`.

use core::fmt;
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crate::error::PathDefect;

/// Name of the durable authority inside a state root.
const DATABASE_FILE: &str = "governor.sqlite3";
/// Name of the single-daemon instance lock.
const LOCK_FILE: &str = "daemon.lock";
/// Directory holding the immutable result artifacts.
const ARTIFACTS_DIR: &str = "artifacts";
/// Directory holding the owner-local control socket.
const IPC_DIR: &str = "ipc";
/// Directory holding safe diagnostics.
const LOGS_DIR: &str = "logs";
/// Socket name.
///
/// Deliberately short: a Unix socket address is a fixed-size buffer, so every
/// byte spent here is a byte the operator cannot spend on the state root's own
/// path (see [`crate::ipc`]).
const SOCKET_FILE: &str = "d.sock";

/// Directory mode for everything the daemon owns: owner only.
pub(crate) const PRIVATE_DIR_MODE: u32 = 0o700;
/// File mode for everything the daemon owns: owner only.
pub(crate) const PRIVATE_FILE_MODE: u32 = 0o600;
/// Bits that must be clear on anything the daemon owns.
const GROUP_AND_OTHER: u32 = 0o077;

/// One directory the daemon depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum PathClass {
    /// The state root itself.
    StateRoot,
    /// The result-artifact root.
    Artifacts,
    /// The control-socket directory.
    Ipc,
    /// The diagnostics directory.
    Logs,
}

impl PathClass {
    /// Every directory the audit covers, in validation order.
    pub(crate) const ALL: &'static [Self] =
        &[Self::StateRoot, Self::Artifacts, Self::Ipc, Self::Logs];

    /// Stable `snake_case` code for diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StateRoot => "state_root",
            Self::Artifacts => "artifacts",
            Self::Ipc => "ipc",
            Self::Logs => "logs",
        }
    }
}

impl fmt::Display for PathClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Where one Command Governor installation keeps its durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRoot {
    root: PathBuf,
}

impl StateRoot {
    /// Names a state root without touching the filesystem.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default per-user location.
    ///
    /// `CG_STATE_ROOT` wins, then `XDG_STATE_HOME/command-governor`, then the
    /// platform's own per-user data location. Returns `None` only when neither
    /// `HOME` nor an override is set, which the command line reports as a usage
    /// error rather than guessing at `/`.
    #[must_use]
    pub fn default_location() -> Option<Self> {
        if let Some(explicit) = non_empty_var("CG_STATE_ROOT") {
            return Some(Self::new(explicit));
        }
        if let Some(xdg) = non_empty_var("XDG_STATE_HOME") {
            return Some(Self::new(Path::new(&xdg).join("command-governor")));
        }
        let home = PathBuf::from(non_empty_var("HOME")?);
        let relative = if cfg!(target_os = "macos") {
            Path::new("Library").join("Application Support")
        } else {
            Path::new(".local").join("state")
        };
        Some(Self::new(home.join(relative).join("command-governor")))
    }

    /// The root directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Where the durable authority lives.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.root.join(DATABASE_FILE)
    }

    /// Where the single-daemon instance lock lives.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    /// Where published artifacts live.
    #[must_use]
    pub fn artifact_root(&self) -> PathBuf {
        self.root.join(ARTIFACTS_DIR)
    }

    /// Where the control socket lives.
    #[must_use]
    pub fn ipc_root(&self) -> PathBuf {
        self.root.join(IPC_DIR)
    }

    /// The control socket itself.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.ipc_root().join(SOCKET_FILE)
    }

    /// The database and the sidecars SQLite may have created beside it.
    ///
    /// The write-ahead log and the shared-memory index come and go, so the
    /// caller checks each for existence rather than assuming all three.
    #[must_use]
    pub fn database_files(&self) -> Vec<PathBuf> {
        ["", "-wal", "-shm"]
            .iter()
            .map(|suffix| {
                let mut name = self.database_path().into_os_string();
                name.push(suffix);
                PathBuf::from(name)
            })
            .collect()
    }

    /// Where safe diagnostics are written.
    #[must_use]
    pub fn log_root(&self) -> PathBuf {
        self.root.join(LOGS_DIR)
    }

    /// The directory backing one path class.
    #[must_use]
    pub fn directory(&self, class: PathClass) -> PathBuf {
        match class {
            PathClass::StateRoot => self.root.clone(),
            PathClass::Artifacts => self.artifact_root(),
            PathClass::Ipc => self.ipc_root(),
            PathClass::Logs => self.log_root(),
        }
    }
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Creates a directory owner-only, forcing the mode past the host umask.
///
/// # Errors
///
/// Returns [`PathDefect::Uncreatable`] when the directory cannot be created or
/// its mode cannot be set.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), PathDefect> {
    fs::create_dir_all(path).map_err(|_| PathDefect::Uncreatable)?;
    // `create_dir_all` applies the umask, which a hostile or merely careless
    // one can widen. Setting the mode explicitly afterwards is what makes the
    // guarantee independent of the environment the daemon was started from.
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|_| PathDefect::Uncreatable)
}

/// Checks one directory's type, ownership and mode.
///
/// # Errors
///
/// Returns the first defect found; there is no partial pass.
pub(crate) fn audit_dir(path: &Path, owner_uid: u32) -> Result<(), PathDefect> {
    // `symlink_metadata`, not `metadata`: the question is what this path *is*,
    // and following the link would answer a different one.
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PathDefect::Uncreatable
        } else {
            PathDefect::Unreadable
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PathDefect::Symlink);
    }
    if !metadata.is_dir() {
        return Err(PathDefect::NotADirectory);
    }
    if metadata.uid() != owner_uid {
        return Err(PathDefect::ForeignOwner);
    }
    if metadata.permissions().mode() & GROUP_AND_OTHER != 0 {
        return Err(PathDefect::GroupOrOtherAccessible);
    }
    Ok(())
}

/// Name of the throwaway file used to learn this process's effective user.
const OWNER_PROBE: &str = ".cg-owner-probe";

/// Learns this process's effective user by creating a file and reading it back.
///
/// POSIX gives a newly created file the creating process's effective user, so
/// the owner of a file this call just made *is* the answer, with no `geteuid`
/// and therefore no raw C call in a workspace that denies `unsafe_code`. The
/// probe is removed before returning; a leftover from a crashed run is cleared
/// first, so a stale probe cannot supply somebody else's answer.
///
/// # Errors
///
/// Returns [`PathDefect::Uncreatable`] when the probe cannot be created, which
/// also means the state root is not writable by this process.
pub(crate) fn effective_uid(root: &Path) -> Result<u32, PathDefect> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let probe = root.join(OWNER_PROBE);
    let _ = fs::remove_file(&probe);
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&probe)
        .map_err(|_| PathDefect::Uncreatable)?;
    let uid = file.metadata().map_err(|_| PathDefect::Unreadable)?.uid();
    drop(file);
    let _ = fs::remove_file(&probe);
    Ok(uid)
}

/// Forces a file owner-only, past whatever umask created it.
///
/// # Errors
///
/// Returns [`PathDefect::Uncreatable`] when the mode cannot be set.
pub(crate) fn make_file_private(path: &Path) -> Result<(), PathDefect> {
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|_| PathDefect::Uncreatable)
}

/// Checks one file's type, ownership and mode.
///
/// # Errors
///
/// Returns the first defect found.
pub(crate) fn audit_file(path: &Path, owner_uid: u32) -> Result<(), PathDefect> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PathDefect::Uncreatable
        } else {
            PathDefect::Unreadable
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PathDefect::Symlink);
    }
    if metadata.is_dir() {
        return Err(PathDefect::NotADirectory);
    }
    if metadata.uid() != owner_uid {
        return Err(PathDefect::ForeignOwner);
    }
    if metadata.permissions().mode() & GROUP_AND_OTHER != 0 {
        return Err(PathDefect::GroupOrOtherAccessible);
    }
    Ok(())
}

/// One directory's audit outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryAudit {
    /// Which directory.
    pub class: PathClass,
    /// What is wrong with it, if anything.
    pub defect: Option<PathDefect>,
}

/// Audits every directory the daemon depends on, without creating any.
///
/// This is the read-only half, used by `doctor` against a state root the
/// caller may not own.
#[must_use]
pub fn audit(root: &StateRoot, owner_uid: u32) -> Vec<DirectoryAudit> {
    PathClass::ALL
        .iter()
        .map(|&class| DirectoryAudit {
            class,
            defect: audit_dir(&root.directory(class), owner_uid).err(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_path_stays_inside_the_root() {
        let root = StateRoot::new("/tmp/cg-test-root");
        for path in [
            root.database_path(),
            root.lock_path(),
            root.artifact_root(),
            root.ipc_root(),
            root.socket_path(),
            root.log_root(),
        ] {
            assert!(
                path.starts_with(root.path()),
                "{} escaped the state root",
                path.display()
            );
        }
    }

    #[test]
    fn a_group_readable_directory_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("widened");
        ensure_private_dir(&path).expect("created");
        let uid = fs::metadata(&path).expect("metadata").uid();
        assert_eq!(audit_dir(&path, uid), Ok(()));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).expect("widened");
        assert_eq!(
            audit_dir(&path, uid),
            Err(PathDefect::GroupOrOtherAccessible)
        );
    }

    #[test]
    fn a_symlinked_directory_is_refused_rather_than_followed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().join("real");
        ensure_private_dir(&real).expect("created");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let uid = fs::metadata(&real).expect("metadata").uid();
        assert_eq!(audit_dir(&link, uid), Err(PathDefect::Symlink));
    }

    #[test]
    fn a_foreign_owner_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("owned");
        ensure_private_dir(&path).expect("created");
        let uid = fs::metadata(&path).expect("metadata").uid();
        assert_eq!(
            audit_dir(&path, uid.wrapping_add(1)),
            Err(PathDefect::ForeignOwner)
        );
    }

    #[test]
    fn a_missing_directory_is_reported_rather_than_created() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("absent");
        assert_eq!(audit_dir(&path, 0), Err(PathDefect::Uncreatable));
        assert!(!path.exists(), "the audit must not create anything");
    }
}
