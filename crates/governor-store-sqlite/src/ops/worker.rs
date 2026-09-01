//! Worker lifecycle transitions, and the terminal publication boundary.
//!
//! # Worker terminal publication
//!
//! `docs/data-model.md` "Critical transaction boundaries" requires *one* DB
//! transaction, **after the final result artifact is durable**, that
//!
//! 1. inserts or dedupes the terminal event;
//! 2. finalizes the `turns` projection;
//! 3. inserts the result-artifact metadata;
//! 4. transitions exactly one obligation to `completed_unprocessed`;
//! 5. appends the obligation transition.
//!
//! The file half of "file before database" is the artifact layer's, and it is
//! expressed in the type system here: this operation takes a
//! [`DurableArtifact`], which that layer produces only once the bytes are
//! `fsync`ed, renamed to their immutable key, and the containing directory is
//! synced. There is no other way to reach step 3.
//!
//! Duplicate terminal source events converge on the existing row through the
//! ledger's source-identity unique index, so a replayed provider callback
//! returns the first obligation instead of creating a second one
//! (`docs/state-machines.md` invariant 3, `docs/testing.md` DB-007).

use governor_core::artifact::ArtifactDigest;
use governor_core::fence::{IncarnationGeneration, SafeToken, SourceRef};
use governor_core::id::{EventId, ObligationId, ResultArtifactId};
use governor_core::obligation::{Obligation, ObligationEvent, ObligationState};
use governor_core::time::Timestamp;
use governor_core::worker_evidence::{
    ChildExitReceipt, ChildExitStatus, FinalResultReceipt, ManagedRunEvidence, ManagedRunOutcome,
    WorkerFailureClass, WorkerOutcome,
};
use rusqlite::params;

use crate::codec::{
    ActorClass, RetentionLabel, TurnLifecycle, encode_retention, encode_turn_lifecycle,
    encode_worker_failure, hex32, id_text, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::load;
use crate::ops::record_obligation_transition;
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// Metadata for an artifact whose bytes are already durable on disk.
///
/// # Why this type exists
///
/// It is the seam between the two halves of crash-safe publication. The
/// artifact layer writes an owner-private temp file, `fsync`s it, renames it to
/// its immutable key, syncs the containing directory, and only then builds one
/// of these. [`PublishWorkerResult`] cannot insert artifact metadata without
/// one, so the forbidden outcome — a committed open obligation pointing at an
/// artifact that was never made durable — is unreachable through this API.
///
/// A crash before the transaction leaves an unreferenced orphan file, which is
/// the safe direction and is quarantined by a later sweep.
///
/// # Why the fields are private
///
/// The value *is* the durability claim. Public fields would make it a plain
/// record that any caller could fill in from a filename and a guess, and the
/// seam would then be documentation rather than structure. There is exactly one
/// way to build one — [`DurableArtifact::assert_durable_from_parts`] — and its
/// name is the assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableArtifact {
    storage_ref: SafeToken,
    digest: ArtifactDigest,
    byte_len: u64,
    media_type: SafeToken,
}

impl DurableArtifact {
    /// Asserts that one artifact's bytes and immutable name are already durable.
    ///
    /// **The caller asserts that the crash-safe publication ordering —
    /// temp → write → `fsync` → link → directory `fsync` → verify — completed
    /// for these exact bytes.** This crate cannot check that; it holds the
    /// database half and never touches a file. Calling this without having
    /// performed that sequence is how the forbidden outcome is reintroduced:
    /// a committed open obligation pointing at an artifact that was never made
    /// durable.
    ///
    /// The sanctioned callers are
    /// `governor_artifacts::PublishedArtifact::durable`, which can only be
    /// reached from a value the artifact layer produced on the far side of the
    /// directory `fsync`, and test fixtures that stand in for it.
    #[must_use]
    pub const fn assert_durable_from_parts(
        storage_ref: SafeToken,
        digest: ArtifactDigest,
        byte_len: u64,
        media_type: SafeToken,
    ) -> Self {
        Self {
            storage_ref,
            digest,
            byte_len,
            media_type,
        }
    }

    /// Daemon-allocated opaque storage key. A worker never supplies a path.
    #[must_use]
    pub const fn storage_ref(&self) -> &SafeToken {
        &self.storage_ref
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

    /// Opaque media type label.
    #[must_use]
    pub const fn media_type(&self) -> &SafeToken {
        &self.media_type
    }
}

/// The bounded safe receipts that prove one managed run completed.
///
/// Carried by value rather than as a [`ConfirmedFinalResult`], because that
/// proof has no public constructor: the only way to obtain one is for
/// [`ManagedRunEvidence::classify`] to agree, and this operation runs that
/// arbitration itself. A caller cannot hand in a conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionReceipts {
    /// Opaque identity of the exact managed run.
    pub run_ref: SafeToken,
    /// Whether the final structured record was received in full.
    pub final_result_complete: bool,
    /// The structured outcome the run reported.
    pub outcome: ManagedRunOutcome,
    /// How the managed child ended.
    pub child_exit: ChildExitStatus,
}

impl CompletionReceipts {
    pub(crate) fn classify(&self) -> WorkerOutcome {
        ManagedRunEvidence::new()
            .with_final_result(FinalResultReceipt {
                run_ref: self.run_ref.clone(),
                complete: self.final_result_complete,
                outcome: self.outcome,
            })
            .with_child_exit(ChildExitReceipt {
                run_ref: self.run_ref.clone(),
                status: self.child_exit,
            })
            .classify()
    }
}

/// A verified worker start, or re-attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWorkerStartedRequest {
    /// Obligation the worker turn belongs to.
    pub obligation: ObligationId,
    /// Source fact behind the observation.
    pub source: SourceRef,
    /// Session incarnation the observation came from.
    pub incarnation: IncarnationGeneration,
}

/// What an obligation transition committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObligationAdvanced {
    /// Obligation that moved.
    pub obligation: ObligationId,
    /// State it is now in.
    pub state: ObligationState,
    /// Version it is now at.
    pub version: governor_core::fence::ObligationVersion,
    /// Whether the source identity was already in the ledger, so this call
    /// changed nothing and reports the state that was already there.
    pub duplicate: bool,
}

/// Records a verified worker start.
pub(crate) struct RecordWorkerStarted {
    request: RecordWorkerStartedRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RecordWorkerStarted {
    type Request = RecordWorkerStartedRequest;
    type Committed = ObligationAdvanced;
    type Output = ObligationAdvanced;

    const NAME: &'static str = "record_worker_started";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::WorkerStarted,
                source: self.request.source.clone(),
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new().int(
                    "incarnation",
                    store_u64(self.request.incarnation.get(), "events", "incarnation")?,
                ),
            },
        )?;
        if appended.is_duplicate() {
            return Ok(unchanged(&loaded.projection));
        }

        let before = loaded.projection;
        let after = before
            .apply(&ObligationEvent::WorkerStarted {
                source: self.request.source.clone(),
                incarnation: self.request.incarnation,
                at: self.now,
            })?
            .or_unchanged(before.clone());
        record_obligation_transition(
            tx,
            &before,
            &after,
            appended.seq(),
            appended.seq(),
            ActorClass::Worker,
            None,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(advanced(&after))
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// A verified terminal worker failure. Unprocessed work, never a closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWorkerFailureRequest {
    /// Obligation the worker turn belongs to.
    pub obligation: ObligationId,
    /// Source fact behind the observation.
    pub source: SourceRef,
    /// Session incarnation the observation came from.
    pub incarnation: IncarnationGeneration,
    /// Documented failure class.
    pub failure: WorkerFailureClass,
}

/// Records a verified terminal worker failure.
pub(crate) struct RecordWorkerFailure {
    request: RecordWorkerFailureRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RecordWorkerFailure {
    type Request = RecordWorkerFailureRequest;
    type Committed = ObligationAdvanced;
    type Output = ObligationAdvanced;

    const NAME: &'static str = "record_worker_failure";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::WorkerFailed,
                source: self.request.source.clone(),
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .int(
                        "incarnation",
                        store_u64(self.request.incarnation.get(), "events", "incarnation")?,
                    )
                    .label(
                        "failure_class",
                        encode_worker_failure(self.request.failure, "events")?,
                    ),
            },
        )?;
        if appended.is_duplicate() {
            return Ok(unchanged(&loaded.projection));
        }

        let before = loaded.projection;
        let after = before
            .apply(&ObligationEvent::WorkerFailed {
                source: self.request.source.clone(),
                incarnation: self.request.incarnation,
                failure: self.request.failure,
                at: self.now,
            })?
            .or_unchanged(before.clone());
        finalize_turn(
            tx,
            loaded.identity.turn,
            TurnLifecycle::Failed,
            appended.seq(),
        )?;
        record_obligation_transition(
            tx,
            &before,
            &after,
            appended.seq(),
            appended.seq(),
            ActorClass::Worker,
            None,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(advanced(&after))
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Publishing a confirmed final result whose artifact is already durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishWorkerResultRequest {
    /// Obligation the worker turn belongs to.
    pub obligation: ObligationId,
    /// Source fact behind the terminal observation.
    pub source: SourceRef,
    /// Session incarnation the observation came from.
    pub incarnation: IncarnationGeneration,
    /// The bounded safe receipts. Arbitrated here, not by the caller.
    pub receipts: CompletionReceipts,
    /// The artifact the artifact layer already made durable.
    pub artifact: DurableArtifact,
}

/// What a terminal publication committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedResult {
    /// The obligation, now `completed_unprocessed` and still open.
    pub obligation: ObligationAdvanced,
    /// The artifact metadata row, pinned by that open obligation.
    pub artifact: ResultArtifactId,
}

/// The one-transaction worker terminal publication.
pub(crate) struct PublishWorkerResult {
    request: PublishWorkerResultRequest,
    artifact: ResultArtifactId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for PublishWorkerResult {
    type Request = PublishWorkerResultRequest;
    type Committed = PublishedResult;
    type Output = PublishedResult;

    const NAME: &'static str = "publish_worker_result";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            artifact: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        // Arbitration first: a truncated final record, a missing exit, or an
        // exit belonging to another run is not completion, and there is no way
        // past this because `ConfirmedFinalResult` has no other constructor.
        let WorkerOutcome::ConfirmedCompletion(proof) = self.request.receipts.classify() else {
            return Err(CorruptValue::new(
                "events",
                "safe_metadata_json",
                CorruptReason::UnprovableEvidence,
            )
            .into());
        };

        let loaded = load::obligation(tx, self.request.obligation)?;
        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ResultPublished,
                source: self.request.source.clone(),
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .int(
                        "incarnation",
                        store_u64(self.request.incarnation.get(), "events", "incarnation")?,
                    )
                    .token("run_ref", proof.run_ref())
                    .id("artifact_id", self.artifact),
            },
        )?;
        if appended.is_duplicate() {
            // The terminal source fact is already in the ledger. Return the
            // obligation and artifact that already exist; never a second one.
            let existing = loaded.projection.result_artifact().ok_or_else(|| {
                CorruptValue::new(
                    "obligations",
                    "result_artifact_id",
                    CorruptReason::DanglingReference,
                )
            })?;
            return Ok(PublishedResult {
                obligation: unchanged(&loaded.projection),
                artifact: existing,
            });
        }

        let turn = loaded.identity.turn.ok_or_else(|| {
            CorruptValue::new("obligations", "turn_id", CorruptReason::DanglingReference)
        })?;
        tx.conn().execute(
            "INSERT INTO result_artifacts (result_artifact_id, task_id, turn_id, source_event_seq,
                    storage_ref, sha256_hex, byte_len, media_type, created_at_ms, retention_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id_text(self.artifact),
                id_text(loaded.identity.task),
                id_text(turn),
                event::store_seq(appended.seq())?,
                self.request.artifact.storage_ref().as_str(),
                hex32(self.request.artifact.digest().as_bytes()),
                store_u64(
                    self.request.artifact.byte_len(),
                    "result_artifacts",
                    "byte_len"
                )?,
                self.request.artifact.media_type().as_str(),
                store_time(self.now),
                // Provisional: `record_obligation_transition` recomputes it from
                // the obligations that actually reference the artifact.
                encode_retention(RetentionLabel::Eligible),
            ],
        )?;
        finalize_turn(
            tx,
            loaded.identity.turn,
            TurnLifecycle::Completed,
            appended.seq(),
        )?;

        let before = loaded.projection;
        let after = before
            .apply(&ObligationEvent::ResultPublished {
                source: self.request.source.clone(),
                incarnation: self.request.incarnation,
                proof,
                artifact: self.artifact,
                at: self.now,
            })?
            .or_unchanged(before.clone());
        record_obligation_transition(
            tx,
            &before,
            &after,
            appended.seq(),
            appended.seq(),
            ActorClass::Worker,
            None,
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(PublishedResult {
            obligation: advanced(&after),
            artifact: self.artifact,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Marks a turn terminal in its projection row.
pub(crate) fn finalize_turn(
    tx: &Tx<'_>,
    turn: Option<governor_core::id::TurnId>,
    lifecycle: TurnLifecycle,
    seq: governor_core::fence::EventSeq,
) -> StoreResult<()> {
    let Some(turn) = turn else {
        return Ok(());
    };
    tx.conn().execute(
        "UPDATE turns
            SET lifecycle_state = ?2, terminal_event_seq = ?3, latest_event_seq = ?3
          WHERE turn_id = ?1",
        params![
            id_text(turn),
            encode_turn_lifecycle(lifecycle),
            event::store_seq(seq)?
        ],
    )?;
    Ok(())
}

pub(crate) fn advanced(obligation: &Obligation) -> ObligationAdvanced {
    ObligationAdvanced {
        obligation: obligation.id(),
        state: obligation.state(),
        version: obligation.version(),
        duplicate: false,
    }
}

pub(crate) fn unchanged(obligation: &Obligation) -> ObligationAdvanced {
    ObligationAdvanced {
        duplicate: true,
        ..advanced(obligation)
    }
}

/// Cancelling delegated work on the local user's authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelObligationRequest {
    /// Obligation being cancelled.
    pub obligation: ObligationId,
    /// Source fact behind the decision, from the CLI.
    pub source: SourceRef,
}

/// Closes an obligation by user cancellation.
///
/// One of the two closing paths that is *not* a foreman ACK, and it is
/// deliberately explicit: `docs/state-machines.md` invariant 1 requires a
/// closing disposition event, and cancellation is one. Closing releases the
/// artifact pin, which `record_obligation_transition` recomputes rather than
/// setting.
pub(crate) struct CancelObligation {
    request: CancelObligationRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for CancelObligation {
    type Request = CancelObligationRequest;
    type Committed = ObligationAdvanced;
    type Output = ObligationAdvanced;

    const NAME: &'static str = "cancel_obligation";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        let before = loaded.projection;
        let transition = before.apply(&ObligationEvent::CancelledByUser {
            source: self.request.source.clone(),
            at: self.now,
        })?;
        let Some(after) = transition.advanced() else {
            return Ok(unchanged(&before));
        };

        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ObligationCancelledByUser,
                source: self.request.source.clone(),
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new(),
            },
        )?;
        if appended.is_duplicate() {
            return Ok(unchanged(&before));
        }
        record_obligation_transition(
            tx,
            &before,
            &after,
            appended.seq(),
            appended.seq(),
            ActorClass::User,
            None,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(advanced(&after))
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}
