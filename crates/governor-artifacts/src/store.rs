//! Crash-safe publication, and integrity-checked reads.
//!
//! # The ordering, and why the proof is the last step
//!
//! `docs/data-model.md` "Crash-safe result publication" fixes the sequence, and
//! [`ArtifactStore::publish`] *is* that sequence:
//!
//! | Step | Syscall | Failpoint after it |
//! | --- | --- | --- |
//! | bound the bytes | — | [`BeforeTempCreate`] |
//! | create owner-only staging file | `open(O_CREAT\|O_EXCL\|O_WRONLY\|O_NOFOLLOW, 0600)` + `fchmod(0600)` | [`AfterTempCreate`] |
//! | write the bounded final result | `write` | [`AfterWrite`] |
//! | make the bytes durable | `fsync` (`F_FULLFSYNC` on Apple) | [`AfterFileSync`] |
//! | publish the immutable name atomically | `link` + `unlink` | [`AfterPublishRename`] |
//! | make the name durable | `fsync` on the `objects/` handle | [`AfterDirSync`] |
//! | hand over the proof | — | [`BeforeProofHandoff`] |
//!
//! [`PublishedArtifact`] is constructed at exactly one place in this crate: the
//! line after the last row of that table. It has no public constructor and no
//! public fields, so a caller cannot build one, and
//! [`DurableArtifact`] — the value
//! [`Store::publish_worker_result`](governor_store_sqlite::Store::publish_worker_result)
//! needs before it will insert artifact metadata and open a
//! `completed_unprocessed` obligation — is obtained from it. The forbidden
//! outcome, a committed open obligation pointing at an artifact that was never
//! made durable, is therefore not something the artifact layer can produce.
//!
//! A crash anywhere in the table leaves at worst an unreferenced file, which
//! [`ArtifactStore::scan_orphans`] sets aside after a grace period.
//!
//! # Deviation: `link` + `unlink` rather than `rename`
//!
//! The document says "atomically rename to immutable store key". This uses
//! `link(2)` followed by `unlink(2)` of the staging name, which is the same
//! atomic name publication with one difference that matters here: `rename(2)`
//! **silently replaces** an existing destination — verified on this platform —
//! so it cannot express "an artifact is immutable and there is no overwrite
//! path", while `link(2)` fails with `EEXIST` and becomes
//! [`ArtifactError::AlreadyPublished`]. Every durability property the document
//! asks for is unchanged: the bytes are `fsync`ed before the immutable name
//! exists, and the directory is `fsync`ed before the proof is minted.
//!
//! A crash between the `link` and the `unlink` leaves the same inode under two
//! names. The published one is correct and durable; the staging one is swept.
//!
//! [`BeforeTempCreate`]: ArtifactFailpoint::BeforeTempCreate
//! [`AfterTempCreate`]: ArtifactFailpoint::AfterTempCreate
//! [`AfterWrite`]: ArtifactFailpoint::AfterWrite
//! [`AfterFileSync`]: ArtifactFailpoint::AfterFileSync
//! [`AfterPublishRename`]: ArtifactFailpoint::AfterPublishRename
//! [`AfterDirSync`]: ArtifactFailpoint::AfterDirSync
//! [`BeforeProofHandoff`]: ArtifactFailpoint::BeforeProofHandoff

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use governor_core::artifact::{ArtifactDigest, ArtifactIntegrityError, ResultArtifact};
use governor_core::fence::SafeToken;
use governor_core::time::DurationMs;
use governor_store_sqlite::DurableArtifact;
use sha2::{Digest as _, Sha256};

use crate::error::{ArtifactError, ArtifactResult, FsOperation};
use crate::failpoint::{ArtifactFailpoint, ArtifactFailpointHook, PUBLISH_OP};
use crate::fs_secure;
use crate::key::{StorageKey, StorageKeySource};
use crate::root::ArtifactRoot;

/// Default bound on one stored result, in bytes.
///
/// A *bounded final result required for review*, not a transcript and not a
/// provider stream: `docs/data-model.md` "Managed-run filesystem staging"
/// forbids the latter outright. One mebibyte is far more prose than a final
/// assistant result needs and still small enough that the whole artifact is
/// comfortably an in-memory value, which is what keeps the streaming shape
/// unrepresentable in this API.
pub const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 1 << 20;

/// Default grace before an unreferenced file may be quarantined.
///
/// Long enough that a slow publication followed by a slow commit is never
/// mistaken for a crash.
pub const DEFAULT_ORPHAN_GRACE: DurationMs = DurationMs::from_millis(15 * 60 * 1_000);

/// Default delay between an artifact being released and becoming deletable.
///
/// `docs/data-model.md`: "ACK only makes an artifact retention-eligible;
/// asynchronous GC deletes later."
///
/// This is where the policy is *defined*; it is not where it is *applied*. The
/// composition root hands it to the fenced ACK, which stamps the resulting
/// instant on the row, and the sweep then obeys the stamp. See
/// [`ArtifactConfig::retention_grace`].
pub const DEFAULT_RETENTION_GRACE: DurationMs = DurationMs::from_millis(24 * 60 * 60 * 1_000);

/// Policy knobs for one artifact root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactConfig {
    /// Largest result this root will store.
    pub max_bytes: u64,
    /// How long an unreferenced file is left alone before quarantine.
    pub orphan_grace: DurationMs,
    /// How long a released artifact is kept before it may be deleted.
    ///
    /// Read by the composition root and passed to the fenced ACK
    /// (`AcknowledgeRequest::retention_grace`), which records the resulting
    /// instant durably. [`ArtifactStore::collect`] deliberately does **not**
    /// consult it: the sweep obeys the recorded instant so that changing this
    /// knob cannot retroactively move the deletion time of work already
    /// closed.
    pub retention_grace: DurationMs,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
            orphan_grace: DEFAULT_ORPHAN_GRACE,
            retention_grace: DEFAULT_RETENTION_GRACE,
        }
    }
}

/// The bounded final result to publish.
///
/// `bytes` is a slice, not a reader. That is deliberate and structural: this
/// store holds one bounded final result that already exists in memory, and
/// there is no shape here that a provider stream could be spooled through
/// (`docs/testing.md` ART-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRequest<'a> {
    /// The complete final worker result required for review.
    pub bytes: &'a [u8],
    /// Opaque media-type label recorded alongside the metadata.
    pub media_type: SafeToken,
}

/// Proof that one artifact's bytes and name are both durable.
///
/// # Why the fields are private
///
/// This value is the artifact layer's half of "file before database". It is
/// returned by [`ArtifactStore::publish`] and by nothing else, on the far side
/// of the directory `fsync`. Private fields and no public constructor mean a
/// caller cannot manufacture the claim it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedArtifact {
    key: StorageKey,
    digest: ArtifactDigest,
    byte_len: u64,
    media_type: SafeToken,
}

impl PublishedArtifact {
    /// The opaque key the bytes were published under.
    #[must_use]
    pub const fn key(&self) -> &StorageKey {
        &self.key
    }

    /// Digest of the bytes as written.
    #[must_use]
    pub const fn digest(&self) -> ArtifactDigest {
        self.digest
    }

    /// Length of the bytes as written.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Opaque media-type label.
    #[must_use]
    pub const fn media_type(&self) -> &SafeToken {
        &self.media_type
    }

    /// The value the SQLite publication transaction requires.
    ///
    /// The only bridge between this crate and
    /// [`Store::publish_worker_result`](governor_store_sqlite::Store::publish_worker_result),
    /// and it exists only on a value that could only have come from a
    /// completed publication.
    ///
    /// This is the sanctioned caller of
    /// [`DurableArtifact::assert_durable_from_parts`]: the assertion that
    /// method demands — temp → write → `fsync` → link → directory `fsync` →
    /// verify — is exactly the sequence [`ArtifactStore::publish`] performed
    /// before it produced `self`.
    #[must_use]
    pub fn durable(&self) -> DurableArtifact {
        DurableArtifact::assert_durable_from_parts(
            self.key.as_token().clone(),
            self.digest,
            self.byte_len,
            self.media_type.clone(),
        )
    }
}

impl From<&PublishedArtifact> for DurableArtifact {
    fn from(published: &PublishedArtifact) -> Self {
        published.durable()
    }
}

/// Everything needed to bring up one artifact root.
///
/// Shaped like [`OpenStore`](governor_store_sqlite::OpenStore) on purpose: the
/// daemon composes both, and a matching shape is one less thing to remember.
pub struct OpenArtifactStore {
    /// Directory the daemon owns for artifacts. Created if absent.
    pub root: PathBuf,
    /// Bounds and retention policy.
    pub config: ArtifactConfig,
    /// Where opaque keys come from. This crate ships no default; see
    /// [`StorageKeySource`].
    pub keys: Box<dyn StorageKeySource>,
    /// Crash seam. `None` in production, where every point is inert.
    pub failpoints: Option<Box<dyn ArtifactFailpointHook>>,
}

impl OpenArtifactStore {
    /// Verifies and repairs the layout, then hands back a usable store.
    ///
    /// # Errors
    ///
    /// Returns whatever [`ArtifactRoot::open`] refused on.
    pub fn start(self) -> ArtifactResult<ArtifactStore> {
        Ok(ArtifactStore {
            root: ArtifactRoot::open(self.root)?,
            config: self.config,
            keys: self.keys,
            failpoints: self.failpoints,
            staging_seq: 0,
        })
    }
}

/// The private immutable result-artifact store.
pub struct ArtifactStore {
    root: ArtifactRoot,
    config: ArtifactConfig,
    keys: Box<dyn StorageKeySource>,
    failpoints: Option<Box<dyn ArtifactFailpointHook>>,
    staging_seq: u64,
}

impl core::fmt::Debug for ArtifactStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ArtifactStore")
            .field("root", &self.root.path())
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ArtifactStore {
    /// The verified root.
    #[must_use]
    pub const fn root(&self) -> &ArtifactRoot {
        &self.root
    }

    /// Policy in force.
    #[must_use]
    pub const fn config(&self) -> &ArtifactConfig {
        &self.config
    }

    /// Publishes one bounded final result, durably and immutably.
    ///
    /// Returns only after the bytes are `fsync`ed, the immutable name exists,
    /// and the containing directory is `fsync`ed. See the module documentation
    /// for the exact ordering and the failpoint after each step.
    ///
    /// # Errors
    ///
    /// - [`ArtifactError::TooLarge`] before any file exists, when the result
    ///   exceeds [`ArtifactConfig::max_bytes`];
    /// - [`ArtifactError::AlreadyPublished`] when the allocated key is taken —
    ///   artifacts are immutable and this call never overwrites;
    /// - [`ArtifactError::UnsafePath`] when the published name is not the plain
    ///   single-linked regular file it must be;
    /// - [`ArtifactError::Io`] for a filesystem failure at a named step;
    /// - [`ArtifactError::Injected`] when a test hook fired.
    ///
    /// On every error path the caller receives no [`PublishedArtifact`], so no
    /// database transaction can follow. Any bytes left behind are an
    /// unreferenced orphan, which is the safe direction.
    pub fn publish(&mut self, request: PublishRequest<'_>) -> ArtifactResult<PublishedArtifact> {
        let byte_len = u64::try_from(request.bytes.len()).unwrap_or(u64::MAX);
        if byte_len > self.config.max_bytes {
            return Err(ArtifactError::TooLarge {
                limit: self.config.max_bytes,
                actual: byte_len,
            });
        }
        self.reach(ArtifactFailpoint::BeforeTempCreate)?;

        let key = self.keys.next_key();
        let staging_name = self.next_staging_name(&key);
        let staging_path = self.root.staging_path(&staging_name);
        let object_path = self.root.object_path(&key);

        let mut staging = fs_secure::create_owner_only_file(&staging_path)?;
        self.reach(ArtifactFailpoint::AfterTempCreate)?;

        staging
            .write_all(request.bytes)
            .map_err(|error| ArtifactError::io(FsOperation::Write, error))?;
        let digest = ArtifactDigest::from_bytes(Sha256::digest(request.bytes).into());
        self.reach(ArtifactFailpoint::AfterWrite)?;

        fs_secure::sync(&staging, FsOperation::SyncFile)?;
        drop(staging);
        self.reach(ArtifactFailpoint::AfterFileSync)?;

        // Atomic, exclusive publication of the immutable name. `EEXIST` here is
        // an immutability violation, never an overwrite.
        fs::hard_link(&staging_path, &object_path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                ArtifactError::AlreadyPublished {
                    key: key.to_string(),
                }
            } else {
                ArtifactError::io(FsOperation::PublishName, error)
            }
        })?;
        // The staging name is now redundant. If removing it fails the artifact
        // is still correctly published, and the leftover is swept as an orphan,
        // so this must not fail the publication.
        drop(fs::remove_file(&staging_path));
        self.reach(ArtifactFailpoint::AfterPublishRename)?;

        fs_secure::sync(self.root.objects_handle(), FsOperation::SyncDirectory)?;
        self.reach(ArtifactFailpoint::AfterDirSync)?;

        // Post-condition: read the published name back and verify it against
        // the digest computed on the way in.
        //
        // The proof this function returns is what authorises the database to
        // record a completed obligation, so it should rest on the bytes that
        // are *there*, not on the bytes that were handed in. This closes a
        // short write the kernel accepted, a device error `fsync` did not
        // surface, and the plain-regular-file and single-link checks in one
        // pass. It costs one bounded re-read, which is affordable precisely
        // because the store holds a bounded final result.
        //
        // Failing here leaves a durable but unreferenced file, which the sweep
        // handles — the safe direction.
        let stored = self.read_verified(&key, digest, byte_len)?;
        debug_assert_eq!(stored.len(), request.bytes.len());
        drop(stored);

        self.reach(ArtifactFailpoint::BeforeProofHandoff)?;
        Ok(PublishedArtifact {
            key,
            digest,
            byte_len,
            media_type: request.media_type,
        })
    }

    /// Reads one artifact, verifying it against its recorded metadata.
    ///
    /// The metadata comes from the SQLite row, so `storage_ref` is re-validated
    /// as a [`StorageKey`] before it is joined to anything: a tampered row must
    /// not become a path.
    ///
    /// # Errors
    ///
    /// [`ArtifactError::Integrity`] on any digest or length mismatch, and the
    /// caller receives **no bytes at all** — a corrupt or truncated result must
    /// never reach review, and the obligation stays open
    /// (`docs/testing.md` ART-003). Also [`ArtifactError::Missing`],
    /// [`ArtifactError::UnsafePath`], [`ArtifactError::InvalidKey`],
    /// [`ArtifactError::TooLarge`] and [`ArtifactError::Io`].
    pub fn read(&self, artifact: &ResultArtifact) -> ArtifactResult<Vec<u8>> {
        let key = StorageKey::new(artifact.storage_ref().clone())?;
        self.read_verified(&key, artifact.digest(), artifact.byte_len())
    }

    /// Reads and verifies the bytes at one key against an expected digest and
    /// length.
    ///
    /// # Errors
    ///
    /// As [`Self::read`].
    pub fn read_verified(
        &self,
        key: &StorageKey,
        expected_digest: ArtifactDigest,
        expected_len: u64,
    ) -> ArtifactResult<Vec<u8>> {
        // A row claiming a length this root would never have stored is itself
        // evidence of tampering, and reading it would be an unbounded
        // allocation driven by that row.
        if expected_len > self.config.max_bytes {
            return Err(ArtifactError::TooLarge {
                limit: self.config.max_bytes,
                actual: expected_len,
            });
        }
        let label = key.to_string();
        let file = fs_secure::open_stored_file(&self.root.object_path(key), &label)?;

        // One byte past the recorded length: enough to notice that the file
        // grew, never enough for a tampered file to drive the allocation.
        let mut bytes = Vec::new();
        file.take(expected_len.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| ArtifactError::io(FsOperation::Read, error))?;

        let observed_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let observed_digest = ArtifactDigest::from_bytes(Sha256::digest(&bytes).into());
        ArtifactIntegrityError::check(expected_digest, expected_len, observed_digest, observed_len)
            .map_err(|source| ArtifactError::Integrity { key: label, source })?;
        Ok(bytes)
    }

    /// Announces that publication reached a named point.
    fn reach(&self, point: ArtifactFailpoint) -> ArtifactResult<()> {
        match &self.failpoints {
            Some(hook) => hook.reached(PUBLISH_OP, point),
            None => Ok(()),
        }
    }

    /// A staging name that cannot collide with a published key.
    ///
    /// `.staging` contains a `.`, so the composed name is never a valid
    /// [`StorageKey`] and an orphan sweep can tell the two populations apart
    /// even if one is found in the wrong directory.
    fn next_staging_name(&mut self, key: &StorageKey) -> String {
        self.staging_seq = self.staging_seq.wrapping_add(1);
        let name = format!("{key}.{}.staging", self.staging_seq);
        debug_assert!(
            name.len() <= crate::key::MAX_STORAGE_KEY_LEN + crate::key::STAGING_SUFFIX_MAX_LEN,
            "the staging suffix budget must cover the composed name"
        );
        name
    }
}
