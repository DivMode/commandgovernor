//! Retention, and the two sweeps that act on it.
//!
//! # Pinning is derived, never asserted
//!
//! `docs/state-machines.md` invariant 2: *an open obligation pins its
//! artifact*. `governor-core` already derives that from the obligations
//! themselves ([`ResultArtifact::retention`]), and `governor-store-sqlite`
//! recomputes the `result_artifacts.retention_state` column from the open
//! obligations on every transition. This module consumes that answer; it never
//! forms its own opinion about whether work is still outstanding, because a
//! second opinion is a second source of truth.
//!
//! ACK is not deletion. A closing disposition makes an artifact *eligible*;
//! deletion happens later, after [`ArtifactConfig::retention_grace`], in a
//! sweep the daemon schedules (`docs/data-model.md`: "ACK only makes an
//! artifact retention-eligible; asynchronous GC deletes later").
//!
//! # Orphans are quarantined, not deleted
//!
//! A crash between the directory `fsync` and the SQLite commit legitimately
//! leaves a durable file with no row. Deleting unreferenced files on sight
//! would therefore race a publication that is merely slow, and the loser would
//! be a real result. So an unreferenced file is left alone for
//! [`ArtifactConfig::orphan_grace`] and then *moved to quarantine*, where it
//! stays. Nothing in this crate deletes a quarantined file.
//!
//! # Deviation: where the release instant comes from
//!
//! `docs/data-model.md` gives `result_artifacts` an
//! `eligible_for_delete_at_ms` column, and the schema has it, but no store
//! operation currently writes it — the store recomputes `retention_state` and
//! leaves the instant `NULL`. So [`RetentionInput::released_at`] is supplied by
//! the caller, from the closing event's timestamp, and an eligible artifact
//! with **no** release instant is refused
//! ([`RetentionDecision::ReleaseInstantUnknown`]) rather than deleted. Failing
//! closed here costs disk; guessing costs a result.

use std::collections::BTreeSet;
use std::fs;
use std::time::UNIX_EPOCH;

use governor_core::artifact::{ResultArtifact, RetentionState};
use governor_core::obligation::Obligation;
use governor_core::time::{DurationMs, Timestamp};

use crate::error::{ArtifactError, ArtifactResult, FsOperation};
use crate::key::StorageKey;
use crate::store::ArtifactStore;

/// One artifact's retention facts, as the durable authority reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionInput {
    /// Opaque key the bytes live under.
    pub key: StorageKey,
    /// Whether any open obligation still needs it.
    pub state: RetentionState,
    /// When the last obligation referencing it closed, if it has.
    pub released_at: Option<Timestamp>,
}

impl RetentionInput {
    /// Derives the input from the domain model itself.
    ///
    /// Pass every obligation in the projection: [`ResultArtifact::retention`]
    /// selects the ones that reference this artifact, so a caller cannot
    /// accidentally filter out the pinning one.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::InvalidKey`] when the recorded `storage_ref` is
    /// not a legal storage key. A row that cannot name a file is not a licence
    /// to guess at one.
    pub fn from_artifact<'a>(
        artifact: &ResultArtifact,
        obligations: impl IntoIterator<Item = &'a Obligation>,
        released_at: Option<Timestamp>,
    ) -> ArtifactResult<Self> {
        Ok(Self {
            key: StorageKey::new(artifact.storage_ref().clone())?,
            state: artifact.retention(obligations),
            released_at,
        })
    }
}

/// What a garbage collector may do with one artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetentionDecision {
    /// An open obligation needs it. Deleting it would destroy work in flight.
    Pinned,
    /// Released, but the release instant is unknown, so the grace period
    /// cannot be evaluated. Fails closed: keep it.
    ReleaseInstantUnknown,
    /// Released, and still inside the retention grace period.
    WithinGrace {
        /// Earliest instant at which deletion becomes permitted.
        deletable_at: Timestamp,
    },
    /// Released, and past the grace period.
    Deletable,
}

impl RetentionDecision {
    /// Whether a sweep may delete the bytes.
    #[must_use]
    pub const fn permits_deletion(self) -> bool {
        matches!(self, Self::Deletable)
    }
}

/// Decides one artifact's fate. Pure, and the whole retention policy.
#[must_use]
pub fn decide(input: &RetentionInput, now: Timestamp, grace: DurationMs) -> RetentionDecision {
    match (input.state, input.released_at) {
        (RetentionState::Pinned, _) => RetentionDecision::Pinned,
        (RetentionState::Eligible, None) => RetentionDecision::ReleaseInstantUnknown,
        (RetentionState::Eligible, Some(released_at)) => {
            let deletable_at = released_at.saturating_add(grace);
            if now >= deletable_at {
                RetentionDecision::Deletable
            } else {
                RetentionDecision::WithinGrace { deletable_at }
            }
        }
    }
}

/// What a retention sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionReport {
    /// Artifacts whose bytes were deleted.
    pub deleted: Vec<StorageKey>,
    /// Artifacts left in place, and why.
    pub kept: Vec<(StorageKey, RetentionDecision)>,
}

/// One file the orphan sweep set aside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantined {
    /// Name it had inside the root.
    pub name: String,
    /// Name it now has inside `quarantine/`.
    pub quarantine_name: String,
    /// Why it was set aside.
    pub reason: OrphanReason,
}

/// Why a file in the root had no business being there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OrphanReason {
    /// A published name with no committed database row: the crash residue
    /// `docs/data-model.md` predicts between the directory sync and the
    /// commit.
    UnreferencedObject,
    /// A staging file whose publication never completed.
    AbandonedStaging,
    /// A name in the root that is not a legal storage key at all, so no row
    /// could ever have referenced it.
    UnrecognisedName,
}

/// What an orphan sweep found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrphanScan {
    /// Files moved into `quarantine/`.
    pub quarantined: Vec<Quarantined>,
    /// Unreferenced files still inside the grace period, left alone.
    pub within_grace: Vec<String>,
    /// Published names a committed row referenced.
    pub referenced: usize,
}

impl ArtifactStore {
    /// Deletes the artifacts whose retention has genuinely expired.
    ///
    /// Every input is decided by [`decide`] first, so a pinned artifact is
    /// never reached: `docs/testing.md` ART-002 requires that garbage
    /// collection attempted before ACK, during a claim, after a physical
    /// ChatGPT settlement, or after claim expiry leaves the artifact alone.
    /// None of those closes an obligation, so all four report
    /// [`RetentionDecision::Pinned`].
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::Io`] if a deletion fails. A file that is
    /// already gone is not an error: the sweep is idempotent.
    pub fn collect(
        &self,
        inputs: &[RetentionInput],
        now: Timestamp,
    ) -> ArtifactResult<CollectionReport> {
        let mut report = CollectionReport::default();
        for input in inputs {
            let decision = decide(input, now, self.config().retention_grace);
            if !decision.permits_deletion() {
                report.kept.push((input.key.clone(), decision));
                continue;
            }
            match fs::remove_file(self.root().object_path(&input.key)) {
                Ok(()) => report.deleted.push(input.key.clone()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    report.deleted.push(input.key.clone());
                }
                Err(error) => return Err(ArtifactError::io(FsOperation::Delete, error)),
            }
        }
        Ok(report)
    }

    /// Quarantines files the durable authority does not know about.
    ///
    /// `committed` is every `storage_ref` with a committed `result_artifacts`
    /// row. Anything else in `objects/`, and anything at all left in
    /// `incoming/`, is unreferenced; once older than
    /// [`ArtifactConfig::orphan_grace`](crate::ArtifactConfig::orphan_grace)
    /// it is moved into `quarantine/`.
    ///
    /// Nothing is deleted, here or ever: a quarantined file is evidence about
    /// a crash, and the daemon decides what to do with it.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::Io`] when a directory cannot be listed or a
    /// file cannot be moved.
    pub fn scan_orphans(
        &self,
        committed: &BTreeSet<StorageKey>,
        now: Timestamp,
    ) -> ArtifactResult<OrphanScan> {
        let mut scan = OrphanScan::default();
        let grace = self.config().orphan_grace;

        for (name, age) in list(self.root().objects_path(), now)? {
            let reason = match StorageKey::parse(&name) {
                Ok(key) if committed.contains(&key) => {
                    scan.referenced += 1;
                    continue;
                }
                Ok(_) => OrphanReason::UnreferencedObject,
                Err(_) => OrphanReason::UnrecognisedName,
            };
            self.set_aside(
                self.root().objects_path().join(&name),
                name,
                age,
                grace,
                reason,
                &mut scan,
            )?;
        }

        for (name, age) in list(self.root().incoming_path(), now)? {
            self.set_aside(
                self.root().incoming_path().join(&name),
                name,
                age,
                grace,
                OrphanReason::AbandonedStaging,
                &mut scan,
            )?;
        }

        Ok(scan)
    }

    fn set_aside(
        &self,
        from: std::path::PathBuf,
        name: String,
        age: DurationMs,
        grace: DurationMs,
        reason: OrphanReason,
        scan: &mut OrphanScan,
    ) -> ArtifactResult<()> {
        if age < grace {
            scan.within_grace.push(name);
            return Ok(());
        }
        let quarantine_name = self.free_quarantine_name(&name)?;
        fs::rename(from, self.root().quarantine_target(&quarantine_name))
            .map_err(|error| ArtifactError::io(FsOperation::Quarantine, error))?;
        scan.quarantined.push(Quarantined {
            name,
            quarantine_name,
            reason,
        });
        Ok(())
    }

    /// Finds an unused name in `quarantine/`.
    ///
    /// Quarantine must never overwrite: two orphans with the same name are two
    /// pieces of evidence, and `rename(2)` would silently destroy the first.
    fn free_quarantine_name(&self, name: &str) -> ArtifactResult<String> {
        if !crate::fs_secure::exists_nofollow(&self.root().quarantine_target(name))? {
            return Ok(name.to_owned());
        }
        for attempt in 1..u32::MAX {
            let candidate = format!("{name}.{attempt}");
            if !crate::fs_secure::exists_nofollow(&self.root().quarantine_target(&candidate))? {
                return Ok(candidate);
            }
        }
        Err(ArtifactError::io(
            FsOperation::Quarantine,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "no free quarantine name remains",
            ),
        ))
    }
}

/// Lists a directory as `(name, age)` pairs, skipping anything unnameable.
///
/// A non-UTF-8 entry cannot be a storage key and cannot be reported without
/// putting raw bytes in a log, so it is left in place and counted by neither
/// population. Directories are skipped for the same reason: this layout has no
/// nested directories, so one is a foreign object rather than an orphan.
fn list(dir: &std::path::Path, now: Timestamp) -> ArtifactResult<Vec<(String, DurationMs)>> {
    let entries =
        fs::read_dir(dir).map_err(|error| ArtifactError::io(FsOperation::ReadDirectory, error))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| ArtifactError::io(FsOperation::ReadDirectory, error))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let metadata = entry
            .metadata()
            .map_err(|error| ArtifactError::io(FsOperation::Stat, error))?;
        if metadata.is_dir() {
            continue;
        }
        out.push((name, age_of(&metadata, now)));
    }
    out.sort();
    Ok(out)
}

/// How long ago a file was last modified, against the daemon's injected clock.
///
/// The clock is a parameter rather than a call to
/// [`SystemTime::now`](std::time::SystemTime::now) for the same reason it is
/// everywhere else in this workspace: a sweep whose behaviour depends on how
/// fast the machine ran cannot be tested.
///
/// A file whose mtime is in the future — a clock that jumped, or a deliberate
/// `utimes` — reads as age zero and is therefore *kept*, not quarantined. That
/// is the fail-closed direction.
fn age_of(metadata: &fs::Metadata, now: Timestamp) -> DurationMs {
    let Ok(modified) = metadata.modified() else {
        return DurationMs::ZERO;
    };
    let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH) else {
        return DurationMs::ZERO;
    };
    let millis = i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX);
    now.saturating_elapsed_since(Timestamp::from_unix_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(state: RetentionState, released_at: Option<i64>) -> RetentionInput {
        RetentionInput {
            key: StorageKey::parse("ra-0001").expect("valid key"),
            state,
            released_at: released_at.map(Timestamp::from_unix_millis),
        }
    }

    const GRACE: DurationMs = DurationMs::from_millis(1_000);

    #[test]
    fn an_open_obligation_keeps_the_bytes_whatever_the_clock_says() {
        let decision = decide(
            &input(RetentionState::Pinned, Some(0)),
            Timestamp::from_unix_millis(i64::MAX),
            GRACE,
        );
        assert_eq!(decision, RetentionDecision::Pinned);
        assert!(!decision.permits_deletion());
    }

    #[test]
    fn release_starts_a_grace_period_rather_than_a_deletion() {
        let decision = decide(
            &input(RetentionState::Eligible, Some(1_000)),
            Timestamp::from_unix_millis(1_500),
            GRACE,
        );
        assert_eq!(
            decision,
            RetentionDecision::WithinGrace {
                deletable_at: Timestamp::from_unix_millis(2_000)
            }
        );
        assert!(!decision.permits_deletion());
    }

    #[test]
    fn only_a_release_plus_the_full_delay_permits_deletion() {
        assert!(
            decide(
                &input(RetentionState::Eligible, Some(1_000)),
                Timestamp::from_unix_millis(2_000),
                GRACE,
            )
            .permits_deletion()
        );
    }

    #[test]
    fn an_unknown_release_instant_fails_closed() {
        let decision = decide(
            &input(RetentionState::Eligible, None),
            Timestamp::from_unix_millis(i64::MAX),
            GRACE,
        );
        assert_eq!(decision, RetentionDecision::ReleaseInstantUnknown);
        assert!(!decision.permits_deletion());
    }
}
