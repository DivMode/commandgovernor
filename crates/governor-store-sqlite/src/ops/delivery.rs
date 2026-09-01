//! Browser wake delivery: create/claim, arm Send, record the outcome.
//!
//! These are three of `docs/data-model.md`'s critical transaction boundaries,
//! and the ordering rules they encode are the ones that keep a wake from being
//! sent twice:
//!
//! - **create/claim** commits `claimed` *before any browser I/O*
//!   (`docs/state-machines.md` invariant 10);
//! - **arm** commits the Send ambiguity fence *immediately before* the exact
//!   Send activation (invariant 11), so a crash after it is recovered as
//!   ambiguous rather than guessed at;
//! - **outcome** records exactly what was observed, and `accepted`/`ambiguous`
//!   freeze the revision forever (invariant 13).
//!
//! Every one of them is a transaction that ends before the adapter is allowed
//! to touch a browser. The structural guarantee is [`crate::ports`]: a
//! transaction body has no port to reach an adapter through.
//!
//! # Two identities, and only one of them is generated here
//!
//! `delivery_key` is deterministic in `(obligation, generation, revision)`, so
//! duplicate scheduling of one logical revision converges on one durable row.
//! `delivery_id` is drawn from the CSPRNG **once**, in `prepare`. When the
//! lookup finds an existing row, the drawn value is discarded and the persisted
//! one is returned: a revision has exactly one correlation ID for its whole
//! life (`docs/data-model.md`, "Create/claim browser delivery").

use governor_core::delivery::{
    AcceptedWakeEvidence, BrowserWake, DeliveryId, DeliveryKey, WakeTarget,
};
use governor_core::error::Conflict;
use governor_core::fence::{
    AttemptNo, BindingGeneration, DeliveryRevision, ObligationVersion, SafeToken, SourceRef,
};
use governor_core::foreman_turn::ProviderMessageRef;
use governor_core::id::{DeliveryAttemptId, EventId, ObligationId};
use governor_core::outbound::{
    AmbiguityReason, AttemptState, DeliveryEvent, DeliveryState, FailureClass,
};
use governor_core::time::Timestamp;
use rusqlite::params;
use sha2::{Digest as _, Sha256};

use crate::codec::{
    encode_ambiguity, encode_attempt_state, encode_delivery_state, encode_failure_class, hex32,
    id_text, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::load;
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// Domain-separation label for the stored wake payload digest.
const WAKE_PAYLOAD_DOMAIN: &str = "command-governor/wake-payload/v1";

/// Scheduling one browser wake revision and claiming an attempt on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOrClaimDeliveryRequest {
    /// Obligation the wake is about.
    pub obligation: ObligationId,
    /// Binding generation the caller believes is current.
    pub binding_generation: BindingGeneration,
    /// Obligation version the caller believes is current.
    pub expected_version: ObligationVersion,
    /// Source fact the caller believes is current.
    pub expected_source: SourceRef,
    /// Revision within the obligation and binding generation.
    pub revision: DeliveryRevision,
    /// Bounded attempt budget for the revision.
    pub attempt_budget: u32,
    /// Opaque wake protocol label.
    pub wake_protocol: SafeToken,
}

/// The wake an adapter may now act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedDelivery {
    /// The random correlation ID carried in the wake.
    ///
    /// Generated once per revision. Never returned by bootstrap or status.
    pub delivery_id: DeliveryId,
    /// Revision this claim belongs to.
    pub revision: DeliveryRevision,
    /// Attempt number the claim created.
    pub attempt: AttemptNo,
    /// Whether this call created the revision, as opposed to finding it.
    pub created: bool,
}

/// Creates or finds a wake revision and claims an attempt, before any I/O.
pub(crate) struct CreateOrClaimDelivery {
    request: CreateOrClaimDeliveryRequest,
    /// Drawn unconditionally in `prepare`, because the CSPRNG is a port and a
    /// transaction body cannot reach one. Discarded when the row already exists.
    candidate_delivery_id: DeliveryId,
    attempt_id: DeliveryAttemptId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for CreateOrClaimDelivery {
    type Request = CreateOrClaimDeliveryRequest;
    type Committed = ClaimedDelivery;
    type Output = ClaimedDelivery;

    const NAME: &'static str = "create_or_claim_delivery";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            candidate_delivery_id: ports.draw_delivery_id(),
            attempt_id: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let bindings = load::bindings(tx)?;
        let binding = bindings.fence(self.request.binding_generation)?;
        let binding_id = binding.id();

        let loaded = load::obligation(tx, self.request.obligation)?;
        let obligation = &loaded.projection;
        obligation.require_version(self.request.expected_version)?;
        obligation.require_source(&self.request.expected_source)?;

        let key = DeliveryKey::derive(
            self.request.obligation,
            self.request.binding_generation,
            self.request.revision,
        );
        let key_hex = key.to_hex();

        let (wake, created) = match load::wake_by_key(tx, &key_hex)? {
            Some(existing) => (existing.wake, false),
            None => {
                let wake = BrowserWake::create_at_revision_for_store(
                    self.candidate_delivery_id.clone(),
                    WakeTarget::snapshot(obligation),
                    binding_id,
                    self.request.binding_generation,
                    self.request.revision,
                    self.request.attempt_budget,
                )?;
                tx.conn().execute(
                    "INSERT INTO browser_deliveries (delivery_id, delivery_key, obligation_id,
                            target_obligation_version, target_source_event_seq, foreman_binding_id,
                            binding_generation, delivery_revision, attempt_budget, wake_protocol,
                            wake_payload_digest, state)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        self.candidate_delivery_id.expose_hex(),
                        key_hex,
                        id_text(self.request.obligation),
                        store_u64(
                            obligation.version().get(),
                            "browser_deliveries",
                            "target_obligation_version"
                        )?,
                        event::store_seq(loaded.source_event_seq)?,
                        id_text(binding_id),
                        store_u64(
                            self.request.binding_generation.get(),
                            "browser_deliveries",
                            "binding_generation"
                        )?,
                        i64::from(self.request.revision.get()),
                        i64::from(self.request.attempt_budget),
                        self.request.wake_protocol.as_str(),
                        wake_payload_digest(
                            &self.candidate_delivery_id,
                            self.request.obligation,
                            self.request.binding_generation,
                            self.request.revision,
                            &self.request.wake_protocol,
                        ),
                        encode_delivery_state(DeliveryState::Pending),
                    ],
                )?;
                (wake, true)
            }
        };

        // The wake's own snapshot must still be current: a revision found by key
        // was scheduled against a state the obligation may since have left.
        wake.require_current_target(obligation)?;

        let next = wake
            .apply(&DeliveryEvent::AttemptClaimed { at: self.now })?
            .advanced()
            .ok_or(Conflict::IllegalDeliveryTransition {
                from: AttemptState::Claimed,
                event: "attempt_claimed",
            })?;
        let attempt = next
            .delivery()
            .attempts()
            .last()
            .expect("a claim always appends an attempt")
            .number();

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::BrowserDeliveryAttemptClaimed,
                source: crate::ops::internal_source_text(
                    &next.delivery_id().expose_hex(),
                    &format!("claimed.{attempt}"),
                )?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .int("revision", i64::from(self.request.revision.get()))
                    .int("attempt_no", i64::from(attempt.get())),
            },
        )?
        .seq();

        tx.conn().execute(
            "INSERT INTO delivery_attempts (delivery_attempt_id, delivery_id, attempt_no, state,
                    claimed_event_seq, started_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id_text(self.attempt_id),
                next.delivery_id().expose_hex(),
                i64::from(attempt.get()),
                encode_attempt_state(AttemptState::Claimed),
                event::store_seq(seq)?,
                store_time(self.now),
            ],
        )?;
        set_delivery_state(tx, next.delivery_id(), next.state(), None, None)?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(ClaimedDelivery {
            delivery_id: next.delivery_id().clone(),
            revision: self.request.revision,
            attempt,
            created,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Arming the Send ambiguity fence immediately before the exact Send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmDeliverySendRequest {
    /// The wake being armed.
    pub delivery_id: DeliveryId,
    /// Binding generation the caller believes is current.
    pub binding_generation: BindingGeneration,
    /// Attempt the caller holds an I/O permit for.
    pub attempt: AttemptNo,
}

/// Arms the Send fence. A crash after this commit is recovered as ambiguous.
pub(crate) struct ArmDeliverySend {
    request: ArmDeliverySendRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for ArmDeliverySend {
    type Request = ArmDeliverySendRequest;
    type Committed = AttemptNo;
    type Output = AttemptNo;

    const NAME: &'static str = "arm_delivery_send";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let bindings = load::bindings(tx)?;
        bindings.fence(self.request.binding_generation)?;

        let loaded = load::wake_by_delivery_id(tx, &self.request.delivery_id)?;
        let wake = loaded.wake;
        if wake.binding_generation() != self.request.binding_generation {
            return Err(Conflict::StaleBindingGeneration {
                presented: self.request.binding_generation,
                active: wake.binding_generation(),
            }
            .into());
        }

        // Re-verified immediately before Send, exactly as the data model
        // requires: an obligation that moved on since scheduling is stale, and
        // this wake may never submit.
        let obligation = load::obligation(tx, wake.target().obligation)?;
        wake.require_current_target(&obligation.projection)?;

        let armed = wake.apply(&DeliveryEvent::ActivationArmed {
            attempt: self.request.attempt,
            at: self.now,
        })?;
        if armed.is_duplicate() {
            return Ok(self.request.attempt);
        }

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::BrowserDeliveryActivationArmed,
                source: crate::ops::internal_source_text(
                    &self.request.delivery_id.expose_hex(),
                    &format!("armed.{}", self.request.attempt),
                )?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(obligation.identity.task),
                    obligation: Some(obligation.projection.id()),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .int("revision", i64::from(wake.revision().get()))
                    .int("attempt_no", i64::from(self.request.attempt.get())),
            },
        )?
        .seq();

        tx.conn().execute(
            "UPDATE delivery_attempts
                SET state = ?3, activation_armed_event_seq = ?4
              WHERE delivery_id = ?1 AND attempt_no = ?2",
            params![
                self.request.delivery_id.expose_hex(),
                i64::from(self.request.attempt.get()),
                encode_attempt_state(AttemptState::ActivationArmed),
                event::store_seq(seq)?,
            ],
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(self.request.attempt)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// What an adapter observed after Send.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliveryOutcome {
    /// Exact evidence binding the wake to a provider user message.
    ///
    /// A weak UI signal cannot be expressed here: acceptance needs the exact
    /// provider message identity, and there is no variant that takes less.
    Accepted {
        /// Opaque provider-native identity of the submitted user message.
        message: ProviderMessageRef,
    },
    /// Proven not to have submitted anything.
    Failed {
        /// The proven pre-submit class.
        failure: FailureClass,
    },
    /// The outcome could not be determined.
    Ambiguous {
        /// Why the outcome was lost.
        reason: AmbiguityReason,
    },
}

/// Recording what an attempt actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordDeliveryOutcomeRequest {
    /// The wake the attempt belongs to.
    pub delivery_id: DeliveryId,
    /// Attempt whose outcome is being recorded.
    pub attempt: AttemptNo,
    /// What was observed.
    pub outcome: DeliveryOutcome,
}

/// Records an attempt's outcome and freezes the revision when it must be.
pub(crate) struct RecordDeliveryOutcome {
    request: RecordDeliveryOutcomeRequest,
    event: EventId,
    /// Minted for the `foreman_unreachable` resolution an acceptance implies.
    /// Discarded when the outcome is not an acceptance, or none is open.
    resolution_event: EventId,
    now: Timestamp,
}

impl WriteOp for RecordDeliveryOutcome {
    type Request = RecordDeliveryOutcomeRequest;
    type Committed = DeliveryState;
    type Output = DeliveryState;

    const NAME: &'static str = "record_delivery_outcome";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            resolution_event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::wake_by_delivery_id(tx, &self.request.delivery_id)?;
        let wake = loaded.wake;
        let obligation = load::obligation(tx, wake.target().obligation)?;

        let conversation = wake
            .delivery()
            .accepted_evidence()
            .map(AcceptedWakeEvidence::conversation)
            .cloned();
        let (kind, domain, metadata) = match &self.request.outcome {
            DeliveryOutcome::Accepted { message } => {
                // The conversation half of the evidence is the bound surface's,
                // never the adapter's to assert: it is read from the binding
                // this wake was created under.
                let conversation = match conversation {
                    Some(existing) => existing,
                    None => binding_conversation(tx, &wake)?,
                };
                (
                    EventKind::BrowserDeliveryAccepted,
                    DeliveryEvent::AttemptAccepted {
                        attempt: self.request.attempt,
                        evidence: AcceptedWakeEvidence::new(conversation, message.clone()),
                        at: self.now,
                    },
                    SafeMetadata::new().token("message_ref", message.as_token()),
                )
            }
            DeliveryOutcome::Failed { failure } => (
                EventKind::BrowserDeliveryFailed,
                DeliveryEvent::AttemptFailed {
                    attempt: self.request.attempt,
                    failure: *failure,
                    at: self.now,
                },
                SafeMetadata::new().label(
                    "failure_class",
                    encode_failure_class(*failure, "delivery_attempts")?,
                ),
            ),
            DeliveryOutcome::Ambiguous { reason } => (
                EventKind::BrowserDeliveryAmbiguous,
                DeliveryEvent::AttemptAmbiguous {
                    attempt: self.request.attempt,
                    reason: *reason,
                    at: self.now,
                },
                SafeMetadata::new().label(
                    "evidence_class",
                    encode_ambiguity(*reason, "delivery_attempts")?,
                ),
            ),
        };

        let transition = wake.apply(&domain)?;
        let Some(next) = transition.advanced() else {
            return Ok(wake.state());
        };

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind,
                source: crate::ops::internal_source_text(
                    &self.request.delivery_id.expose_hex(),
                    &format!("{}.{}", kind.label(), self.request.attempt),
                )?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(obligation.identity.task),
                    obligation: Some(obligation.projection.id()),
                    ..EventScope::default()
                },
                metadata: metadata
                    .int("revision", i64::from(wake.revision().get()))
                    .int("attempt_no", i64::from(self.request.attempt.get())),
            },
        )?
        .seq();

        let attempt_state = next
            .delivery()
            .attempts()
            .iter()
            .find(|candidate| candidate.number() == self.request.attempt)
            .ok_or(Conflict::UnknownAttempt {
                attempt: self.request.attempt,
            })?;
        tx.conn().execute(
            "UPDATE delivery_attempts
                SET state = ?3, terminal_event_seq = ?4, finished_at_ms = ?5,
                    failure_class = ?6, evidence_class = ?7
              WHERE delivery_id = ?1 AND attempt_no = ?2",
            params![
                self.request.delivery_id.expose_hex(),
                i64::from(self.request.attempt.get()),
                encode_attempt_state(attempt_state.state()),
                event::store_seq(seq)?,
                store_time(self.now),
                attempt_state
                    .failure()
                    .map(|failure| encode_failure_class(failure, "delivery_attempts"))
                    .transpose()?,
                attempt_state
                    .ambiguity()
                    .map(|reason| encode_ambiguity(reason, "delivery_attempts"))
                    .transpose()?,
            ],
        )?;

        let accepted_message = match &self.request.outcome {
            DeliveryOutcome::Accepted { message } => Some(message.as_token().as_str().to_owned()),
            _ => None,
        };
        set_delivery_state(
            tx,
            next.delivery_id(),
            next.state(),
            accepted_message.as_deref(),
            Some(seq),
        )?;

        // A wake that landed is evidence the foreman *was* reachable, so it
        // closes the attention that said otherwise. Attention only — nothing
        // here touches the obligation.
        if accepted_message.is_some() {
            crate::ops::health::resolve_on_acceptance(
                tx,
                obligation.projection.id(),
                self.resolution_event,
                self.now,
            )?;
        }

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(next.state())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Exact later evidence that an ambiguous revision did in fact submit.
///
/// Every field is a fence. There is no variant that takes less, and in
/// particular there is none that takes an absence: `docs/state-machines.md`
/// "Ambiguous reconciliation" — *absence is not proof of no submission*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileAmbiguousDeliveryRequest {
    /// The random correlation ID of the ambiguous revision.
    ///
    /// Possession of this value is the only way to name a delivery, exactly as
    /// it is for `foreman_resume`; a deterministic key does not identify one.
    pub delivery_id: DeliveryId,
    /// Binding generation the evidence was observed under. Must be current.
    pub binding_generation: BindingGeneration,
    /// Conversation the message was observed in. Must be the bound one.
    pub conversation: governor_core::binding::ConversationRef,
    /// Provider-native identity of the user message that was found.
    pub message: ProviderMessageRef,
}

/// Promotes an ambiguous revision to accepted on exact evidence. No Send.
///
/// `docs/testing.md` DEL-015 and `docs/state-machines.md` "Ambiguous
/// reconciliation". Four properties, and each is structural rather than
/// asserted:
///
/// - **no external effect.** This operation is a transaction body, so it has no
///   port to reach an adapter through, and it hands back no
///   [`governor_core::outbound::IoPermit`] — the promoted revision's last
///   attempt is `accepted`, and [`governor_core::outbound::Delivery::io_permit`]
///   yields `Some` only for a *live* attempt.
/// - **still frozen.** `accepted` satisfies
///   [`governor_core::outbound::DeliveryState::is_frozen`], so a later claim on
///   this revision is refused by the same rule that refuses one on an accepted
///   wake.
/// - **exact evidence only.** The conversation must be the delivery's bound
///   one, and the correlation ID must be the delivery's own. A caller that has
///   neither cannot name a delivery at all.
/// - **replay-foldable.** The event is folded back through
///   [`DeliveryEvent::ReconciledAccepted`] like every other delivery fact, so
///   DB-001 equivalence covers the promotion.
///
/// # Undifferentiated refusals
///
/// A wrong correlation ID and a wrong conversation both report
/// [`Conflict::UnknownDeliveryId`]. Together those two values *are* how a
/// delivery is named for reconciliation, and a caller probing one of them must
/// not learn which half it got right.
pub(crate) struct ReconcileAmbiguousDelivery {
    request: ReconcileAmbiguousDeliveryRequest,
    event: EventId,
    /// Minted for the `foreman_unreachable` resolution an acceptance implies.
    /// Discarded when no such condition is open.
    resolution_event: EventId,
    now: Timestamp,
}

impl WriteOp for ReconcileAmbiguousDelivery {
    type Request = ReconcileAmbiguousDeliveryRequest;
    type Committed = DeliveryState;
    type Output = DeliveryState;

    const NAME: &'static str = "reconcile_ambiguous_delivery";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            resolution_event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let bindings = load::bindings(tx)?;
        let active = bindings.fence(self.request.binding_generation)?;

        let loaded = load::wake_by_delivery_id(tx, &self.request.delivery_id)?;
        let wake = loaded.wake;
        if wake.binding_generation() != self.request.binding_generation {
            return Err(Conflict::StaleBindingGeneration {
                presented: self.request.binding_generation,
                active: wake.binding_generation(),
            }
            .into());
        }
        if active.conversation() != &self.request.conversation {
            return Err(Conflict::UnknownDeliveryId.into());
        }

        let obligation = load::obligation(tx, wake.target().obligation)?;
        let transition = wake.apply(&DeliveryEvent::ReconciledAccepted {
            // The conversation half is read from the binding, never taken from
            // the caller's copy, so the stored evidence is the surface's own.
            evidence: AcceptedWakeEvidence::new(
                active.conversation().clone(),
                self.request.message.clone(),
            ),
            at: self.now,
        })?;
        let Some(next) = transition.advanced() else {
            return Ok(wake.state());
        };

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::BrowserDeliveryReconciled,
                source: crate::ops::internal_source_text(
                    &self.request.delivery_id.expose_hex(),
                    &format!("reconciled.{}", wake.revision()),
                )?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(obligation.identity.task),
                    obligation: Some(obligation.projection.id()),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .int("revision", i64::from(wake.revision().get()))
                    .token("message_ref", self.request.message.as_token()),
            },
        )?
        .seq();

        // Exactly the attempt the machine promoted, and only from `ambiguous`.
        for attempt in next.delivery().attempts() {
            if attempt.state() != AttemptState::Accepted {
                continue;
            }
            tx.conn().execute(
                "UPDATE delivery_attempts
                    SET state = ?3, evidence_class = NULL, finished_at_ms = ?4
                  WHERE delivery_id = ?1 AND attempt_no = ?2 AND state = 'ambiguous'",
                params![
                    self.request.delivery_id.expose_hex(),
                    i64::from(attempt.number().get()),
                    encode_attempt_state(AttemptState::Accepted),
                    store_time(self.now),
                ],
            )?;
        }
        set_delivery_state(
            tx,
            next.delivery_id(),
            next.state(),
            Some(self.request.message.as_token().as_str()),
            Some(seq),
        )?;

        // An acceptance is an acceptance however it was proven.
        crate::ops::health::resolve_on_acceptance(
            tx,
            obligation.projection.id(),
            self.resolution_event,
            self.now,
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(next.state())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

fn binding_conversation(
    tx: &Tx<'_>,
    wake: &BrowserWake,
) -> StoreResult<governor_core::binding::ConversationRef> {
    let bindings = load::bindings(tx)?;
    let active = bindings.fence(wake.binding_generation())?;
    Ok(active.conversation().clone())
}

/// Writes the aggregate delivery projection.
pub(crate) fn set_delivery_state(
    tx: &Tx<'_>,
    delivery_id: &DeliveryId,
    state: DeliveryState,
    accepted_message: Option<&str>,
    terminal_seq: Option<governor_core::fence::EventSeq>,
) -> StoreResult<()> {
    let terminal = terminal_seq
        .filter(|_| state.is_terminal())
        .map(event::store_seq)
        .transpose()?;
    let accepted = terminal.filter(|_| state == DeliveryState::Accepted);
    tx.conn().execute(
        "UPDATE browser_deliveries
            SET state = ?2,
                accepted_message_ref = COALESCE(?3, accepted_message_ref),
                accepted_event_seq = COALESCE(accepted_event_seq, ?4),
                terminal_event_seq = COALESCE(terminal_event_seq, ?5)
          WHERE delivery_id = ?1",
        params![
            delivery_id.expose_hex(),
            encode_delivery_state(state),
            accepted_message,
            accepted,
            terminal,
        ],
    )?;
    Ok(())
}

/// The digest of the wake payload this revision will carry.
///
/// Deterministic in the *already-created* random correlation ID plus the
/// scheduling tuple, exactly as `docs/data-model.md` describes: "the wake
/// payload is deterministic given the already-created random delivery ID;
/// SQLite stores its digest, not worker output". Computing it here rather than
/// accepting it from a caller means no caller can put worker output in this
/// column even by accident.
fn wake_payload_digest(
    delivery_id: &DeliveryId,
    obligation: ObligationId,
    generation: BindingGeneration,
    revision: DeliveryRevision,
    protocol: &SafeToken,
) -> String {
    let mut hasher = Sha256::new();
    let mut absorb = |bytes: &[u8]| {
        let len = u64::try_from(bytes.len()).expect("bounded length fits in u64");
        hasher.update(len.to_be_bytes());
        hasher.update(bytes);
    };
    absorb(WAKE_PAYLOAD_DOMAIN.as_bytes());
    absorb(delivery_id.expose_bytes());
    absorb(obligation.as_uuid().as_bytes());
    absorb(&generation.get().to_be_bytes());
    absorb(&revision.get().to_be_bytes());
    absorb(protocol.as_str().as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    hex32(&digest)
}

/// Builds a wake revision around a correlation ID the store already drew.
///
/// `BrowserWake::create` draws its own correlation ID from a [`SecureRandom`],
/// which a transaction body cannot reach. This goes through the crate's
/// validating rehydration path instead, with a freshly drawn ID and an empty
/// attempt machine — the same code path a reopened wake takes, so the
/// deterministic key is still re-derived and checked.
trait CreateForStore: Sized {
    fn create_at_revision_for_store(
        delivery_id: DeliveryId,
        target: WakeTarget,
        binding: governor_core::id::ForemanBindingId,
        generation: BindingGeneration,
        revision: DeliveryRevision,
        attempt_budget: u32,
    ) -> StoreResult<Self>;
}

impl CreateForStore for BrowserWake {
    fn create_at_revision_for_store(
        delivery_id: DeliveryId,
        target: WakeTarget,
        binding: governor_core::id::ForemanBindingId,
        generation: BindingGeneration,
        revision: DeliveryRevision,
        attempt_budget: u32,
    ) -> StoreResult<Self> {
        Self::rehydrate(governor_core::delivery::PersistedWake {
            delivery_id,
            delivery_key: DeliveryKey::derive(target.obligation, generation, revision),
            target,
            binding,
            binding_generation: generation,
            revision,
            delivery: governor_core::outbound::Delivery::pending(attempt_budget),
        })
        .map_err(|_| {
            CorruptValue::new(
                "browser_deliveries",
                "delivery_key",
                CorruptReason::MalformedIdentity,
            )
            .into()
        })
    }
}
