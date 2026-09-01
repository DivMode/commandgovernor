//! Durable health conditions: attention, and never anything more.
//!
//! `docs/data-model.md`: *a health condition never pretends to be worker
//! completion*. Nothing in this module closes an obligation, releases an
//! artifact, moves a turn, or schedules a wake — and structurally it cannot,
//! because the only projection row it writes is `health_conditions` and the
//! only domain machine it drives is [`governor_core::health::HealthLedger`],
//! which has no path to any other state.
//!
//! # One shape, three triggers
//!
//! | Operation | Kind | Scope | Resolved by |
//! | --- | --- | --- | --- |
//! | [`RaiseForemanUnreachable`] | `foreman_unreachable` | obligation | a later accepted delivery for it |
//! | [`RaiseResultArtifactMissing`] | `result_artifact_missing` | obligation | [`ResolveResultArtifactMissing`], after a successful verify |
//! | [`RecordTerminalEvidenceConflict`] | whatever the arbitration says, `runtime_state_conflict` for the documented case | turn | an explicit human decision, which Phase 1 does not automate |
//!
//! # Why the ledger event carries the scope and nothing else
//!
//! A condition's identity is `(kind, scope)` — that is what
//! [`governor_core::health::HealthLedger::raise`] deduplicates on, and what the
//! partial unique index `health_conditions_one_open_per_scope` enforces
//! durably. So the event's own
//! scope columns *are* the scope, the one allowlisted metadata field is the
//! kind, and [`crate::load::fold_health`] rebuilds the whole ledger from those
//! two facts. There is no second copy of either to drift.
//!
//! # The row is the fence, the fold is the proof
//!
//! Deduplication reads the projection row through the unique index, because
//! that index is what makes a second open condition impossible rather than
//! merely unlikely. Whenever a write is actually going to happen, the ledger is
//! folded as well and the transition must agree; a row and a ledger that
//! disagree are a corrupt state root, not something to write on top of.

use governor_core::error::Conflict;
use governor_core::health::{HealthConditionKind, HealthConditionState, HealthScope};
use governor_core::id::{EventId, HealthConditionId, ObligationId, ResultArtifactId, SessionId};
use governor_core::time::Timestamp;
use governor_core::worker_evidence::WorkerOutcome;
use rusqlite::{OptionalExtension as _, params};

use crate::codec::{encode_health_kind, encode_health_state, id_text};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::load;
use crate::ops::internal_source;
use crate::ops::worker::CompletionReceipts;
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// What a health-condition write committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthConditionRecorded {
    /// Kind of attention.
    pub kind: HealthConditionKind,
    /// What the condition is about.
    pub scope: HealthScope,
    /// Whether a matching condition was already in that state, so this call
    /// changed nothing at all.
    pub duplicate: bool,
}

/// The resume budget for one obligation is spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaiseForemanUnreachableRequest {
    /// Obligation whose automatic wake budget ran out.
    pub obligation: ObligationId,
}

/// Opens `foreman_unreachable` for an obligation whose wake budget is spent.
///
/// `docs/testing.md` GPT-006: after the configured number of automatic resumes
/// there is exactly one condition, the obligation stays open indefinitely, and
/// nothing here reschedules anything.
pub(crate) struct RaiseForemanUnreachable {
    request: RaiseForemanUnreachableRequest,
    condition: HealthConditionId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RaiseForemanUnreachable {
    type Request = RaiseForemanUnreachableRequest;
    type Committed = HealthConditionRecorded;
    type Output = HealthConditionRecorded;

    const NAME: &'static str = "raise_foreman_unreachable";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            condition: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        require_open(&loaded.projection)?;
        let recorded = raise(
            tx,
            self.condition,
            HealthConditionKind::ForemanUnreachable,
            HealthScope::obligation(self.request.obligation),
            self.event,
            self.now,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(recorded)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// An artifact an open obligation requires could not be read or verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultArtifactMissingRequest {
    /// Obligation that still requires the artifact.
    pub obligation: ObligationId,
    /// The artifact whose bytes could not be proven.
    pub artifact: ResultArtifactId,
}

/// Opens `result_artifact_missing` for an artifact an open obligation pins.
///
/// `docs/testing.md` DB-008: a restore that lost the artifact root must enter
/// an explicit health/repair state and must not pretend the obligation is
/// processable or closed. This is that state; it changes no obligation.
pub(crate) struct RaiseResultArtifactMissing {
    request: ResultArtifactMissingRequest,
    condition: HealthConditionId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RaiseResultArtifactMissing {
    type Request = ResultArtifactMissingRequest;
    type Committed = HealthConditionRecorded;
    type Output = HealthConditionRecorded;

    const NAME: &'static str = "raise_result_artifact_missing";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            condition: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        require_open(&loaded.projection)?;
        require_requires_artifact(&loaded.projection, self.request.artifact)?;
        let recorded = raise(
            tx,
            self.condition,
            HealthConditionKind::ResultArtifactMissing,
            HealthScope::obligation(self.request.obligation),
            self.event,
            self.now,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(recorded)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// The artifact was read back and verified after all.
pub(crate) struct ResolveResultArtifactMissing {
    request: ResultArtifactMissingRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for ResolveResultArtifactMissing {
    type Request = ResultArtifactMissingRequest;
    type Committed = HealthConditionRecorded;
    type Output = HealthConditionRecorded;

    const NAME: &'static str = "resolve_result_artifact_missing";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        // Deliberately not fenced on the obligation still being open: an
        // artifact that came back is good news whatever became of the work.
        let loaded = load::obligation(tx, self.request.obligation)?;
        require_requires_artifact(&loaded.projection, self.request.artifact)?;
        let recorded = resolve(
            tx,
            HealthConditionKind::ResultArtifactMissing,
            HealthScope::obligation(self.request.obligation),
            self.event,
            self.now,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(recorded)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Contradictory terminal evidence for a turn that already has a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEvidenceConflictRequest {
    /// Obligation whose turn already holds a confirmed terminal result.
    pub obligation: ObligationId,
    /// The bounded safe receipts that contradict it. Arbitrated here.
    pub receipts: CompletionReceipts,
}

/// Records that a turn's terminal evidence contradicts its confirmed result.
///
/// `docs/testing.md` OBL-006. Two things make this attention rather than a
/// decision:
///
/// - the arbitration is [`governor_core::worker_evidence::ManagedRunEvidence`]'s,
///   not the caller's — only [`WorkerOutcome::NeedsReconciliation`] opens a
///   condition, so a caller cannot hand in a conclusion;
/// - what is stored is the *class* the arbitration returned and the turn it is
///   about. No receipt, no payload, and no second obligation.
pub(crate) struct RecordTerminalEvidenceConflict {
    request: TerminalEvidenceConflictRequest,
    condition: HealthConditionId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RecordTerminalEvidenceConflict {
    type Request = TerminalEvidenceConflictRequest;
    type Committed = HealthConditionRecorded;
    type Output = HealthConditionRecorded;

    const NAME: &'static str = "record_terminal_evidence_conflict";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            condition: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        let state = loaded.projection.state();

        // There must be something to contradict. An obligation with no
        // published result has no confirmed terminal fact, so a second terminal
        // report is ordinary evidence, not a conflict.
        if loaded.projection.result_artifact().is_none() {
            return Err(Conflict::IllegalObligationTransition {
                from: state,
                event: "terminal_evidence_conflict",
            }
            .into());
        }
        let turn = loaded.identity.turn.ok_or_else(|| {
            CorruptValue::new("obligations", "turn_id", CorruptReason::DanglingReference)
        })?;

        let WorkerOutcome::NeedsReconciliation(kind) = self.request.receipts.classify() else {
            // Not contradictory: a confirmed completion, a documented failure or
            // an inconclusive-but-consistent set is not a conflict, and calling
            // one attention would make the ledger lie.
            return Err(Conflict::IllegalObligationTransition {
                from: state,
                event: "terminal_evidence_conflict",
            }
            .into());
        };

        let recorded = raise(
            tx,
            self.condition,
            kind,
            HealthScope::turn(turn),
            self.event,
            self.now,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(recorded)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

// --- Session-scoped attention ------------------------------------------------

/// A logical session that needs attention before it can be resumed.
///
/// One request type for all three session-scoped kinds: they differ in *which*
/// evidence failed, never in what is recorded, and giving each its own
/// near-identical struct would be three places to keep in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionHealthRequest {
    /// Session whose launch state could not be proved.
    pub session: SessionId,
}

/// Generates the raise/resolve pair for one session-scoped condition kind.
///
/// Both halves are the same four lines around a different [`HealthConditionKind`],
/// and writing them out six times would be six chances to scope one to the
/// wrong thing.
macro_rules! session_condition {
    (
        $kind:path,
        raise = $raise:ident as $raise_name:literal,
        resolve = $resolve:ident as $resolve_name:literal,
        $(#[$raise_doc:meta])*
    ) => {
        $(#[$raise_doc])*
        pub(crate) struct $raise {
            request: SessionHealthRequest,
            condition: HealthConditionId,
            event: EventId,
            now: Timestamp,
        }

        impl WriteOp for $raise {
            type Request = SessionHealthRequest;
            type Committed = HealthConditionRecorded;
            type Output = HealthConditionRecorded;

            const NAME: &'static str = $raise_name;

            fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
                Ok(Self {
                    request,
                    condition: ports.next_id(),
                    event: ports.next_id(),
                    now: ports.now(),
                })
            }

            fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
                require_session(tx, self.request.session)?;
                let recorded = raise(
                    tx,
                    self.condition,
                    $kind,
                    HealthScope::session(self.request.session),
                    self.event,
                    self.now,
                )?;
                tx.reach(Failpoint::AfterProjectionUpdate)?;
                tx.reach(Failpoint::BeforeCommit)?;
                Ok(recorded)
            }

            fn finish(self, committed: Self::Committed) -> Self::Output {
                committed
            }
        }

        /// Closes the matching condition after the evidence verified again.
        pub(crate) struct $resolve {
            request: SessionHealthRequest,
            event: EventId,
            now: Timestamp,
        }

        impl WriteOp for $resolve {
            type Request = SessionHealthRequest;
            type Committed = HealthConditionRecorded;
            type Output = HealthConditionRecorded;

            const NAME: &'static str = $resolve_name;

            fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
                Ok(Self {
                    request,
                    event: ports.next_id(),
                    now: ports.now(),
                })
            }

            fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
                require_session(tx, self.request.session)?;
                let recorded = resolve(
                    tx,
                    $kind,
                    HealthScope::session(self.request.session),
                    self.event,
                    self.now,
                )?;
                tx.reach(Failpoint::AfterProjectionUpdate)?;
                tx.reach(Failpoint::BeforeCommit)?;
                Ok(recorded)
            }

            fn finish(self, committed: Self::Committed) -> Self::Output {
                committed
            }
        }
    };
}

session_condition! {
    HealthConditionKind::LoadoutUnverifiable,
    raise = RaiseLoadoutUnverifiable as "raise_loadout_unverifiable",
    resolve = ResolveLoadoutUnverifiable as "resolve_loadout_unverifiable",
    /// Opens `loadout_unverifiable` for a session whose launch row will not
    /// re-derive its own digest.
    ///
    /// Attention, not repair: the session stays exactly as it is, and resume is
    /// refused independently by `CommittedLoadout::rehydrate`.
}

session_condition! {
    HealthConditionKind::ManagedConfigMissing,
    raise = RaiseManagedConfigMissing as "raise_managed_config_missing",
    resolve = ResolveManagedConfigMissing as "resolve_managed_config_missing",
    /// Opens `managed_config_missing` for a session whose configuration bytes
    /// are gone, truncated or rewritten.
    ///
    /// The durable equivalent of `result_artifact_missing`, and scoped the same
    /// way: to the one thing that cannot proceed, never to the whole daemon.
}

session_condition! {
    HealthConditionKind::LineageBroken,
    raise = RaiseLineageBroken as "raise_lineage_broken",
    resolve = ResolveLineageBroken as "resolve_lineage_broken",
    /// Opens `lineage_broken` for a session whose ancestor walk does not
    /// terminate.
    ///
    /// Unreachable from any legal sequence of store operations — the
    /// edge-insert transaction refuses a cycle before it commits — so this
    /// exists for the graph a restore or a hand edit produced.
}

/// The session must exist before attention can be recorded about it.
fn require_session(tx: &Tx<'_>, session: SessionId) -> StoreResult<()> {
    let found: Option<i64> = tx
        .conn()
        .query_row(
            "SELECT 1 FROM sessions WHERE session_id = ?1",
            rusqlite::params![id_text(session)],
            |row| row.get(0),
        )
        .optional()?;
    if found.is_some() {
        Ok(())
    } else {
        Err(CorruptValue::new("sessions", "session_id", CorruptReason::DanglingReference).into())
    }
}

// --- Shared internals --------------------------------------------------------

/// Attention is for work somebody is still owed.
fn require_open(obligation: &governor_core::obligation::Obligation) -> StoreResult<()> {
    if obligation.state().is_closed() {
        return Err(Conflict::ObligationClosed {
            state: obligation.state(),
        }
        .into());
    }
    Ok(())
}

/// The condition must be about the artifact this obligation actually requires.
fn require_requires_artifact(
    obligation: &governor_core::obligation::Obligation,
    artifact: ResultArtifactId,
) -> StoreResult<()> {
    if obligation.result_artifact() == Some(artifact) {
        return Ok(());
    }
    Err(Conflict::IllegalObligationTransition {
        from: obligation.state(),
        event: "result_artifact_missing",
    }
    .into())
}

/// The scope columns one health scope occupies on an event.
const fn event_scope(scope: HealthScope) -> EventScope {
    EventScope {
        project: None,
        task: scope.task,
        session: scope.session,
        incarnation: None,
        turn: scope.turn,
        obligation: scope.obligation,
    }
}

/// Opens one condition, or reports that an identical one is already open.
fn raise(
    tx: &Tx<'_>,
    condition: HealthConditionId,
    kind: HealthConditionKind,
    scope: HealthScope,
    event_id: EventId,
    now: Timestamp,
) -> StoreResult<HealthConditionRecorded> {
    let recorded = |duplicate| HealthConditionRecorded {
        kind,
        scope,
        duplicate,
    };
    if load::open_condition_id(tx, kind, scope)?.is_some() {
        // One condition per (kind, scope), not one per timer tick. Nothing is
        // appended either, so a scheduler that asks every second leaves one
        // event rather than a million.
        return Ok(recorded(true));
    }

    let ledger = load::health_ledger(tx)?;
    if ledger.raise(condition, kind, scope, now)?.is_duplicate() {
        return Err(disagreement());
    }

    let seq = event::append(
        tx,
        &NewEvent {
            event_id,
            kind: EventKind::HealthConditionOpened,
            // Deterministic in the freshly minted condition identity, so a
            // repeated raise for a *new* scope never collides with an old one.
            source: internal_source(condition, "health_condition_opened")?,
            observed_at: now,
            occurred_at: None,
            scope: event_scope(scope),
            metadata: SafeMetadata::new().label("health_kind", encode_health_kind(kind)),
        },
    )?
    .seq();

    tx.conn().execute(
        "INSERT INTO health_conditions (health_condition_id, kind, state,
                task_id, session_id, turn_id, obligation_id, external_attempt_id,
                opened_event_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id_text(condition),
            encode_health_kind(kind),
            encode_health_state(HealthConditionState::Open),
            scope.task.map(id_text),
            scope.session.map(id_text),
            scope.turn.map(id_text),
            scope.obligation.map(id_text),
            scope.external_attempt.map(id_text),
            event::store_seq(seq)?,
        ],
    )?;

    Ok(recorded(false))
}

/// Closes one open condition, or reports that there was nothing to close.
pub(crate) fn resolve(
    tx: &Tx<'_>,
    kind: HealthConditionKind,
    scope: HealthScope,
    event_id: EventId,
    now: Timestamp,
) -> StoreResult<HealthConditionRecorded> {
    let recorded = |duplicate| HealthConditionRecorded {
        kind,
        scope,
        duplicate,
    };
    let Some(condition) = load::open_condition_id(tx, kind, scope)? else {
        return Ok(recorded(true));
    };

    let ledger = load::health_ledger(tx)?;
    if ledger.resolve(kind, scope, now)?.is_duplicate() {
        return Err(disagreement());
    }

    let seq = event::append(
        tx,
        &NewEvent {
            event_id,
            kind: EventKind::HealthConditionResolved,
            source: internal_source(condition, "health_condition_resolved")?,
            observed_at: now,
            occurred_at: None,
            scope: event_scope(scope),
            metadata: SafeMetadata::new().label("health_kind", encode_health_kind(kind)),
        },
    )?
    .seq();

    tx.conn().execute(
        "UPDATE health_conditions
            SET state = ?2, resolved_event_seq = ?3
          WHERE health_condition_id = ?1",
        params![
            id_text(condition),
            encode_health_state(HealthConditionState::Resolved),
            event::store_seq(seq)?,
        ],
    )?;

    Ok(recorded(false))
}

/// Resolves `foreman_unreachable` for an obligation a wake just reached.
///
/// `docs/testing.md` GPT-006's other half: the condition says the foreman could
/// not be reached, so an accepted delivery for that obligation is the evidence
/// that closes it. Called from both acceptance paths — a proven Send and an
/// exact reconciliation — because both are acceptances.
pub(crate) fn resolve_on_acceptance(
    tx: &Tx<'_>,
    obligation: ObligationId,
    event_id: EventId,
    now: Timestamp,
) -> StoreResult<()> {
    resolve(
        tx,
        HealthConditionKind::ForemanUnreachable,
        HealthScope::obligation(obligation),
        event_id,
        now,
    )?;
    Ok(())
}

/// The projection row and the folded ledger disagree. Fail closed.
fn disagreement() -> crate::error::StoreError {
    CorruptValue::new(
        "health_conditions",
        "state",
        CorruptReason::UnprovableEvidence,
    )
    .into()
}
