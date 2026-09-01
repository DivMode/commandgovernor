//! Why the daemon refused to serve.
//!
//! `docs/architecture.md` "Startup recovery order" ends with *missing evidence
//! never becomes success*. Every variant here is therefore a refusal, not a
//! warning: reaching one means the daemon did not become ready, and nothing
//! external was scheduled.
//!
//! The messages carry classes, counters and opaque identities. They do not
//! carry filesystem paths, because a refusal is written to the log surface the
//! SEC-001 sweep scans (`docs/threat-model.md`, "Threat: diagnostics become
//! exfiltration"). The one place a path is shown is the command line's own
//! output, where it is the argument the user just supplied.

use governor_core::lease::{IncarnationMismatch, ProcessSlot};

use crate::layout::PathClass;

/// The daemon's typed refusal.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DaemonError {
    /// Another process holds the state root's instance lock.
    ///
    /// `docs/testing.md` DB-005: the second instance fails closed. It does not
    /// wait, retry, or degrade into a partial authority.
    #[error("another daemon already holds authority over this state root (process {slot})")]
    AuthorityHeld {
        /// Process number recorded by the holder, for the operator.
        slot: ProcessSlot,
    },

    /// The recorded lock holder is still running but does not hold the lock.
    ///
    /// Genuinely ambiguous — a holder mid-shutdown looks the same as a holder
    /// that lost its lock — so it fails closed rather than reclaiming.
    #[error("the recorded lock holder (process {slot}) is still running; refusing to reclaim")]
    LockHolderStillAlive {
        /// Process number recorded by the holder.
        slot: ProcessSlot,
    },

    /// The instance lock could not be created, read, or written.
    #[error("the state root's instance lock could not be acquired: {reason}")]
    Lock {
        /// What went wrong, as an operation class.
        reason: LockDefect,
    },

    /// A state-root directory failed its ownership or permission check.
    #[error("the {class} directory failed validation: {defect}")]
    Filesystem {
        /// Which directory.
        class: PathClass,
        /// What is wrong with it.
        defect: PathDefect,
    },

    /// The durable authority refused to open.
    #[error("the durable store refused to open: {0}")]
    Store(#[from] governor_store_sqlite::StoreError),

    /// The artifact root refused to open, or a sweep failed.
    #[error("the result-artifact root refused: {0}")]
    Artifacts(#[from] governor_artifacts::ArtifactError),

    /// The owner-local socket could not be prepared.
    #[error("the owner-local control socket could not be prepared: {0}")]
    Ipc(#[from] crate::ipc::IpcError),

    /// A log file under the state root could not be opened.
    #[error("the diagnostics log could not be opened")]
    Logging,

    /// The shutdown signal handlers could not be installed.
    ///
    /// Refused rather than continued: a daemon that cannot be stopped cleanly
    /// would have to be killed, and a killed daemon leaves its lock record
    /// saying `held`.
    #[error("the shutdown signal handlers could not be installed")]
    SignalHandler,
}

/// What went wrong while acquiring the instance lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LockDefect {
    /// The lock file could not be created or opened.
    #[error("the lock file could not be opened")]
    Unopenable,
    /// The lock file's contents are not a lock record this binary understands.
    #[error("the lock file does not hold a readable lock record")]
    Unreadable,
    /// The lock record could not be written or flushed.
    #[error("the lock record could not be made durable")]
    Unwritable,
    /// Repeated reclaim attempts kept losing the race to create the file.
    #[error("the lock was contended by another starting process")]
    Contended,
}

/// What is wrong with a state-root directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PathDefect {
    /// The path does not exist and could not be created.
    #[error("it does not exist and could not be created")]
    Uncreatable,
    /// The path exists but is not a directory.
    #[error("it exists but is not a directory")]
    NotADirectory,
    /// The path is a symbolic link.
    ///
    /// Refused rather than followed: a link is a redirection an attacker with
    /// the parent directory's write bit can install (`docs/testing.md`
    /// SEC-008).
    #[error("it is a symbolic link")]
    Symlink,
    /// The directory belongs to another OS principal.
    #[error("it is owned by another user")]
    ForeignOwner,
    /// The directory grants access to group or other.
    #[error("it is readable, writable, or traversable by group or other")]
    GroupOrOtherAccessible,
    /// The directory's metadata could not be read.
    #[error("its metadata could not be read")]
    Unreadable,
}

/// A stale instance lock was reclaimed, and how the holder differed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReclaimedLock {
    /// Process number the dead holder recorded.
    pub slot: ProcessSlot,
    /// How the holder's incarnation differs from anything running now.
    ///
    /// `None` when the recorded process number no longer resolves at all.
    pub mismatch: Option<IncarnationMismatch>,
}

impl DaemonError {
    /// Stable `snake_case` code, for the command line's exit classification.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AuthorityHeld { .. } => "authority_held",
            Self::LockHolderStillAlive { .. } => "lock_holder_still_alive",
            Self::Lock { .. } => "lock_unavailable",
            Self::Filesystem { .. } => "state_root_invalid",
            Self::Store(_) => "store_refused",
            Self::Artifacts(_) => "artifact_root_refused",
            Self::Ipc(_) => "ipc_unavailable",
            Self::Logging => "logging_unavailable",
            Self::SignalHandler => "signal_handler_unavailable",
        }
    }
}
