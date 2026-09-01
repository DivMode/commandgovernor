//! The daemon-owned private artifact root, and its fixed layout.
//!
//! ```text
//! <root>/                 0700   opened O_DIRECTORY|O_NOFOLLOW and verified
//!   objects/              0700   immutable published artifacts, one per key
//!   incoming/             0700   exclusive staging files, pre-publication
//!   quarantine/           0700   orphans a sweep set aside; never auto-deleted
//! ```
//!
//! Three directories rather than one, because the three populations have
//! genuinely different rules and a sweep must not confuse them:
//!
//! - a name in `objects/` with no committed database row is the legitimate
//!   crash residue `docs/data-model.md` describes, and needs a grace period
//!   before anyone touches it;
//! - a name in `incoming/` was never published and is garbage the moment its
//!   process is gone;
//! - a name in `quarantine/` is evidence. Nothing in this crate deletes it.
//!
//! Publication moves a name from `incoming/` to `objects/`, so the durability
//! barrier is one `fsync` of the `objects/` handle this type holds open.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::ArtifactResult;
use crate::fs_secure;
use crate::key::StorageKey;

/// Directory holding published, immutable artifacts.
///
/// Public together with its two siblings so a read-only diagnosis can name
/// the layout without opening — and therefore repairing — the root.
pub const OBJECTS_DIR: &str = "objects";
/// Directory holding pre-publication staging files.
pub const INCOMING_DIR: &str = "incoming";
/// Directory holding set-aside orphans.
pub const QUARANTINE_DIR: &str = "quarantine";

/// A verified, owner-only artifact root.
#[derive(Debug)]
pub struct ArtifactRoot {
    root: PathBuf,
    objects: PathBuf,
    incoming: PathBuf,
    quarantine: PathBuf,
    /// Held open so the durability barrier does not have to re-resolve a path
    /// at the one moment correctness depends on it.
    objects_handle: File,
}

impl ArtifactRoot {
    /// Creates or adopts the root, repairing every directory to owner-only.
    ///
    /// Adopting an existing root is the normal case after a restart. It is
    /// repaired rather than refused, because a directory left too open by an
    /// earlier build or a laxer umask is exactly the condition this wants to
    /// fix, and refusing to start would strand the artifacts inside it.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::UnsafePath`] when a layout component is a
    /// symlink or not a directory, or when owner-only cannot be established,
    /// and [`ArtifactError::Io`] for any other filesystem failure.
    ///
    /// [`ArtifactError::UnsafePath`]: crate::ArtifactError::UnsafePath
    /// [`ArtifactError::Io`]: crate::ArtifactError::Io
    pub fn open(root: impl Into<PathBuf>) -> ArtifactResult<Self> {
        let root = root.into();
        drop(fs_secure::owner_only_dir(&root, "root")?);

        let objects = root.join(OBJECTS_DIR);
        let incoming = root.join(INCOMING_DIR);
        let quarantine = root.join(QUARANTINE_DIR);
        let objects_handle = fs_secure::owner_only_dir(&objects, OBJECTS_DIR)?;
        drop(fs_secure::owner_only_dir(&incoming, INCOMING_DIR)?);
        drop(fs_secure::owner_only_dir(&quarantine, QUARANTINE_DIR)?);

        Ok(Self {
            root,
            objects,
            incoming,
            quarantine,
            objects_handle,
        })
    }

    /// The root directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The published-artifact directory.
    #[must_use]
    pub fn objects_path(&self) -> &Path {
        &self.objects
    }

    /// The staging directory.
    #[must_use]
    pub fn incoming_path(&self) -> &Path {
        &self.incoming
    }

    /// The quarantine directory.
    #[must_use]
    pub fn quarantine_path(&self) -> &Path {
        &self.quarantine
    }

    /// The open handle the directory barrier is taken on.
    pub(crate) const fn objects_handle(&self) -> &File {
        &self.objects_handle
    }

    /// Where a key's bytes live.
    ///
    /// A [`StorageKey`] is a validated single component, so this can only ever
    /// name a direct child of `objects/`.
    pub(crate) fn object_path(&self, key: &StorageKey) -> PathBuf {
        self.objects.join(key.as_str())
    }

    /// Where a staging name lives.
    pub(crate) fn staging_path(&self, name: &str) -> PathBuf {
        self.incoming.join(name)
    }

    /// Where a set-aside name lives.
    pub(crate) fn quarantine_target(&self, name: &str) -> PathBuf {
        self.quarantine.join(name)
    }
}
