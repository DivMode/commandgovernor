//! Rebuilding domain projections from the durable ledger.
//!
//! # Why a fenced transition reads the ledger, not the row
//!
//! `governor-core` deliberately offers no field-wise constructor for
//! [`Obligation`]: "the only way to reach any state but `created` is to fold
//! [`ObligationEvent`]s". This module honours that rather than working around
//! it. A compare-then-mutate holds the write lock, folds the obligation's
//! ledger slice, and applies the next event to the value that fold produced —
//! so the state a fence is checked against was necessarily built by the state
//! machine.
//!
//! The `obligations` and `browser_deliveries` rows are a *materialised* copy of
//! that fold, written in the same transaction and used for indexed lookup.
//! [`crate::replay`] proves the copy still agrees with the ledger, which is
//! exactly `docs/testing.md` DB-001.
//!
//! # Evidence is re-proved, never assumed
//!
//! [`ConfirmedFinalResult`] has no public constructor either: obtaining one
//! requires [`ManagedRunEvidence::classify`] to agree. So replaying a
//! `result_published` event does not fabricate the proof — it rebuilds the safe
//! receipts the event stands on and runs the real arbitration again. If
//! `governor-core` ever stopped classifying those facts as a confirmed
//! completion, replay would fail closed with
//! [`CorruptReason::UnprovableEvidence`] instead of quietly projecting success.

use governor_core::artifact::{ArtifactDigest, ResultArtifact};
use governor_core::binding::{
    BindingEvent, BindingLedger, BrowserProfileRef, ConnectorAbi, ConversationRef,
    VerifiedBindingTarget,
};
use governor_core::delivery::{
    AcceptedWakeEvidence, BrowserWake, DeliveryId, PersistedWake, WakeTarget,
};
use governor_core::fence::{
    AttemptNo, BindingGeneration, DeliveryRevision, EventSeq, IncarnationGeneration,
    ObligationVersion, SafeToken, SourceRef,
};
use governor_core::foreman_turn::ProviderMessageRef;
use governor_core::health::{HealthConditionKind, HealthConditionState, HealthLedger, HealthScope};
use governor_core::id::{HealthConditionId, ObligationId, ResultArtifactId, TaskId, TurnId};
use governor_core::obligation::{
    AckRequest, Obligation, ObligationEvent, ObligationKind, ObligationState,
};
use governor_core::outbound::{Delivery, DeliveryEvent};
use governor_core::time::Timestamp;
use governor_core::worker_evidence::{
    ChildExitReceipt, ChildExitStatus, ConfirmedFinalResult, FinalResultReceipt,
    ManagedRunEvidence, ManagedRunOutcome, WorkerOutcome,
};

use rusqlite::{OptionalExtension as _, params};

use crate::codec::{
    RetentionLabel, decode_ambiguity, decode_disposition, decode_failure_class, decode_health_kind,
    decode_health_state, decode_obligation_kind, decode_worker_failure, decode_write_capability,
    encode_retention, id_text, parse_delivery_id, parse_id, parse_source, parse_token, parse_u32,
    parse_u64, rederive_delivery_key,
};
use crate::error::{CorruptReason, CorruptValue, StoreError, StoreResult};
use crate::event::{self, EventKind, LedgerEvent, parse_seq, store_seq};
use crate::tx::Tx;

/// The immutable identity columns of an `obligations` row.
///
/// Everything here is fixed at creation. The *fenced* state — lifecycle state,
/// version, source, claim — is never read from the row; it is folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObligationIdentity {
    pub(crate) task: TaskId,
    pub(crate) turn: Option<TurnId>,
    pub(crate) kind: ObligationKind,
    pub(crate) priority: i64,
    pub(crate) created_event_seq: EventSeq,
}

/// An obligation, folded from its ledger slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedObligation {
    pub(crate) identity: ObligationIdentity,
    pub(crate) projection: Obligation,
    /// Sequence of the newest event in the slice.
    pub(crate) latest_event_seq: EventSeq,
    /// Sequence of the event carrying the source fact the obligation stands on.
    pub(crate) source_event_seq: EventSeq,
}

fn dangling(table: &'static str, column: &'static str) -> StoreError {
    CorruptValue::new(table, column, CorruptReason::DanglingReference).into()
}

/// Reads an obligation's identity columns.
///
/// # Errors
///
/// Returns a corrupt-row error when the obligation does not exist.
pub(crate) fn obligation_identity(
    tx: &Tx<'_>,
    obligation: ObligationId,
) -> StoreResult<ObligationIdentity> {
    let row: Option<(String, Option<String>, String, i64, i64)> = tx
        .conn()
        .query_row(
            "SELECT task_id, turn_id, obligation_kind, priority, created_event_seq
               FROM obligations WHERE obligation_id = ?1",
            params![id_text(obligation)],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let (task, turn, kind, priority, created) =
        row.ok_or_else(|| dangling("obligations", "obligation_id"))?;
    Ok(ObligationIdentity {
        task: parse_id(&task, "obligations", "task_id")?,
        turn: turn
            .map(|text| parse_id(&text, "obligations", "turn_id"))
            .transpose()?,
        kind: decode_obligation_kind(&kind, "obligations")?,
        priority,
        created_event_seq: parse_seq(created, "obligations", "created_event_seq")?,
    })
}

/// Folds one obligation's ledger slice into its current projection.
///
/// # Errors
///
/// - a corrupt-row error when the slice is missing, does not start with a
///   creation event, or carries evidence that no longer proves its transition;
/// - [`StoreError::Conflict`] when a recorded transition is not legal from the
///   state the fold reached, which means the ledger itself disagrees with the
///   state machine.
pub(crate) fn obligation(tx: &Tx<'_>, id: ObligationId) -> StoreResult<LoadedObligation> {
    let identity = obligation_identity(tx, id)?;
    let events = event::read_for_obligation(tx, id)?;
    fold_obligation(id, identity, &events)
}

/// Folds a ledger slice that has already been read.
///
/// # Errors
///
/// As [`obligation`].
pub(crate) fn fold_obligation(
    id: ObligationId,
    identity: ObligationIdentity,
    events: &[LedgerEvent],
) -> StoreResult<LoadedObligation> {
    let malformed = |reason| CorruptValue::new("events", "kind", reason);
    let (created, rest) = events
        .split_first()
        .ok_or_else(|| malformed(CorruptReason::DanglingReference))?;
    if created.kind != EventKind::ObligationCreated {
        return Err(malformed(CorruptReason::UnprovableEvidence).into());
    }

    let mut projection = Obligation::created(
        id,
        identity.task,
        identity.turn,
        decode_obligation_kind(created.metadata.label("obligation_kind")?, "events")?,
        created.source.clone(),
        IncarnationGeneration::new(created.metadata.u64("incarnation")?),
    );
    let mut source_event_seq = created.seq;
    let mut latest_event_seq = created.seq;

    for event in rest {
        let Some(domain) = obligation_event(&projection, event)? else {
            // Not an obligation transition: a delivery attempt event scoped to
            // the same obligation, for instance. It advances no version.
            latest_event_seq = event.seq;
            continue;
        };
        let changes_source = matches!(
            event.kind,
            EventKind::WorkerStarted
                | EventKind::WorkerFailed
                | EventKind::ResultPublished
                | EventKind::ObligationCancelledByUser
                | EventKind::ObligationSuperseded
        );
        projection = projection.apply(&domain)?.or_unchanged(projection);
        if changes_source {
            source_event_seq = event.seq;
        }
        latest_event_seq = event.seq;
    }

    Ok(LoadedObligation {
        identity,
        projection,
        latest_event_seq,
        source_event_seq,
    })
}

/// Translates one ledger event into the obligation transition it recorded.
///
/// Returns `None` for a ledger event that is scoped to the obligation but is
/// not one of its transitions.
fn obligation_event(
    current: &Obligation,
    event: &LedgerEvent,
) -> StoreResult<Option<ObligationEvent>> {
    let fields = &event.metadata;
    let translated = match event.kind {
        EventKind::WorkerStarted => ObligationEvent::WorkerStarted {
            source: event.source.clone(),
            incarnation: IncarnationGeneration::new(fields.u64("incarnation")?),
            at: event.observed_at,
        },
        EventKind::WorkerFailed => ObligationEvent::WorkerFailed {
            source: event.source.clone(),
            incarnation: IncarnationGeneration::new(fields.u64("incarnation")?),
            failure: decode_worker_failure(fields.label("failure_class")?, "events")?,
            at: event.observed_at,
        },
        EventKind::ResultPublished => {
            let run_ref = fields.token("run_ref")?;
            ObligationEvent::ResultPublished {
                source: event.source.clone(),
                incarnation: IncarnationGeneration::new(fields.u64("incarnation")?),
                proof: reprove_completion(&run_ref)?,
                artifact: fields.id::<governor_core::id::kind::ResultArtifact>("artifact_id")?,
                at: event.observed_at,
            }
        }
        EventKind::ForemanClaimMinted => ObligationEvent::ForemanClaimed {
            claim: fields.id::<governor_core::id::kind::Claim>("claim_id")?,
            binding_generation: BindingGeneration::new(fields.u64("binding_generation")?),
            expected_version: ObligationVersion::new(fields.u64("expected_version")?),
            // The recorded `expected_version` is the fence the caller actually
            // presented, so `Obligation::apply` compares it against the fold
            // and a drifted ledger is rejected rather than rubber-stamped. The
            // source fence moves in lockstep with the version — every accepted
            // transition advances both — so matching the version pins the same
            // point in history.
            expected_source: current.source().clone(),
            at: event.observed_at,
        },
        EventKind::ForemanHandoffDelivered => ObligationEvent::HandoffDelivered {
            claim: fields.id::<governor_core::id::kind::Claim>("claim_id")?,
            at: event.observed_at,
        },
        EventKind::ForemanClaimExpired => ObligationEvent::ClaimExpired {
            claim: fields.id::<governor_core::id::kind::Claim>("claim_id")?,
            at: event.observed_at,
        },
        EventKind::ForemanAcked => ObligationEvent::ForemanAcked(Box::new(AckRequest {
            obligation: current.id(),
            expected_version: ObligationVersion::new(fields.u64("expected_version")?),
            expected_source: current.source().clone(),
            binding_generation: BindingGeneration::new(fields.u64("binding_generation")?),
            claim: fields.id::<governor_core::id::kind::Claim>("claim_id")?,
            disposition: decode_disposition(fields.label("disposition")?, "events")?,
            at: event.observed_at,
        })),
        EventKind::ObligationCancelledByUser => ObligationEvent::CancelledByUser {
            source: event.source.clone(),
            at: event.observed_at,
        },
        EventKind::ObligationSuperseded => ObligationEvent::Superseded {
            source: event.source.clone(),
            replacement: fields.id::<governor_core::id::kind::Obligation>("replacement")?,
            at: event.observed_at,
        },
        _ => return Ok(None),
    };
    Ok(Some(translated))
}

/// Re-proves a confirmed completion from the safe facts the event stands on.
///
/// Not a constructor: it rebuilds the bounded receipts a `result_published`
/// event is only ever written for — a complete successful final structured
/// result and a matching successful child exit for the same run — and asks
/// [`ManagedRunEvidence::classify`] again. Anything but
/// [`WorkerOutcome::ConfirmedCompletion`] is a fail-closed corrupt row.
fn reprove_completion(run_ref: &SafeToken) -> StoreResult<ConfirmedFinalResult> {
    let evidence = ManagedRunEvidence::new()
        .with_final_result(FinalResultReceipt {
            run_ref: run_ref.clone(),
            complete: true,
            outcome: ManagedRunOutcome::Success,
        })
        .with_child_exit(ChildExitReceipt {
            run_ref: run_ref.clone(),
            status: ChildExitStatus::Success,
        });
    match evidence.classify() {
        WorkerOutcome::ConfirmedCompletion(proof) => Ok(proof),
        _ => Err(CorruptValue::new(
            "events",
            "safe_metadata_json",
            CorruptReason::UnprovableEvidence,
        )
        .into()),
    }
}

// --- Foreman bindings -------------------------------------------------------

/// Folds the binding ledger from its projection rows.
///
/// Rows are replayed in generation order, which reproduces the generations the
/// ledger assigns: `BindingEvent::Bound` always takes `highest + 1`. A gap or a
/// second active row is therefore impossible to reproduce and fails closed.
///
/// # Errors
///
/// Returns a corrupt-row error, or a conflict when the rows cannot be folded.
pub(crate) fn bindings(tx: &Tx<'_>) -> StoreResult<BindingLedger> {
    let mut statement = tx.conn().prepare(
        "SELECT foreman_binding_id, canonical_conversation_id, browser_profile_id,
                connector_abi, binding_generation, capability_epoch,
                write_capability_state, is_active
           FROM foreman_bindings ORDER BY binding_generation",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
        ))
    })?;

    let mut ledger = BindingLedger::unbound();
    for row in rows {
        let (id, conversation, profile, abi, generation, epoch, capability, active) = row?;
        const TABLE: &str = "foreman_bindings";
        let generation =
            BindingGeneration::new(parse_u64(generation, TABLE, "binding_generation")?);
        let target = VerifiedBindingTarget {
            id: parse_id(&id, TABLE, "foreman_binding_id")?,
            conversation: ConversationRef::new(parse_token(
                &conversation,
                TABLE,
                "canonical_conversation_id",
            )?),
            profile: BrowserProfileRef::new(parse_token(&profile, TABLE, "browser_profile_id")?),
            connector_abi: ConnectorAbi::new(parse_token(&abi, TABLE, "connector_abi")?),
            capability_epoch: parse_u64(epoch, TABLE, "capability_epoch")?,
            write_capability: decode_write_capability(&capability, TABLE)?,
        };
        ledger = ledger
            .apply(&BindingEvent::Bound {
                target: Box::new(target),
                at: Timestamp::from_unix_millis(0),
            })?
            .or_unchanged(ledger);
        if ledger
            .active()
            .is_none_or(|binding| binding.generation() != generation)
        {
            // The row's recorded generation is not the one replay assigns, so
            // the projection has a gap. Refuse rather than fence against a
            // generation that never existed.
            return Err(CorruptValue::new(
                TABLE,
                "binding_generation",
                CorruptReason::UnprovableEvidence,
            )
            .into());
        }
        if active == 0 {
            ledger = ledger
                .apply(&BindingEvent::Displaced {
                    generation,
                    at: Timestamp::from_unix_millis(0),
                })?
                .or_unchanged(ledger);
        }
    }
    Ok(ledger)
}

// --- Browser wake deliveries ------------------------------------------------

/// A wake revision, folded from its row plus its attempt events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedWake {
    pub(crate) wake: BrowserWake,
    /// Sequence of the event that established the wake's target snapshot.
    pub(crate) target_source_event_seq: EventSeq,
}

/// Loads one wake revision by its random correlation ID.
///
/// # Errors
///
/// Returns [`StoreError::Conflict`] with
/// [`governor_core::error::Conflict::UnknownDeliveryId`] when no such delivery
/// exists — deliberately undifferentiated, so probing correlation IDs reveals
/// nothing — or a corrupt-row error for an undecodable row.
pub(crate) fn wake_by_delivery_id(
    tx: &Tx<'_>,
    delivery_id: &DeliveryId,
) -> StoreResult<LoadedWake> {
    let found: Option<String> = tx
        .conn()
        .query_row(
            "SELECT delivery_id FROM browser_deliveries WHERE delivery_id = ?1",
            params![delivery_id.expose_hex()],
            |row| row.get(0),
        )
        .optional()?;
    if found.is_none() {
        return Err(governor_core::error::Conflict::UnknownDeliveryId.into());
    }
    wake_by_hex(tx, &delivery_id.expose_hex())
}

/// Loads the wake revision recorded under a deterministic key, if any.
///
/// # Errors
///
/// Returns a corrupt-row error for an undecodable row.
pub(crate) fn wake_by_key(tx: &Tx<'_>, key_hex: &str) -> StoreResult<Option<LoadedWake>> {
    let found: Option<String> = tx
        .conn()
        .query_row(
            "SELECT delivery_id FROM browser_deliveries WHERE delivery_key = ?1",
            params![key_hex],
            |row| row.get(0),
        )
        .optional()?;
    found.map(|hex| wake_by_hex(tx, &hex)).transpose()
}

/// The `browser_deliveries` columns one wake is rebuilt from.
type WakeRow = (
    String,
    String,
    i64,
    i64,
    String,
    i64,
    i64,
    i64,
    Option<String>,
);

fn wake_by_hex(tx: &Tx<'_>, delivery_hex: &str) -> StoreResult<LoadedWake> {
    const TABLE: &str = "browser_deliveries";
    let row: Option<WakeRow> = tx
        .conn()
        .query_row(
            "SELECT delivery_key, obligation_id, target_obligation_version,
                    target_source_event_seq, foreman_binding_id, binding_generation,
                    delivery_revision, attempt_budget, accepted_message_ref
               FROM browser_deliveries WHERE delivery_id = ?1",
            params![delivery_hex],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    let (key_hex, obligation, version, target_seq, binding, generation, revision, budget, message) =
        row.ok_or_else(|| dangling(TABLE, "delivery_id"))?;

    let obligation: ObligationId = parse_id(&obligation, TABLE, "obligation_id")?;
    let generation = BindingGeneration::new(parse_u64(generation, TABLE, "binding_generation")?);
    let revision = DeliveryRevision::new(parse_u32(revision, TABLE, "delivery_revision")?);
    let target_source_event_seq = parse_seq(target_seq, TABLE, "target_source_event_seq")?;

    // The wake's target snapshot is the source fact recorded on the event the
    // delivery pinned, read back from the ledger rather than duplicated here.
    let source = source_of_event(tx, target_source_event_seq)?;
    let conversation = active_conversation(tx, binding.as_str())?;

    let events = event::read_for_obligation(tx, obligation)?;
    let delivery = fold_delivery(
        &events,
        revision,
        parse_u32(budget, TABLE, "attempt_budget")?,
        &conversation,
        message.as_deref(),
    )?;

    let wake = BrowserWake::rehydrate(PersistedWake {
        delivery_id: parse_delivery_id(delivery_hex, TABLE, "delivery_id")?,
        delivery_key: rederive_delivery_key(&key_hex, obligation, generation, revision)?,
        target: WakeTarget {
            obligation,
            version: ObligationVersion::new(parse_u64(
                version,
                TABLE,
                "target_obligation_version",
            )?),
            source,
        },
        binding: parse_id::<governor_core::id::kind::ForemanBinding>(
            &binding,
            TABLE,
            "foreman_binding_id",
        )?,
        binding_generation: generation,
        revision,
        delivery,
    })
    .map_err(|_| -> StoreError {
        CorruptValue::new(TABLE, "delivery_key", CorruptReason::MalformedIdentity).into()
    })?;

    Ok(LoadedWake {
        wake,
        target_source_event_seq,
    })
}

fn source_of_event(tx: &Tx<'_>, seq: EventSeq) -> StoreResult<SourceRef> {
    let row: Option<(String, String, String)> = tx
        .conn()
        .query_row(
            "SELECT source_namespace, source_event_id, source_event_fence
               FROM events WHERE seq = ?1",
            params![store_seq(seq)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (namespace, id, fence) = row.ok_or_else(|| dangling("events", "seq"))?;
    parse_source(&namespace, &id, &fence)
}

fn active_conversation(tx: &Tx<'_>, binding: &str) -> StoreResult<ConversationRef> {
    let found: Option<String> = tx
        .conn()
        .query_row(
            "SELECT canonical_conversation_id FROM foreman_bindings WHERE foreman_binding_id = ?1",
            params![binding],
            |row| row.get(0),
        )
        .optional()?;
    let text = found.ok_or_else(|| dangling("foreman_bindings", "foreman_binding_id"))?;
    Ok(ConversationRef::new(parse_token(
        &text,
        "foreman_bindings",
        "canonical_conversation_id",
    )?))
}

/// Folds one revision's attempt machine from the obligation's ledger slice.
///
/// # Errors
///
/// Returns a corrupt-row error, or a conflict when a recorded attempt event is
/// not legal from the state the fold reached.
pub(crate) fn fold_delivery(
    events: &[LedgerEvent],
    revision: DeliveryRevision,
    attempt_budget: u32,
    conversation: &ConversationRef,
    accepted_message: Option<&str>,
) -> StoreResult<Delivery<AcceptedWakeEvidence>> {
    let mut delivery = Delivery::pending(attempt_budget);
    for event in events {
        let Some(domain) = delivery_event(event, revision, conversation, accepted_message)? else {
            continue;
        };
        delivery = delivery.apply(&domain)?.or_unchanged(delivery);
    }
    Ok(delivery)
}

fn delivery_event(
    event: &LedgerEvent,
    revision: DeliveryRevision,
    conversation: &ConversationRef,
    accepted_message: Option<&str>,
) -> StoreResult<Option<DeliveryEvent<AcceptedWakeEvidence>>> {
    let is_delivery = matches!(
        event.kind,
        EventKind::BrowserDeliveryAttemptClaimed
            | EventKind::BrowserDeliveryActivationArmed
            | EventKind::BrowserDeliveryAccepted
            | EventKind::BrowserDeliveryFailed
            | EventKind::BrowserDeliveryAmbiguous
            | EventKind::BrowserDeliveryOrphanQuarantined
            | EventKind::BrowserDeliveryReconciled
    );
    if !is_delivery {
        return Ok(None);
    }
    let fields = &event.metadata;
    if DeliveryRevision::new(fields.u32("revision")?) != revision {
        return Ok(None);
    }
    let attempt = || -> StoreResult<AttemptNo> { Ok(AttemptNo::new(fields.u32("attempt_no")?)) };
    let translated = match event.kind {
        EventKind::BrowserDeliveryAttemptClaimed => DeliveryEvent::AttemptClaimed {
            at: event.observed_at,
        },
        EventKind::BrowserDeliveryActivationArmed => DeliveryEvent::ActivationArmed {
            attempt: attempt()?,
            at: event.observed_at,
        },
        EventKind::BrowserDeliveryAccepted => {
            let message = accepted_message
                .ok_or_else(|| dangling("browser_deliveries", "accepted_message_ref"))?;
            DeliveryEvent::AttemptAccepted {
                attempt: attempt()?,
                evidence: AcceptedWakeEvidence::new(
                    conversation.clone(),
                    ProviderMessageRef::new(parse_token(
                        message,
                        "browser_deliveries",
                        "accepted_message_ref",
                    )?),
                ),
                at: event.observed_at,
            }
        }
        EventKind::BrowserDeliveryFailed => DeliveryEvent::AttemptFailed {
            attempt: attempt()?,
            failure: decode_failure_class(fields.label("failure_class")?, "delivery_attempts")?,
            at: event.observed_at,
        },
        EventKind::BrowserDeliveryAmbiguous => DeliveryEvent::AttemptAmbiguous {
            attempt: attempt()?,
            reason: decode_ambiguity(fields.label("evidence_class")?, "delivery_attempts")?,
            at: event.observed_at,
        },
        EventKind::BrowserDeliveryOrphanQuarantined => DeliveryEvent::OrphanQuarantined {
            at: event.observed_at,
        },
        // Reconciliation carries the same exact evidence acceptance does: the
        // conversation is the delivery's bound one, never the reconciler's to
        // assert, and the message is the one that was proven to exist.
        EventKind::BrowserDeliveryReconciled => {
            let message = accepted_message
                .ok_or_else(|| dangling("browser_deliveries", "accepted_message_ref"))?;
            DeliveryEvent::ReconciledAccepted {
                evidence: AcceptedWakeEvidence::new(
                    conversation.clone(),
                    ProviderMessageRef::new(parse_token(
                        message,
                        "browser_deliveries",
                        "accepted_message_ref",
                    )?),
                ),
                at: event.observed_at,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(translated))
}

// --- Artifact retention -----------------------------------------------------

/// Recomputes and writes the retention state of one artifact.
///
/// Retention is *derived*: an artifact is pinned exactly while some open
/// obligation references it. There is no setter, so nothing can release an
/// artifact an open obligation still needs.
///
/// The deletion instant is kept consistent with that answer in the same
/// statement: a pinned artifact has no deletion instant, full stop. Stamping it
/// is a separate, explicit act — see [`stamp_deletion_instant`].
///
/// # Errors
///
/// Returns a SQLite error.
pub(crate) fn refresh_retention(tx: &Tx<'_>, artifact: ResultArtifactId) -> StoreResult<()> {
    tx.conn().execute(
        "UPDATE result_artifacts
            SET retention_state = CASE WHEN EXISTS (
                    SELECT 1 FROM obligations
                     WHERE result_artifact_id = ?1 AND closed_event_seq IS NULL
                ) THEN ?2 ELSE ?3 END,
                eligible_for_delete_at_ms = CASE WHEN EXISTS (
                    SELECT 1 FROM obligations
                     WHERE result_artifact_id = ?1 AND closed_event_seq IS NULL
                ) THEN NULL ELSE eligible_for_delete_at_ms END
          WHERE result_artifact_id = ?1",
        params![
            id_text(artifact),
            encode_retention(RetentionLabel::Pinned),
            encode_retention(RetentionLabel::Eligible),
        ],
    )?;
    Ok(())
}

/// Records the earliest instant at which a released artifact may be deleted.
///
/// `docs/data-model.md`: *ACK only makes an artifact retention-eligible;
/// asynchronous GC deletes later.* This writes the "later": the instant a sweep
/// compares `now` against, computed by the caller as **ACK instant + the
/// retention delay it was given**. The store invents neither half.
///
/// Two guards, both deliberate:
///
/// - `retention_state = 'eligible'` — [`refresh_retention`] has already
///   recomputed the pin from the obligations that actually reference the
///   artifact, so this stamps only what that recompute genuinely released. An
///   artifact another open obligation still needs is untouched.
/// - `COALESCE` — the first release instant stands. A repeated or idempotent
///   ACK must not push the deletion of an already-released artifact further
///   into the future.
///
/// A row that never gets here keeps `NULL`, and the artifact layer's retention
/// decision fails closed on it: an artifact whose deletion instant is unknown
/// is kept, never guessed at.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-value error for an unstorable instant.
pub(crate) fn stamp_deletion_instant(
    tx: &Tx<'_>,
    artifact: ResultArtifactId,
    deletable_at: Timestamp,
) -> StoreResult<()> {
    tx.conn().execute(
        "UPDATE result_artifacts
            SET eligible_for_delete_at_ms = COALESCE(eligible_for_delete_at_ms, ?2)
          WHERE result_artifact_id = ?1 AND retention_state = ?3",
        params![
            id_text(artifact),
            crate::codec::store_time(deletable_at),
            encode_retention(RetentionLabel::Eligible),
        ],
    )?;
    Ok(())
}

// --- Health conditions ------------------------------------------------------

/// One condition currently demanding attention.
///
/// A health condition is attention, never terminal worker state: nothing in
/// this shape can close an obligation, and there is no store operation that
/// takes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenCondition {
    /// What kind of attention.
    pub kind: HealthConditionKind,
    /// What the condition is about. Every field is optional and opaque.
    pub scope: HealthScope,
}

/// One transition a ledger event records against the health ledger.
enum HealthTransition {
    /// Open a condition, deduplicated on `(kind, scope)`.
    Raise(HealthConditionKind, HealthScope),
    /// Close the open condition for `(kind, scope)`, if there is one.
    Resolve(HealthConditionKind, HealthScope),
}

/// Translates one ledger event into the health transition it recorded.
///
/// Returns `None` for every event that says nothing about attention, which is
/// almost all of them.
fn health_event(event: &LedgerEvent) -> StoreResult<Option<HealthTransition>> {
    let translated = match event.kind {
        // Startup quarantine's finding: the attempt scope is in metadata
        // because `events` has no column that can express it.
        EventKind::ExternalAttemptQuarantined => HealthTransition::Raise(
            HealthConditionKind::ReconciliationRequired,
            HealthScope::external_attempt(
                event
                    .metadata
                    .id::<governor_core::id::kind::ExternalAttempt>("external_attempt")?,
            ),
        ),
        EventKind::HealthConditionOpened => HealthTransition::Raise(
            decode_health_kind(event.metadata.label("health_kind")?, "events")?,
            scope_of(event),
        ),
        EventKind::HealthConditionResolved => HealthTransition::Resolve(
            decode_health_kind(event.metadata.label("health_kind")?, "events")?,
            scope_of(event),
        ),
        _ => return Ok(None),
    };
    Ok(Some(translated))
}

/// The health scope an event's own scope columns describe.
///
/// Exactly the columns, never more: a writer that recorded a task alongside an
/// obligation would be describing a different scope, and `HealthLedger::raise`
/// deduplicates on the whole tuple.
const fn scope_of(event: &LedgerEvent) -> HealthScope {
    HealthScope {
        task: event.scope.task,
        turn: event.scope.turn,
        obligation: event.scope.obligation,
        external_attempt: None,
    }
}

/// Folds the whole health ledger from the durable event log.
///
/// # Errors
///
/// Returns a corrupt-row error for an undecodable condition kind or scope.
pub(crate) fn health_ledger(tx: &Tx<'_>) -> StoreResult<HealthLedger> {
    fold_health(&event::read_all(tx)?)
}

/// Folds a health ledger from an already-read slice.
///
/// The identity each raise is given is derived from the event sequence rather
/// than read back: a condition's *identity* is a minted opaque value the ledger
/// never branches on, while its `(kind, scope, state)` is the semantic state
/// [`crate::replay`] compares. Deriving it keeps the fold total without
/// pretending to reproduce a value only the writer knew.
///
/// # Errors
///
/// As [`health_ledger`].
pub(crate) fn fold_health(events: &[LedgerEvent]) -> StoreResult<HealthLedger> {
    let mut ledger = HealthLedger::new();
    for event in events {
        match health_event(event)? {
            Some(HealthTransition::Raise(kind, scope)) => {
                let id = HealthConditionId::from_uuid(uuid::Uuid::from_u128(u128::from(
                    event.seq.get(),
                )));
                ledger = ledger
                    .raise(id, kind, scope, event.observed_at)?
                    .or_unchanged(ledger);
            }
            Some(HealthTransition::Resolve(kind, scope)) => {
                ledger = ledger
                    .resolve(kind, scope, event.observed_at)?
                    .or_unchanged(ledger);
            }
            None => {}
        }
    }
    Ok(ledger)
}

/// The identity of the one open condition for `(kind, scope)`, if there is one.
///
/// The partial unique index makes at most one such row possible, so this is a
/// lookup rather than a scan with a policy.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an unparseable identity.
pub(crate) fn open_condition_id(
    tx: &Tx<'_>,
    kind: HealthConditionKind,
    scope: HealthScope,
) -> StoreResult<Option<HealthConditionId>> {
    const TABLE: &str = "health_conditions";
    let found: Option<String> = tx
        .conn()
        .query_row(
            "SELECT health_condition_id FROM health_conditions
              WHERE state = 'open' AND kind = ?1
                AND COALESCE(task_id, '') = ?2
                AND COALESCE(turn_id, '') = ?3
                AND COALESCE(obligation_id, '') = ?4
                AND COALESCE(external_attempt_id, '') = ?5",
            params![
                crate::codec::encode_health_kind(kind),
                scope.task.map(id_text).unwrap_or_default(),
                scope.turn.map(id_text).unwrap_or_default(),
                scope.obligation.map(id_text).unwrap_or_default(),
                scope.external_attempt.map(id_text).unwrap_or_default(),
            ],
            |row| row.get(0),
        )
        .optional()?;
    found
        .map(|text| parse_id(&text, TABLE, "health_condition_id"))
        .transpose()
}

/// Reads every open health condition.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an undecodable label.
pub(crate) fn open_conditions(tx: &Tx<'_>) -> StoreResult<Vec<OpenCondition>> {
    const TABLE: &str = "health_conditions";
    let mut statement = tx.conn().prepare(
        "SELECT kind, state, task_id, turn_id, obligation_id, external_attempt_id
           FROM health_conditions WHERE state = 'open' ORDER BY opened_event_seq",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (kind, state, task, turn, obligation, attempt) = row?;
        // Decoded, not trusted: an unknown label is a corrupt row.
        if decode_health_state(&state, TABLE)? != HealthConditionState::Open {
            continue;
        }
        out.push(OpenCondition {
            kind: decode_health_kind(&kind, TABLE)?,
            scope: HealthScope {
                task: task
                    .map(|text| parse_id(&text, TABLE, "task_id"))
                    .transpose()?,
                turn: turn
                    .map(|text| parse_id(&text, TABLE, "turn_id"))
                    .transpose()?,
                obligation: obligation
                    .map(|text| parse_id(&text, TABLE, "obligation_id"))
                    .transpose()?,
                external_attempt: attempt
                    .map(|text| parse_id(&text, TABLE, "external_attempt_id"))
                    .transpose()?,
            },
        });
    }
    Ok(out)
}

// --- Open work, for the daemon's status surface -----------------------------

/// One obligation that still owes somebody something.
///
/// Built for the daemon's status/diagnostic surface, so every field is either
/// an opaque identity, a class, a counter, or an instant — the safe-diagnostics
/// set `docs/threat-model.md` "Threat: diagnostics become exfiltration" allows.
/// There is no task title, no repository reference, and no artifact content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenObligation {
    /// Opaque obligation identity.
    pub id: ObligationId,
    /// What the obligation is about.
    pub kind: ObligationKind,
    /// Current lifecycle state, folded from the ledger.
    pub state: ObligationState,
    /// Current compare-and-swap version.
    pub version: ObligationVersion,
    /// When the obligation was created, from its creating event.
    pub created_at: Timestamp,
    /// The artifact this obligation pins, when it has one.
    ///
    /// The whole record rather than the identity, because the caller that
    /// needs it is startup artifact verification, and it needs the digest and
    /// length to prove the bytes.
    pub result_artifact: Option<ResultArtifact>,
}

/// Every obligation whose projection row has no closing event.
///
/// The row is used only as the *index* of open work; each obligation's state is
/// then folded from its ledger slice exactly as [`obligation`] does, so nothing
/// here reads a fenced value out of the materialised copy.
///
/// # Errors
///
/// Returns a SQLite error, or whatever folding one obligation refused on.
pub(crate) fn open_obligations(tx: &Tx<'_>) -> StoreResult<Vec<OpenObligation>> {
    let mut statement = tx.conn().prepare(
        "SELECT obligation_id FROM obligations
          WHERE closed_event_seq IS NULL
          ORDER BY created_event_seq",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(parse_id(&row?, "obligations", "obligation_id")?);
    }

    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let loaded = obligation(tx, id)?;
        let artifact = loaded
            .projection
            .result_artifact()
            .map(|artifact| result_artifact(tx, artifact))
            .transpose()?;
        out.push(OpenObligation {
            id,
            kind: loaded.identity.kind,
            state: loaded.projection.state(),
            version: loaded.projection.version(),
            created_at: created_at(tx, loaded.identity.created_event_seq)?,
            result_artifact: artifact,
        });
    }
    Ok(out)
}

/// The instant an event was observed.
fn created_at(tx: &Tx<'_>, seq: EventSeq) -> StoreResult<Timestamp> {
    let millis: Option<i64> = tx
        .conn()
        .query_row(
            "SELECT observed_at_ms FROM events WHERE seq = ?1",
            params![store_seq(seq)?],
            |row| row.get(0),
        )
        .optional()?;
    millis
        .map(crate::codec::parse_time)
        .ok_or_else(|| dangling("obligations", "created_event_seq"))
}

/// Reads one committed result-artifact row.
///
/// # Errors
///
/// Returns a corrupt-row error when the row is missing or undecodable.
pub(crate) fn result_artifact(tx: &Tx<'_>, id: ResultArtifactId) -> StoreResult<ResultArtifact> {
    let row: Option<(String, String, i64, i64)> = tx
        .conn()
        .query_row(
            "SELECT storage_ref, sha256_hex, byte_len, created_at_ms
               FROM result_artifacts WHERE result_artifact_id = ?1",
            params![id_text(id)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let (storage_ref, digest_hex, byte_len, created) =
        row.ok_or_else(|| dangling("result_artifacts", "result_artifact_id"))?;
    Ok(ResultArtifact::new(
        id,
        parse_token(&storage_ref, "result_artifacts", "storage_ref")?,
        ArtifactDigest::from_bytes(crate::codec::parse_hex32(
            &digest_hex,
            "result_artifacts",
            "sha256_hex",
        )?),
        parse_u64(byte_len, "result_artifacts", "byte_len")?,
        crate::codec::parse_time(created),
    ))
}

/// Every committed result-artifact row.
///
/// The daemon's orphan sweep needs the set of storage keys the durable
/// authority actually knows about: anything else in the artifact root is
/// unreferenced.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an undecodable row.
pub(crate) fn committed_artifacts(tx: &Tx<'_>) -> StoreResult<Vec<ResultArtifact>> {
    let mut statement = tx
        .conn()
        .prepare("SELECT result_artifact_id FROM result_artifacts ORDER BY result_artifact_id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(parse_id(&row?, "result_artifacts", "result_artifact_id")?);
    }
    ids.into_iter().map(|id| result_artifact(tx, id)).collect()
}
