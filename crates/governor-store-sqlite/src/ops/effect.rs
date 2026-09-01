//! The durable-intent protocol for consequential external effects.
//!
//! ```text
//! BEGIN IMMEDIATE
//!   insert unique intent row
//! COMMIT
//! -> only now: RecordedIntent::accept_committed
//! -> EffectDecision::Execute(ExternalExecutionPermit)
//! ```
//!
//! # The three obligations `governor-core` states, and how each is upheld
//!
//! [`governor_core::effect::DurableIntentAccepted`] documents exactly what a store asserts when it
//! calls `accept_committed`. Each assertion has a structural counterpart here.
//!
//! **1. The intent row is committed before the acceptance.** `accept_committed`
//! is called from [`WriteOp::finish`], and the runner in [`crate::writer`]
//! calls `finish` only after `Transaction::commit()` returned `Ok`. It is not
//! reachable from `commit`, which does not own the operation by value. So the
//! ordering is a property of the trait, not of anyone remembering it.
//!
//! **2. The commit is unique for the attempt identity.**
//! `external_attempts.external_attempt_id` is the primary key and the insert is
//! a plain `INSERT`, not an upsert. A crash-and-retry that reuses an attempt
//! identity hits the constraint and gets a typed conflict, so one logical
//! operation cannot produce two intent rows and therefore cannot produce two
//! permits.
//!
//! **3. The permit is handed on exactly once.** `finish(self, …)` consumes the
//! operation by value, moving the [`RecordedIntent`] out; `accept_committed`
//! consumes that; [`ExternalAttempt::decide`] consumes the acceptance; and
//! [`ExternalExecutionPermit`] is neither `Clone` nor `Copy`. The chain is
//! by-value from end to end, and the permit leaves through the reply channel
//! once.
//!
//! # Retry goes through a *new* attempt
//!
//! `ambiguous` is terminal. There is no operation here that resolves it, and
//! [`ResolveExternalAttempt`] answers [`EffectDecision::Reconcile`] rather than
//! offering a permit. Opening the next attempt is a fresh
//! [`RecordExternalIntent`], which the caller may only do after
//! [`ExternalAttempt::admit_retry`] agrees.

use governor_core::effect::{
    DestinationRef, EffectAmbiguityReason, EffectDecision, ExternalAttempt, ExternalAttemptEvent,
    ExternalAttemptState, ExternalEffectClass, ExternalExecutionPermit, NoEffectClass,
    RecordedIntent,
};
use governor_core::error::Conflict;
use governor_core::fence::{DaemonEpoch, SourceRef};
use governor_core::id::ExternalAttemptId;
use governor_core::time::Timestamp;
use rusqlite::{OptionalExtension as _, params};

use crate::codec::{
    decode_attempt_effect_state, decode_effect_ambiguity, decode_effect_class, decode_no_effect,
    encode_attempt_effect_state, encode_effect_ambiguity, encode_effect_class, encode_no_effect,
    id_text, parse_source, parse_token, parse_u64, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::ops::AttemptEvidence;
use crate::ports::StorePorts;
use crate::tx::{Failpoint, Tx, WriteOp};

const TABLE: &str = "external_attempts";

/// A consequential external effect the daemon is about to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordExternalIntentRequest {
    /// How consequential the call is, with its exact idempotency key.
    pub class: ExternalEffectClass,
    /// Opaque destination the call targets.
    pub destination: DestinationRef,
    /// The source fact that justifies the effect.
    pub source: SourceRef,
    /// Daemon epoch the intent is recorded under.
    pub daemon_epoch: DaemonEpoch,
}

/// A durable intent and the single-use capability it authorises.
///
/// Not `Clone`: it carries an [`ExternalExecutionPermit`], and one durable
/// intent authorises one call.
#[derive(Debug)]
pub struct GrantedPermit {
    /// The attempt whose intent is now durable.
    pub attempt: ExternalAttemptId,
    /// Permission to perform exactly one consequential external call.
    pub permit: ExternalExecutionPermit,
}

/// Commits one intent row, then surrenders one permit.
pub(crate) struct RecordExternalIntent {
    request: RecordExternalIntentRequest,
    attempt: ExternalAttemptId,
    /// Taken by value in `finish`, which the runner calls only after `COMMIT`.
    recorded: RecordedIntent<AttemptEvidence>,
}

impl WriteOp for RecordExternalIntent {
    type Request = RecordExternalIntentRequest;
    type Committed = ();
    type Output = GrantedPermit;

    const NAME: &'static str = "record_external_intent";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        let attempt: ExternalAttemptId = ports.next_id();
        let recorded = ExternalAttempt::<AttemptEvidence>::record_intent(
            attempt,
            request.class.clone(),
            request.destination.clone(),
            request.source.clone(),
            request.daemon_epoch,
            ports.now(),
        );
        Ok(Self {
            request,
            attempt,
            recorded,
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        insert_intent(
            tx,
            self.attempt,
            &self.request.class,
            &self.request.destination,
            &self.request.source,
            self.request.daemon_epoch,
            self.recorded.attempt().recorded_at(),
        )?;
        tx.reach(Failpoint::AfterIntentInsert)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(())
    }

    fn finish(self, (): Self::Committed) -> Self::Output {
        // Reached only after `Transaction::commit()` returned `Ok`. This one
        // line is where the store asserts the durability `governor-core` cannot
        // check for itself.
        grant(self.attempt, self.recorded)
    }
}

/// Writes the one intent row that must commit before any consequential call.
///
/// A plain `INSERT` against the primary key: reusing an attempt identity is a
/// constraint violation, not an upsert. That is obligation 2 of
/// [`governor_core::effect::DurableIntentAccepted`], and it is shared rather
/// than copied because every operation that hands out a permit has to uphold
/// it identically — [`RecordExternalIntent`] and
/// [`crate::ops::session::AuthorizeWorkerSpawn`] both write through here, so
/// there is one statement and one set of columns to review.
pub(crate) fn insert_intent(
    tx: &Tx<'_>,
    attempt: ExternalAttemptId,
    class: &ExternalEffectClass,
    destination: &DestinationRef,
    source: &SourceRef,
    daemon_epoch: DaemonEpoch,
    recorded_at: Timestamp,
) -> StoreResult<()> {
    let encoded = encode_effect_class(class)?;
    tx.conn().execute(
        "INSERT INTO external_attempts (external_attempt_id, effect_class,
                idempotency_contract, idempotency_window_ms, idempotency_key,
                destination_namespace, destination_endpoint, destination_fence,
                source_namespace, source_event_id, source_event_fence,
                daemon_epoch, state, dispatched, recorded_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, ?14)",
        params![
            id_text(attempt),
            encoded.class,
            encoded.contract,
            encoded.window_ms,
            encoded.key,
            destination.namespace().as_str(),
            destination.endpoint().as_str(),
            destination.fence().as_str(),
            source.namespace().as_str(),
            source.event().as_str(),
            source.fence().as_str(),
            store_u64(daemon_epoch.get(), TABLE, "daemon_epoch")?,
            encode_attempt_effect_state(ExternalAttemptState::IntentRecorded),
            store_time(recorded_at),
        ],
    )?;
    Ok(())
}

/// Turns a freshly committed intent into the single permit it authorises.
///
/// Callable only from a [`WriteOp::finish`], which the runner in
/// [`crate::writer`] reaches only after `Transaction::commit()` returned `Ok`.
/// `recorded` is taken by value, so the acceptance — and therefore the permit —
/// moves out exactly once.
pub(crate) fn grant(
    attempt: ExternalAttemptId,
    recorded: RecordedIntent<AttemptEvidence>,
) -> GrantedPermit {
    let (projection, acceptance) = recorded.accept_committed();
    let decision = projection
        .decide(Some(acceptance), AttemptEvidence::clone)
        .expect("a freshly committed, undispatched intent always decides to execute");
    let EffectDecision::Execute(permit) = decision else {
        unreachable!("a freshly committed intent has no recorded outcome to replay")
    };
    GrantedPermit { attempt, permit }
}

/// Committing the dispatch fence immediately before the adapter issues a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkExternalDispatchedRequest {
    /// The attempt about to be dispatched.
    pub attempt: ExternalAttemptId,
}

/// Commits the dispatch fence. A crash after this is ambiguous, never success.
pub(crate) struct MarkExternalDispatched {
    request: MarkExternalDispatchedRequest,
    now: Timestamp,
}

impl WriteOp for MarkExternalDispatched {
    type Request = MarkExternalDispatchedRequest;
    type Committed = ExternalAttemptState;
    type Output = ExternalAttemptState;

    const NAME: &'static str = "mark_external_dispatched";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let attempt = require(tx, self.request.attempt)?;
        let transition = attempt.apply(&ExternalAttemptEvent::CallDispatched { at: self.now })?;
        let Some(next) = transition.advanced() else {
            return Ok(attempt.state());
        };
        tx.conn().execute(
            "UPDATE external_attempts SET dispatched = 1, dispatched_at_ms = ?2
              WHERE external_attempt_id = ?1",
            params![
                id_text(self.request.attempt),
                next.dispatched_at().map(store_time),
            ],
        )?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(next.state())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// What an adapter learned about a dispatched call.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExternalOutcome {
    /// Exact evidence proves the effect landed.
    Completed {
        /// The opaque reference the destination returned.
        evidence: AttemptEvidence,
    },
    /// Proof establishes the effect did not happen.
    FailedBeforeEffect {
        /// The proof class. Every variant is a proof; there is no weak one.
        proof: NoEffectClass,
    },
    /// The fate of the effect is unknown. Terminal, and never auto-retried.
    Ambiguous {
        /// Why the fate was lost.
        reason: EffectAmbiguityReason,
    },
}

/// Recording what became of a dispatched external call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordExternalOutcomeRequest {
    /// Attempt whose fate is being recorded.
    pub attempt: ExternalAttemptId,
    /// What was learned.
    pub outcome: ExternalOutcome,
}

/// Records an attempt's terminal state.
pub(crate) struct RecordExternalOutcome {
    request: RecordExternalOutcomeRequest,
    now: Timestamp,
}

impl WriteOp for RecordExternalOutcome {
    type Request = RecordExternalOutcomeRequest;
    type Committed = ExternalAttemptState;
    type Output = ExternalAttemptState;

    const NAME: &'static str = "record_external_outcome";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let attempt = require(tx, self.request.attempt)?;
        let event = match &self.request.outcome {
            ExternalOutcome::Completed { evidence } => ExternalAttemptEvent::Completed {
                evidence: evidence.clone(),
                at: self.now,
            },
            ExternalOutcome::FailedBeforeEffect { proof } => {
                ExternalAttemptEvent::FailedBeforeEffect {
                    proof: *proof,
                    at: self.now,
                }
            }
            ExternalOutcome::Ambiguous { reason } => ExternalAttemptEvent::OutcomeUnknown {
                reason: *reason,
                at: self.now,
            },
        };
        let transition = attempt.apply(&event)?;
        let Some(next) = transition.advanced() else {
            return Ok(attempt.state());
        };
        write_terminal(tx, self.request.attempt, &next)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(next.state())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Writes an attempt's terminal columns.
pub(crate) fn write_terminal(
    tx: &Tx<'_>,
    attempt: ExternalAttemptId,
    next: &ExternalAttempt<AttemptEvidence>,
) -> StoreResult<()> {
    tx.conn().execute(
        "UPDATE external_attempts
            SET state = ?2, completion_ref = ?3, no_effect_class = ?4,
                ambiguity_reason = ?5, finished_at_ms = ?6
          WHERE external_attempt_id = ?1",
        params![
            id_text(attempt),
            encode_attempt_effect_state(next.state()),
            next.outcome().map(|e| e.as_token().as_str()),
            next.no_effect().map(encode_no_effect),
            next.ambiguity().map(encode_effect_ambiguity),
            next.finished_at().map(store_time),
        ],
    )?;
    Ok(())
}

/// Reads one attempt, or a typed conflict when it does not exist.
///
/// An unknown attempt identity is reported as
/// [`Conflict::ExecuteRequiresDurableIntent`]: from the caller's side there is
/// no durable intent for it, which is exactly true and is the answer that
/// forbids the same things.
pub(crate) fn require(
    tx: &Tx<'_>,
    attempt: ExternalAttemptId,
) -> StoreResult<ExternalAttempt<AttemptEvidence>> {
    read(tx, attempt)?.ok_or_else(|| Conflict::ExecuteRequiresDurableIntent { attempt }.into())
}

/// Reads and re-proves one attempt row.
///
/// Rebuilt by folding its own recorded history through [`ExternalAttempt`] —
/// intent, then any dispatch fence, then any outcome — and the folded state
/// must equal the stored one. A row that no legal sequence of transitions can
/// reach is corrupt and fails closed.
///
/// # Errors
///
/// Returns a SQLite error, a corrupt-row error, or a conflict when the recorded
/// history is not a legal sequence.
pub(crate) fn read(
    tx: &Tx<'_>,
    attempt: ExternalAttemptId,
) -> StoreResult<Option<ExternalAttempt<AttemptEvidence>>> {
    type Row = (
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        String,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        Option<i64>,
        Option<i64>,
    );
    let row: Option<Row> = tx
        .conn()
        .query_row(
            "SELECT effect_class, idempotency_contract, idempotency_window_ms, idempotency_key,
                    destination_namespace, destination_endpoint, destination_fence,
                    source_namespace, source_event_id, source_event_fence,
                    daemon_epoch, state, dispatched, completion_ref, no_effect_class,
                    ambiguity_reason, recorded_at_ms, dispatched_at_ms, finished_at_ms
               FROM external_attempts WHERE external_attempt_id = ?1",
            params![id_text(attempt)],
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
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                    row.get(17)?,
                    row.get(18)?,
                ))
            },
        )
        .optional()?;
    let Some((
        class,
        contract,
        window,
        key,
        destination_namespace,
        destination_endpoint,
        destination_fence,
        source_namespace,
        source_event,
        source_fence,
        epoch,
        state,
        dispatched,
        completion,
        no_effect,
        ambiguity,
        recorded_at,
        dispatched_at,
        finished_at,
    )) = row
    else {
        return Ok(None);
    };

    let stored_state = decode_attempt_effect_state(&state, TABLE)?;
    // The projection is *cloned* out of the recorded intent rather than taken
    // through `accept_committed`: this is a read, and it must not be able to
    // produce an acceptance — and therefore a permit — for a row it is only
    // looking at.
    let projection = ExternalAttempt::<AttemptEvidence>::record_intent(
        attempt,
        decode_effect_class(&class, contract.as_deref(), window, key.as_deref())?,
        DestinationRef::new(
            parse_token(&destination_namespace, TABLE, "destination_namespace")?,
            parse_token(&destination_endpoint, TABLE, "destination_endpoint")?,
            parse_token(&destination_fence, TABLE, "destination_fence")?,
        ),
        parse_source(&source_namespace, &source_event, &source_fence)?,
        DaemonEpoch::new(parse_u64(epoch, TABLE, "daemon_epoch")?),
        Timestamp::from_unix_millis(recorded_at),
    )
    .attempt()
    .clone();

    let mut folded = projection;
    if dispatched != 0 {
        let at = dispatched_at.ok_or_else(|| {
            CorruptValue::new(TABLE, "dispatched_at_ms", CorruptReason::MalformedMetadata)
        })?;
        folded = folded
            .apply(&ExternalAttemptEvent::CallDispatched {
                at: Timestamp::from_unix_millis(at),
            })?
            .or_unchanged(folded);
    }
    if let Some(at) = finished_at {
        let at = Timestamp::from_unix_millis(at);
        let event = match stored_state {
            ExternalAttemptState::Completed => ExternalAttemptEvent::Completed {
                evidence: AttemptEvidence::new(parse_token(
                    completion.as_deref().unwrap_or_default(),
                    TABLE,
                    "completion_ref",
                )?),
                at,
            },
            ExternalAttemptState::FailedBeforeEffect => ExternalAttemptEvent::FailedBeforeEffect {
                proof: decode_no_effect(no_effect.as_deref().unwrap_or_default(), TABLE)?,
                at,
            },
            ExternalAttemptState::Ambiguous => ExternalAttemptEvent::OutcomeUnknown {
                reason: decode_effect_ambiguity(ambiguity.as_deref().unwrap_or_default(), TABLE)?,
                at,
            },
            ExternalAttemptState::IntentRecorded => {
                return Err(
                    CorruptValue::new(TABLE, "state", CorruptReason::UnprovableEvidence).into(),
                );
            }
        };
        folded = folded.apply(&event)?.or_unchanged(folded);
    }
    if folded.state() != stored_state {
        return Err(CorruptValue::new(TABLE, "state", CorruptReason::UnprovableEvidence).into());
    }
    Ok(Some(folded))
}

/// Resolves an attempt without offering a permit.
///
/// The replay half of the deterministic execution seam: a completed attempt
/// replays its recorded evidence, an ambiguous one reconciles. `None` is passed
/// for the acceptance, so [`EffectDecision::Execute`] is unreachable from here —
/// live permission comes only from a fresh [`RecordExternalIntent`].
///
/// # Errors
///
/// Returns the conflict [`ExternalAttempt::decide`] raises, or a corrupt-row
/// error.
pub(crate) fn resolve(
    tx: &Tx<'_>,
    attempt: ExternalAttemptId,
) -> StoreResult<EffectDecision<AttemptEvidence>> {
    let found = require(tx, attempt)?;
    Ok(found.decide(None, AttemptEvidence::clone)?)
}

/// Every attempt from an older daemon epoch whose fate was never proven.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an unfoldable row.
pub(crate) fn unresolved_before(
    tx: &Tx<'_>,
    epoch: DaemonEpoch,
) -> StoreResult<Vec<ExternalAttempt<AttemptEvidence>>> {
    let mut statement = tx.conn().prepare(
        "SELECT external_attempt_id FROM external_attempts
          WHERE state = 'intent_recorded' AND daemon_epoch < ?1",
    )?;
    let rows = statement.query_map(
        params![store_u64(epoch.get(), TABLE, "daemon_epoch")?],
        |row| row.get::<_, String>(0),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let id = crate::codec::parse_id(&row?, TABLE, "external_attempt_id")?;
        if let Some(found) = read(tx, id)? {
            out.push(found);
        }
    }
    Ok(out)
}
