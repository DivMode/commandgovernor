//! Typed, machine-classifiable artifact failures.
//!
//! Every variant is something a caller must be able to act on without parsing
//! a string, because the actions differ sharply:
//!
//! | Kind | What the daemon must do |
//! | --- | --- |
//! | [`ArtifactError::Integrity`] | fail closed: leave the obligation open, raise `result_artifact_missing`, never return partial bytes |
//! | [`ArtifactError::Missing`] | same, but the bytes are gone rather than wrong |
//! | [`ArtifactError::AlreadyPublished`] | a key was reused; an artifact is immutable, so this is a bug or an attack, never an overwrite |
//! | [`ArtifactError::UnsafePath`] | something in the root is not what it must be; refuse and report |
//! | [`ArtifactError::TooLarge`] | the bounded-result rule refused the bytes before any file existed |
//! | [`ArtifactError::Io`] | operational; the publication simply did not happen |
//! | [`ArtifactError::Injected`] | only reachable with a test failpoint hook installed |
//!
//! # Why no path appears in a message
//!
//! `SECURITY.md` "Sensitive data policy" forbids persisting a cwd or a
//! filesystem path in routine logs and diagnostics, and an error string is a
//! routine log line. So a failure names the *operation* and the opaque
//! [`StorageKey`](crate::StorageKey), never the absolute path it was working
//! on.

use governor_core::artifact::ArtifactIntegrityError;

use crate::failpoint::ArtifactFailpoint;
use crate::key::InvalidStorageKey;

/// Result alias for every fallible artifact operation.
pub type ArtifactResult<T> = Result<T, ArtifactError>;

/// A failure from the private result-artifact store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactError {
    /// A storage key was not a legal, root-relative, single-component name.
    ///
    /// This is the path-security refusal: traversal, an absolute path, a dot
    /// name, and a separator are all rejected here, before any file is opened
    /// (`docs/testing.md` ART-004).
    #[error("storage key rejected: {0}")]
    InvalidKey(#[from] InvalidStorageKey),

    /// The bytes exceeded the bounded-final-result limit.
    ///
    /// Refused before a temp file exists, so an oversized result never becomes
    /// a partial artifact. This store holds a *bounded final result*, never a
    /// provider stream spool (`docs/data-model.md`, "Managed-run filesystem
    /// staging").
    #[error("result is {actual} bytes, bounded limit is {limit}")]
    TooLarge {
        /// Configured maximum.
        limit: u64,
        /// Length that was offered.
        actual: u64,
    },

    /// The immutable key already exists. There is no overwrite path.
    #[error("storage key {key} is already published; artifacts are immutable")]
    AlreadyPublished {
        /// The opaque key that was already taken.
        key: String,
    },

    /// No file exists at the key an artifact row points at.
    #[error("no stored bytes for storage key {key}")]
    Missing {
        /// The opaque key that resolved to nothing.
        key: String,
    },

    /// Stored bytes did not match the recorded digest or length.
    ///
    /// The caller receives no bytes at all: a partial or corrupt result must
    /// never reach review (`docs/testing.md` ART-003).
    #[error("stored bytes for {key} failed integrity: {source}")]
    Integrity {
        /// The opaque key whose bytes are wrong.
        key: String,
        /// Which half of the check failed.
        #[source]
        source: ArtifactIntegrityError,
    },

    /// Something in the artifact root was not the shape it must be.
    #[error("unsafe path for {key}: {reason}")]
    UnsafePath {
        /// The opaque key, or the layout component, involved.
        key: String,
        /// What was wrong.
        reason: UnsafePathReason,
    },

    /// An operating-system failure during a named filesystem step.
    #[error("{operation} failed")]
    Io {
        /// Which step failed. Deliberately an enum: a caller classifies on
        /// this, and no path is carried.
        operation: FsOperation,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A test failpoint hook aborted the operation at a named point.
    ///
    /// Unreachable in production: with no hook installed every point is inert.
    #[error("injected failure at {op}/{point}")]
    Injected {
        /// Operation the hook fired in.
        op: &'static str,
        /// Point it fired at.
        point: ArtifactFailpoint,
    },
}

impl ArtifactError {
    /// Wraps an I/O failure with the step it came from.
    pub(crate) fn io(operation: FsOperation, source: std::io::Error) -> Self {
        Self::Io { operation, source }
    }

    /// Reports whether this failure means the stored bytes cannot be trusted.
    ///
    /// The daemon treats both the same way — leave the obligation open, raise
    /// [`HealthConditionKind::ResultArtifactMissing`] — so the distinction is
    /// worth making once, here, rather than at each call site.
    ///
    /// [`HealthConditionKind::ResultArtifactMissing`]: governor_core::health::HealthConditionKind::ResultArtifactMissing
    #[must_use]
    pub const fn is_artifact_unusable(&self) -> bool {
        matches!(
            self,
            Self::Missing { .. } | Self::Integrity { .. } | Self::UnsafePath { .. }
        )
    }
}

/// Why a path inside the artifact root was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UnsafePathReason {
    /// The name resolved to a symbolic link. Opens are `O_NOFOLLOW`, so this
    /// is what an attempted symlink swap looks like from inside.
    #[error("the name is a symbolic link")]
    Symlink,
    /// The name exists but is not a regular file.
    #[error("the name is not a regular file")]
    NotRegularFile,
    /// A layout component exists but is not a directory.
    #[error("the layout component is not a directory")]
    NotDirectory,
    /// The published inode has more than one name.
    ///
    /// Publication creates exactly one surviving link, so a second one means
    /// somebody else made it. Under the V1 trust model that somebody is the
    /// same OS user and could do worse, but the check is free and the digest
    /// behind it is the real defence.
    #[error("the stored inode has more than one link")]
    HardLinked,
    /// The root, or a directory inside it, is readable or writable by somebody
    /// other than the owner and could not be repaired.
    #[error("the directory is not owner-only")]
    NotOwnerOnly,
}

/// The filesystem step an [`ArtifactError::Io`] came from.
///
/// Naming the step rather than the path is what lets a diagnostic say *what*
/// broke without persisting *where*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FsOperation {
    /// Creating or repairing a layout directory.
    #[error("creating the artifact root layout")]
    PrepareLayout,
    /// Opening a layout directory with no-follow semantics.
    #[error("opening a layout directory")]
    OpenDirectory,
    /// Reading a directory's entries during a scan.
    #[error("listing a layout directory")]
    ReadDirectory,
    /// Enforcing owner-only permissions.
    #[error("setting owner-only permissions")]
    SetPermissions,
    /// Creating the exclusive staging file.
    #[error("creating the staging file")]
    CreateStaging,
    /// Writing the bounded result bytes.
    #[error("writing the result bytes")]
    Write,
    /// `fsync` of the staging file.
    #[error("syncing the staging file")]
    SyncFile,
    /// Publishing the immutable name.
    #[error("publishing the immutable name")]
    PublishName,
    /// Removing the staging name after publication.
    #[error("removing the staging name")]
    RemoveStaging,
    /// `fsync` of the containing directory.
    #[error("syncing the containing directory")]
    SyncDirectory,
    /// Opening a stored artifact for reading.
    #[error("opening the stored artifact")]
    OpenArtifact,
    /// Reading a stored artifact's bytes.
    #[error("reading the stored artifact")]
    Read,
    /// Inspecting a name without following it.
    #[error("inspecting a name")]
    Stat,
    /// Moving an orphan into quarantine.
    #[error("quarantining an orphan")]
    Quarantine,
    /// Deleting a retention-eligible artifact.
    #[error("deleting a released artifact")]
    Delete,
}
