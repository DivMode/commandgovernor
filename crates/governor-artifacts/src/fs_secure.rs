//! The narrow set of filesystem primitives everything else is built from.
//!
//! Three properties are enforced here once, so no call site has to remember
//! them.
//!
//! # 1. Owner-only, regardless of umask
//!
//! `OpenOptions::mode` and `DirBuilder::mode` are *requests*: the kernel masks
//! them with the process umask. Measured on this platform, creating a file with
//! `mode(0o600)` under `umask 0700` produces mode `000` — not merely a laxer
//! mode, the wrong one entirely. So every created object is immediately
//! `fchmod`/`chmod`ed to its intended mode, which the umask does not touch.
//! That is what `docs/testing.md` ART-005 ("regardless of host umask") asks
//! for, and it cannot be satisfied by the creation mode alone.
//!
//! # 2. No-follow
//!
//! Every open of a name inside the root passes `O_NOFOLLOW`, and directories
//! also pass `O_DIRECTORY`. A symlink planted at a key therefore fails the open
//! with `ELOOP` instead of redirecting a read or a write outside the root
//! (`docs/testing.md` ART-004, SEC-008). Creation additionally uses `O_EXCL`,
//! which refuses an existing name of any kind, symlink included.
//!
//! # 3. What this does *not* claim
//!
//! `SECURITY.md` "Local trust model": owner-only modes protect the store from
//! **other OS principals**. They are not a hostile same-user sandbox, and
//! nothing here should be read as one. A process running as the same user can
//! rename a directory between a check and an open, and no combination of
//! `O_NOFOLLOW` and component validation closes that — only a real isolation
//! boundary would. Rust's standard library exposes no `openat`, so the
//! resolution here is path-based; the residual window is same-user only, which
//! is explicitly outside the V1 boundary.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::error::{ArtifactError, ArtifactResult, FsOperation, UnsafePathReason};

/// Mode every directory in the artifact root is held at.
pub(crate) const DIR_MODE: u32 = 0o700;

/// Mode every stored artifact and staging file is held at.
pub(crate) const FILE_MODE: u32 = 0o600;

/// Bits that must be clear for something to be owner-only.
pub(crate) const GROUP_AND_OTHER: u32 = 0o077;

/// Opens a layout directory, creating and repairing it to [`DIR_MODE`].
///
/// The returned handle is proven: it was opened `O_DIRECTORY | O_NOFOLLOW`, and
/// its own metadata — not the path's — says it is a directory with no group or
/// other bits.
pub(crate) fn owner_only_dir(path: &Path, label: &str) -> ArtifactResult<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(unsafe_path(label, UnsafePathReason::Symlink));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(unsafe_path(label, UnsafePathReason::NotDirectory));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match DirBuilder::new().mode(DIR_MODE).create(path) {
                Ok(()) => {}
                // Another process in the same daemon start-up got there first;
                // the repair below covers it either way.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(ArtifactError::io(FsOperation::PrepareLayout, error)),
            }
        }
        Err(error) => return Err(ArtifactError::io(FsOperation::Stat, error)),
    }

    // The umask may have stripped the creation mode down to nothing, and a
    // directory left over from an earlier, laxer build may be too open. One
    // unconditional chmod covers both.
    fs::set_permissions(path, Permissions::from_mode(DIR_MODE))
        .map_err(|error| ArtifactError::io(FsOperation::SetPermissions, error))?;

    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| classify_open(label, FsOperation::OpenDirectory, error))?;

    let metadata = dir
        .metadata()
        .map_err(|error| ArtifactError::io(FsOperation::Stat, error))?;
    if !metadata.is_dir() {
        return Err(unsafe_path(label, UnsafePathReason::NotDirectory));
    }
    if metadata.mode() & GROUP_AND_OTHER != 0 {
        return Err(unsafe_path(label, UnsafePathReason::NotOwnerOnly));
    }
    Ok(dir)
}

/// Creates an exclusive, owner-only staging file.
///
/// `create_new` is `O_CREAT | O_EXCL`, so an existing name of any kind —
/// regular file, directory, symlink — fails the call rather than being written
/// through.
pub(crate) fn create_owner_only_file(path: &Path) -> ArtifactResult<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            // `O_EXCL` refuses a symlink with `EEXIST`, not `ELOOP`, so the
            // symlink case here is indistinguishable from an ordinary
            // name clash and is reported as the operational failure it is.
            ArtifactError::io(FsOperation::CreateStaging, error)
        })?;
    enforce_file_mode(&file)?;
    Ok(file)
}

/// Forces [`FILE_MODE`] on an open handle, defeating the umask.
pub(crate) fn enforce_file_mode(file: &File) -> ArtifactResult<()> {
    file.set_permissions(Permissions::from_mode(FILE_MODE))
        .map_err(|error| ArtifactError::io(FsOperation::SetPermissions, error))
}

/// Opens a stored artifact for reading, proving it is a plain single-linked
/// regular file.
pub(crate) fn open_stored_file(path: &Path, label: &str) -> ArtifactResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| classify_open(label, FsOperation::OpenArtifact, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| ArtifactError::io(FsOperation::Stat, error))?;
    if !metadata.is_file() {
        return Err(unsafe_path(label, UnsafePathReason::NotRegularFile));
    }
    if metadata.nlink() != 1 {
        return Err(unsafe_path(label, UnsafePathReason::HardLinked));
    }
    Ok(file)
}

/// `fsync`s a handle.
///
/// # Platform note
///
/// On Apple platforms `File::sync_all` is implemented as
/// `fcntl(F_FULLFSYNC)`, which flushes the drive's own write cache — plain
/// `fsync(2)` does not. Measured on this machine over 20 iterations:
/// `File::sync_all` 4.58 ms, `fcntl(F_FULLFSYNC)` 3.64 ms, `fsync(2)` 63 µs.
/// The two-orders-of-magnitude gap is the device-cache flush, so `sync_all`
/// is the strong barrier and not the cheap one. Directory handles accept it
/// too, which was verified on this platform before the ordering below was
/// built on it.
pub(crate) fn sync(file: &File, operation: FsOperation) -> ArtifactResult<()> {
    file.sync_all()
        .map_err(|error| ArtifactError::io(operation, error))
}

/// Reports whether a name exists, without following a final symlink.
pub(crate) fn exists_nofollow(path: &Path) -> ArtifactResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ArtifactError::io(FsOperation::Stat, error)),
    }
}

fn classify_open(label: &str, operation: FsOperation, error: io::Error) -> ArtifactError {
    // `O_NOFOLLOW` reports a symlinked final component as `ELOOP`, which is
    // the signal that somebody planted a link where a key should be.
    if error.raw_os_error() == Some(libc::ELOOP) {
        return unsafe_path(label, UnsafePathReason::Symlink);
    }
    if error.kind() == io::ErrorKind::NotFound {
        return ArtifactError::Missing {
            key: label.to_owned(),
        };
    }
    ArtifactError::io(operation, error)
}

fn unsafe_path(label: &str, reason: UnsafePathReason) -> ArtifactError {
    ArtifactError::UnsafePath {
        key: label.to_owned(),
        reason,
    }
}
