//! The crash seam the artifact publication exposes.
//!
//! `docs/testing.md` ART-001 requires a crash injected *at each* candidate
//! validation, write, `fsync`, rename, directory-sync and database point, with
//! the same forbidden outcome each time: a committed `completed_unprocessed`
//! obligation pointing at an artifact that was never made durable.
//!
//! The database half of that matrix already has a seam —
//! [`governor_store_sqlite::Failpoint`]. This is the file half, and the two
//! compose: an artifact failpoint aborts before the store is ever called, a
//! store failpoint aborts after the bytes are already durable. Both leave the
//! same safe residue, an unreferenced orphan file.
//!
//! With no hook installed every point is a branch on `None`.

use core::fmt;

use crate::error::ArtifactResult;

/// A named point inside the publication ordering.
///
/// The variants are in execution order, and [`ArtifactFailpoint::ALL`] is that
/// order, so a matrix is `for point in ArtifactFailpoint::ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ArtifactFailpoint {
    /// Bounds have been checked; no file exists yet.
    ///
    /// Crashing here must leave the root exactly as it was.
    BeforeTempCreate,
    /// The owner-only staging file exists and is empty.
    AfterTempCreate,
    /// Every byte has been written; nothing is `fsync`ed.
    AfterWrite,
    /// The staging file is `fsync`ed; the immutable name does not exist.
    AfterFileSync,
    /// The immutable name now exists and the staging name is gone.
    ///
    /// The directory entry is not yet `fsync`ed, so the name may not survive a
    /// power loss. That is precisely why the proof is not minted yet.
    AfterPublishRename,
    /// The containing directory is `fsync`ed. The bytes and the name are
    /// durable, and the only thing still missing is the database row.
    ///
    /// Crashing here is the legitimate orphan window `docs/data-model.md`
    /// names: "a crash before step 7 may leave an unreferenced orphan file".
    AfterDirSync,
    /// Immediately before the [`PublishedArtifact`] proof is handed back.
    ///
    /// The last point at which the caller can still be denied the value that
    /// authorises the database transaction.
    ///
    /// [`PublishedArtifact`]: crate::PublishedArtifact
    BeforeProofHandoff,
}

impl ArtifactFailpoint {
    /// Every point, in execution order.
    pub const ALL: &'static [Self] = &[
        Self::BeforeTempCreate,
        Self::AfterTempCreate,
        Self::AfterWrite,
        Self::AfterFileSync,
        Self::AfterPublishRename,
        Self::AfterDirSync,
        Self::BeforeProofHandoff,
    ];

    /// Stable name, for test reports and failure messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeTempCreate => "before_temp_create",
            Self::AfterTempCreate => "after_temp_create",
            Self::AfterWrite => "after_write",
            Self::AfterFileSync => "after_file_sync",
            Self::AfterPublishRename => "after_publish_rename",
            Self::AfterDirSync => "after_dir_sync",
            Self::BeforeProofHandoff => "before_proof_handoff",
        }
    }

    /// Reports whether a crash at this point can leave bytes behind.
    ///
    /// The orphan sweep's whole justification: everything from
    /// [`Self::AfterTempCreate`] onwards may leave a file with no database row.
    #[must_use]
    pub const fn may_leave_bytes(self) -> bool {
        !matches!(self, Self::BeforeTempCreate)
    }
}

impl fmt::Display for ArtifactFailpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Injectable interruption for the artifact publication.
///
/// Mirrors [`governor_store_sqlite::FailpointHook`] deliberately: the testkit
/// drives one matrix across both halves, and two differently shaped seams
/// would make that awkward for no gain.
pub trait ArtifactFailpointHook: Send + Sync {
    /// Called when `op` reaches `point`.
    ///
    /// # Errors
    ///
    /// Returns the failure the hook wants injected. Returning `Ok(())`
    /// continues.
    fn reached(&self, op: &'static str, point: ArtifactFailpoint) -> ArtifactResult<()>;
}

/// Name of the only operation that currently reaches a failpoint.
pub(crate) const PUBLISH_OP: &str = "publish_artifact";
