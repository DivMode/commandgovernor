//! Consequential external effects: intent before I/O, and no invented success.
//!
//! This is the provider-independent form of the rule every reviewed durable
//! runtime converged on
//! ([`docs/research/2026-08-31-durable-orchestration-pattern-review.md`]):
//!
//! > Record enough intent before a consequential external effect to detect an
//! > uncertain crash window, and never turn uncertainty into an automatic
//! > replay.
//!
//! ```text
//! intent_recorded --(call_dispatched)--> intent_recorded[dispatched]
//!        |                                      |
//!        |                                      +--> completed          (terminal)
//!        +--> failed_before_effect (terminal, proof required)
//!        +--> ambiguous           (terminal, never an automatic retry)
//! ```
//!
//! # Two identities, two jobs
//!
//! - [`ExternalAttempt`] is the durable *record*: what was intended, against
//!   which destination, under which effect class, and what became of it.
//! - [`ExternalExecutionPermit`] is the transient *capability*: the only value
//!   an adapter can be handed that authorises one consequential call. It has no
//!   public constructor, is neither `Clone` nor `Copy`, carries no payload, and
//!   is consumed by value.
//!
//! # Why `ambiguous` is terminal
//!
//! An attempt whose fate is unknown is never resolved by guessing. It produces
//! [`ReconciliationRequired`], and progress is made by opening a *new* attempt —
//! which [`ExternalAttempt::admit_retry`] permits only when the recorded class
//! is [`ExternalEffectClass::Read`], or an
//! [`ExternalEffectClass::IdempotentWrite`] whose destination, contract and
//! exact key the new attempt reproduces. There is no "probably safe" class, so
//! there is nothing to mistake for one.
//!
//! # Relationship to the browser delivery machinery
//!
//! [`crate::outbound`] and [`crate::delivery`] are the *specialised* instance of
//! this same discipline for the one transport V1 ships: a browser wake, whose
//! Send has its own arming fence, a bounded retry budget, and a deterministic
//! wake key. That machinery is reviewed, tested and unchanged; this module is
//! the generic form the Phase 2 adapters (worker launch/resume, answer
//! delivery, publication) will use. The correspondence is:
//!
//! | Browser delivery ([`crate::outbound`]) | Generic external effect (here) |
//! | --- | --- |
//! | `AttemptState::Claimed` | [`ExternalAttemptState::IntentRecorded`] |
//! | [`crate::outbound::IoPermit`] | [`ExternalExecutionPermit`] |
//! | `AttemptState::ActivationArmed` | the `dispatched` fence flag |
//! | [`crate::outbound::SendActivation`] | permit consumed by the adapter |
//! | `AttemptState::Accepted` | [`ExternalAttemptState::Completed`] |
//! | `AttemptState::Failed` + [`crate::outbound::FailureClass`] | [`ExternalAttemptState::FailedBeforeEffect`] + [`NoEffectClass`] |
//! | `AttemptState::Ambiguous` | [`ExternalAttemptState::Ambiguous`] |
//! | retry budget on the revision | [`ExternalAttempt::admit_retry`] on the class |
//!
//! They are deliberately not merged in Phase 1. The browser machine encodes a
//! transport-specific two-stage fence that the generic one has no business
//! carrying, and unifying reviewed machinery for symmetry alone would be a
//! rewrite rather than a change.
//!
//! [`docs/research/2026-08-31-durable-orchestration-pattern-review.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/research/2026-08-31-durable-orchestration-pattern-review.md

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{DaemonEpoch, SafeToken, SourceRef};
use crate::id::ExternalAttemptId;
use crate::time::{DurationMs, Timestamp};

/// Opaque identity of the external destination one effect targets.
///
/// Shaped like [`SourceRef`] on purpose: an adapter namespace, an opaque
/// endpoint identity within it, and a fence distinguishing revisions of that
/// endpoint. Every part is a [`SafeToken`], so a URL with a path, a shell
/// command, or a filesystem location is not representable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DestinationRef {
    namespace: SafeToken,
    endpoint: SafeToken,
    fence: SafeToken,
}

impl DestinationRef {
    /// Builds a destination identity from its three opaque parts.
    #[must_use]
    pub const fn new(namespace: SafeToken, endpoint: SafeToken, fence: SafeToken) -> Self {
        Self {
            namespace,
            endpoint,
            fence,
        }
    }

    /// Adapter namespace that owns the destination.
    #[must_use]
    pub const fn namespace(&self) -> &SafeToken {
        &self.namespace
    }

    /// Opaque destination identity within the namespace.
    #[must_use]
    pub const fn endpoint(&self) -> &SafeToken {
        &self.endpoint
    }

    /// Opaque fence distinguishing revisions of the same destination.
    #[must_use]
    pub const fn fence(&self) -> &SafeToken {
        &self.fence
    }
}

impl core::fmt::Display for DestinationRef {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}#{}", self.namespace, self.endpoint, self.fence)
    }
}

/// The exact key a destination deduplicates a repeated request by.
///
/// A distinct newtype rather than a bare [`SafeToken`]: an idempotency key is
/// the only thing that can make a repeat harmless, and it must never unify by
/// accident with a source fence, a destination fence, or a storage reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(SafeToken);

impl IdempotencyKey {
    /// Wraps the opaque key the caller will actually send to the destination.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the key token, for persistence and for building the request.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

impl core::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.0, f)
    }
}

/// The mechanism the *destination* documents for making a repeat harmless.
///
/// Every variant names a concrete mechanism the destination implements. There
/// is deliberately no "probably safe", "assumed idempotent", or "best effort"
/// variant: retry eligibility must rest on a recorded contract, not on a label
/// somebody attached at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum IdempotencyContract {
    /// The destination deduplicates by the exact key, for a bounded window it
    /// documents. A repeat inside the window returns the first outcome.
    DeduplicatedByKey {
        /// The window the destination documents for that deduplication.
        window: DurationMs,
    },
    /// The write is conditional on a fence the destination evaluates, and a
    /// landed first attempt has already consumed that fence, so a repeat
    /// cannot apply a second time.
    ConditionalOnDestinationFence,
}

/// How consequential one external call is.
///
/// Classification happens at the I/O boundary and is recorded with the attempt,
/// because it is the only thing that decides what an unknown fate means.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExternalEffectClass {
    /// The call changes nothing at the destination.
    Read,
    /// The call writes, and the destination makes an exact repeat harmless.
    IdempotentWrite {
        /// The mechanism the destination documents.
        contract: IdempotencyContract,
        /// The exact key that mechanism keys on.
        key: IdempotencyKey,
    },
    /// The call writes and a repeat would apply twice.
    NonIdempotentWrite,
}

impl ExternalEffectClass {
    /// Reports whether an unknown fate leaves the world possibly changed.
    #[must_use]
    pub const fn is_consequential(&self) -> bool {
        !matches!(self, Self::Read)
    }

    /// The recorded idempotency key, when the class has one.
    #[must_use]
    pub const fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        match self {
            Self::IdempotentWrite { key, .. } => Some(key),
            Self::Read | Self::NonIdempotentWrite => None,
        }
    }

    /// The recorded destination contract, when the class has one.
    #[must_use]
    pub const fn idempotency_contract(&self) -> Option<IdempotencyContract> {
        match self {
            Self::IdempotentWrite { contract, .. } => Some(*contract),
            Self::Read | Self::NonIdempotentWrite => None,
        }
    }

    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::IdempotentWrite { .. } => "idempotent_write",
            Self::NonIdempotentWrite => "non_idempotent_write",
        }
    }
}

/// Lifecycle of one attempt at one consequential external effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExternalAttemptState {
    /// The intent is recorded. The effect has not been proven either way.
    IntentRecorded,
    /// Exact evidence proves the effect landed.
    Completed,
    /// Proof establishes the effect did not happen.
    FailedBeforeEffect,
    /// The fate of the effect is unknown. Terminal, and never auto-retried.
    Ambiguous,
}

impl ExternalAttemptState {
    /// Reports whether the attempt can still change state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::IntentRecorded)
    }

    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::IntentRecorded => "intent_recorded",
            Self::Completed => "completed",
            Self::FailedBeforeEffect => "failed_before_effect",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// Why an attempt is *proven* not to have produced its effect.
///
/// Each variant is a proof obligation, not a hope. The dispatch fence splits
/// them: before the request leaves the process any pre-dispatch class holds,
/// and after it leaves only a class the destination itself established does —
/// see [`NoEffectClass::proves_no_effect_after_dispatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NoEffectClass {
    /// The permit was never handed to an adapter; no call was issued at all.
    NotAttempted,
    /// The transport refused synchronously before any byte was dispatched.
    RejectedBeforeDispatch,
    /// The destination answered with a typed refusal that it documents as
    /// applying nothing.
    DestinationRefusedWithoutApplying,
    /// The destination evaluated a caller-supplied precondition fence and
    /// rejected it; a rejected precondition applies nothing by contract.
    PreconditionRejectedAtDestination,
}

impl NoEffectClass {
    /// Reports whether this class still proves "no effect" once the dispatch
    /// fence has been committed.
    ///
    /// [`Self::NotAttempted`] cannot: once the daemon has durably recorded that
    /// it was about to dispatch, "we never tried" is a claim about a window it
    /// can no longer observe. Everything else is established by the far end.
    #[must_use]
    pub const fn proves_no_effect_after_dispatch(self) -> bool {
        !matches!(self, Self::NotAttempted)
    }

    /// Reports whether this class is observable before the dispatch fence.
    #[must_use]
    pub const fn observable_before_dispatch(self) -> bool {
        matches!(self, Self::NotAttempted | Self::RejectedBeforeDispatch)
    }

    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotAttempted => "not_attempted",
            Self::RejectedBeforeDispatch => "rejected_before_dispatch",
            Self::DestinationRefusedWithoutApplying => "destination_refused_without_applying",
            Self::PreconditionRejectedAtDestination => "precondition_rejected_at_destination",
        }
    }
}

/// Why an attempt's fate could not be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum EffectAmbiguityReason {
    /// The process died between the intent and a committed outcome.
    OrphanedByRestart,
    /// The response channel was lost while the call was in flight.
    ResponseLost,
    /// The call neither confirmed nor refused within its bound.
    DeadlineElapsed,
    /// A response arrived but did not identify the effect exactly.
    EvidenceInconclusive,
}

impl EffectAmbiguityReason {
    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::OrphanedByRestart => "orphaned_by_restart",
            Self::ResponseLost => "response_lost",
            Self::DeadlineElapsed => "deadline_elapsed",
            Self::EvidenceInconclusive => "evidence_inconclusive",
        }
    }
}

/// Evidence that the intent row for exactly one attempt is committed.
///
/// # There is no public constructor
///
/// The only source is [`RecordedIntent::accept_committed`], and the value is
/// neither `Clone` nor `Copy`, so one durable intent yields at most one
/// acceptance and at most one [`ExternalExecutionPermit`].
///
/// # What the store must uphold
///
/// `governor-core` performs no I/O and therefore cannot verify durability. The
/// acceptance is the *place where the store asserts it*, and the assertion has
/// exactly three parts:
///
/// 1. The intent row for this attempt — identity, effect class, idempotency key
///    when the class has one, destination fence, source fence and daemon epoch —
///    is committed and `fsync`-durable **before** `accept_committed` is called.
/// 2. That commit is unique for the attempt identity, so a crash-and-retry
///    cannot produce a second intent row and therefore a second permit for one
///    logical operation.
/// 3. Nothing between that commit and the adapter call may perform the external
///    effect. The permit is the only authorisation, and the store hands it on
///    exactly once.
///
/// A store that calls `accept_committed` before its transaction commits has
/// broken the contract; every downstream guarantee here rests on that one line.
#[derive(Debug)]
pub struct DurableIntentAccepted {
    attempt: ExternalAttemptId,
}

impl DurableIntentAccepted {
    /// The attempt whose intent row this acceptance vouches for.
    #[must_use]
    pub const fn attempt(&self) -> ExternalAttemptId {
        self.attempt
    }
}

/// A freshly opened intent, and the acceptance the store surrenders once the
/// intent row is durable.
///
/// The pair exists so the two halves cannot be separated by accident: a caller
/// holding a [`DurableIntentAccepted`] must have gone through
/// [`Self::accept_committed`], and the only way to reach that is to have built
/// the projection this struct carries.
#[derive(Debug)]
pub struct RecordedIntent<E> {
    attempt: ExternalAttempt<E>,
    acceptance: DurableIntentAccepted,
}

impl<E> RecordedIntent<E> {
    /// The projection whose intent row must be committed.
    #[must_use]
    pub const fn attempt(&self) -> &ExternalAttempt<E> {
        &self.attempt
    }

    /// Surrenders the single-use acceptance for a **committed** intent row.
    ///
    /// Named so every call site reads as the deliberate durability assertion it
    /// is; see [`DurableIntentAccepted`] for the three things the caller is
    /// asserting when it calls this.
    #[must_use]
    pub fn accept_committed(self) -> (ExternalAttempt<E>, DurableIntentAccepted) {
        (self.attempt, self.acceptance)
    }
}

/// Permission to perform exactly one consequential external call.
///
/// # Why this type has the API it has
///
/// - **No public constructor.** The only source is
///   [`ExternalAttempt::decide`], and only when a [`DurableIntentAccepted`] for
///   that exact attempt is presented.
/// - **Not `Clone`, not `Copy`, not serialisable.** One durable intent
///   authorises one call. There is no derive and no method that duplicates it,
///   and it carries no `serde` implementation to smuggle it across a boundary.
/// - **Consumed by value.** An adapter that performs the effect takes the
///   permit by value, so the capability is spent at the call site.
/// - **Fences, never payload.** It carries the attempt identity, the effect
///   class with its idempotency key, the destination fence, the source fence
///   and the daemon epoch — and nothing that a prompt, a tool argument, or a
///   request body could be routed through.
#[derive(Debug)]
pub struct ExternalExecutionPermit {
    attempt: ExternalAttemptId,
    class: ExternalEffectClass,
    destination: DestinationRef,
    source: SourceRef,
    daemon_epoch: DaemonEpoch,
}

impl ExternalExecutionPermit {
    /// The attempt this permit authorises.
    #[must_use]
    pub const fn attempt(&self) -> ExternalAttemptId {
        self.attempt
    }

    /// The recorded effect class, including the exact idempotency key.
    #[must_use]
    pub const fn class(&self) -> &ExternalEffectClass {
        &self.class
    }

    /// The destination fence the call must target.
    #[must_use]
    pub const fn destination(&self) -> &DestinationRef {
        &self.destination
    }

    /// The source fact that justified the effect.
    #[must_use]
    pub const fn source(&self) -> &SourceRef {
        &self.source
    }

    /// The daemon epoch the intent was recorded under.
    #[must_use]
    pub const fn daemon_epoch(&self) -> DaemonEpoch {
        self.daemon_epoch
    }
}

/// A recorded attempt whose fate is unknown and must be resolved by a human or
/// an explicit reconciliation procedure.
///
/// It carries everything the resolver needs to go and look — attempt, class,
/// exact idempotency key when there is one, destination, source, and why the
/// fate was lost — and nothing that would put request content in the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationRequired {
    attempt: ExternalAttemptId,
    class: ExternalEffectClass,
    destination: DestinationRef,
    source: SourceRef,
    reason: EffectAmbiguityReason,
}

impl ReconciliationRequired {
    /// The attempt whose fate is unknown.
    #[must_use]
    pub const fn attempt(&self) -> ExternalAttemptId {
        self.attempt
    }

    /// The recorded effect class, including the exact idempotency key.
    #[must_use]
    pub const fn class(&self) -> &ExternalEffectClass {
        &self.class
    }

    /// The destination the effect targeted.
    #[must_use]
    pub const fn destination(&self) -> &DestinationRef {
        &self.destination
    }

    /// The source fact that justified the effect.
    #[must_use]
    pub const fn source(&self) -> &SourceRef {
        &self.source
    }

    /// Why the fate was lost.
    #[must_use]
    pub const fn reason(&self) -> EffectAmbiguityReason {
        self.reason
    }
}

/// The deterministic execution seam.
///
/// The three ways forward are three variants, and only one of them carries a
/// capability. An adapter that is handed an [`EffectDecision`] cannot reach
/// consequential I/O through [`Self::Replayed`] — there is no permit in it, and
/// [`ExternalExecutionPermit`] has no other constructor — so a replayed outcome
/// and a live call are not merely conventionally different, they are different
/// types.
#[derive(Debug)]
pub enum EffectDecision<T> {
    /// The effect already happened; this is the recorded outcome, projected.
    Replayed(T),
    /// The effect has not happened and may now be performed, exactly once.
    Execute(ExternalExecutionPermit),
    /// The fate is unknown. Neither replay nor retry is permitted.
    Reconcile(ReconciliationRequired),
}

impl<T> EffectDecision<T> {
    /// Reports whether this decision authorises a live external call.
    #[must_use]
    pub const fn is_execute(&self) -> bool {
        matches!(self, Self::Execute(_))
    }

    /// Returns the replayed outcome, if that is what this decision is.
    #[must_use]
    pub fn replayed(self) -> Option<T> {
        match self {
            Self::Replayed(value) => Some(value),
            Self::Execute(_) | Self::Reconcile(_) => None,
        }
    }

    /// Returns the execution permit, if that is what this decision is.
    #[must_use]
    pub fn permit(self) -> Option<ExternalExecutionPermit> {
        match self {
            Self::Execute(permit) => Some(permit),
            Self::Replayed(_) | Self::Reconcile(_) => None,
        }
    }
}

/// An event applied to an external attempt.
///
/// `E` is the exact completion evidence of the concrete adapter, exactly as
/// [`crate::outbound::Delivery`] parameterises its acceptance evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExternalAttemptEvent<E> {
    /// Commit the dispatch fence immediately **before** the adapter issues the
    /// call. A crash after this and before an outcome is
    /// [`ExternalAttemptState::Ambiguous`], never success and never failure.
    CallDispatched {
        /// Observation instant.
        at: Timestamp,
    },
    /// Exact evidence proves the effect landed.
    Completed {
        /// Adapter-specific evidence binding the effect to its intent.
        evidence: E,
        /// Observation instant.
        at: Timestamp,
    },
    /// The effect is proven not to have happened.
    FailedBeforeEffect {
        /// The proof class. Every variant is a proof; there is no weak one.
        proof: NoEffectClass,
        /// Observation instant.
        at: Timestamp,
    },
    /// The fate of the effect cannot be determined.
    OutcomeUnknown {
        /// Why the fate was lost.
        reason: EffectAmbiguityReason,
        /// Observation instant.
        at: Timestamp,
    },
}

impl<E> ExternalAttemptEvent<E> {
    const fn label(&self) -> &'static str {
        match self {
            Self::CallDispatched { .. } => "call_dispatched",
            Self::Completed { .. } => "completed",
            Self::FailedBeforeEffect { .. } => "failed_before_effect",
            Self::OutcomeUnknown { .. } => "outcome_unknown",
        }
    }
}

/// One durable attempt at one consequential external effect.
///
/// # Construction is replay
///
/// The fields are private and the only constructor is
/// [`Self::record_intent`], which always produces
/// [`ExternalAttemptState::IntentRecorded`]. Every other state is reached by
/// folding [`ExternalAttemptEvent`]s, so a projection cannot be internally
/// inconsistent and the store rebuilds it the same way the daemon built it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAttempt<E> {
    id: ExternalAttemptId,
    class: ExternalEffectClass,
    destination: DestinationRef,
    source: SourceRef,
    daemon_epoch: DaemonEpoch,
    state: ExternalAttemptState,
    dispatched: bool,
    recorded_at: Timestamp,
    dispatched_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    outcome: Option<E>,
    no_effect: Option<NoEffectClass>,
    ambiguity: Option<EffectAmbiguityReason>,
}

impl<E: Clone + PartialEq> ExternalAttempt<E> {
    /// Opens an attempt by recording its intent.
    ///
    /// This is the pure half of "intent before I/O": it produces the row the
    /// store must commit, plus the single-use [`DurableIntentAccepted`] the
    /// store surrenders once that commit is durable.
    #[must_use]
    pub fn record_intent(
        id: ExternalAttemptId,
        class: ExternalEffectClass,
        destination: DestinationRef,
        source: SourceRef,
        daemon_epoch: DaemonEpoch,
        at: Timestamp,
    ) -> RecordedIntent<E> {
        RecordedIntent {
            attempt: Self {
                id,
                class,
                destination,
                source,
                daemon_epoch,
                state: ExternalAttemptState::IntentRecorded,
                dispatched: false,
                recorded_at: at,
                dispatched_at: None,
                finished_at: None,
                outcome: None,
                no_effect: None,
                ambiguity: None,
            },
            acceptance: DurableIntentAccepted { attempt: id },
        }
    }

    /// Attempt identity.
    #[must_use]
    pub const fn id(&self) -> ExternalAttemptId {
        self.id
    }

    /// Recorded effect class, including the exact idempotency key.
    #[must_use]
    pub const fn class(&self) -> &ExternalEffectClass {
        &self.class
    }

    /// Recorded destination fence.
    #[must_use]
    pub const fn destination(&self) -> &DestinationRef {
        &self.destination
    }

    /// Source fact that justified the effect.
    #[must_use]
    pub const fn source(&self) -> &SourceRef {
        &self.source
    }

    /// Daemon epoch the intent was recorded under.
    #[must_use]
    pub const fn daemon_epoch(&self) -> DaemonEpoch {
        self.daemon_epoch
    }

    /// Current attempt state.
    #[must_use]
    pub const fn state(&self) -> ExternalAttemptState {
        self.state
    }

    /// Reports whether the dispatch fence was ever committed.
    #[must_use]
    pub const fn dispatched(&self) -> bool {
        self.dispatched
    }

    /// Instant the intent was recorded.
    #[must_use]
    pub const fn recorded_at(&self) -> Timestamp {
        self.recorded_at
    }

    /// Instant the dispatch fence was committed, if it was.
    #[must_use]
    pub const fn dispatched_at(&self) -> Option<Timestamp> {
        self.dispatched_at
    }

    /// Instant the attempt reached a terminal state, if it has.
    #[must_use]
    pub const fn finished_at(&self) -> Option<Timestamp> {
        self.finished_at
    }

    /// The recorded completion evidence, once completed.
    #[must_use]
    pub const fn outcome(&self) -> Option<&E> {
        self.outcome.as_ref()
    }

    /// The proof class, once proven not to have happened.
    #[must_use]
    pub const fn no_effect(&self) -> Option<NoEffectClass> {
        self.no_effect
    }

    /// Why the fate was lost, once ambiguous.
    #[must_use]
    pub const fn ambiguity(&self) -> Option<EffectAmbiguityReason> {
        self.ambiguity
    }

    /// The reconciliation record, once the attempt is ambiguous.
    #[must_use]
    pub fn reconciliation(&self) -> Option<ReconciliationRequired> {
        self.ambiguity.map(|reason| ReconciliationRequired {
            attempt: self.id,
            class: self.class.clone(),
            destination: self.destination.clone(),
            source: self.source.clone(),
            reason,
        })
    }

    /// The deterministic decision for this attempt.
    ///
    /// `acceptance` is the store's assertion that the intent row is durable;
    /// it is consumed, so one durable intent yields at most one permit. Pass
    /// `None` on a pure replay path — a replay can then only ever produce
    /// [`EffectDecision::Replayed`] or [`EffectDecision::Reconcile`].
    ///
    /// `project` turns the recorded evidence into whatever the caller wanted
    /// from the call. It runs only on the replay path and never sees a permit.
    ///
    /// # Errors
    ///
    /// - [`Conflict::AttemptPermitMismatch`] when the acceptance belongs to a
    ///   different attempt;
    /// - [`Conflict::ExecuteRequiresDurableIntent`] when the intent is recorded
    ///   but no acceptance was presented — the acceptance test "no execute
    ///   permit before intent acceptance";
    /// - [`Conflict::AttemptAlreadyDispatched`] when the dispatch fence is
    ///   already committed, so a second permit would authorise a second call;
    /// - [`Conflict::IllegalAttemptTransition`] from
    ///   [`ExternalAttemptState::FailedBeforeEffect`], which needs a *new*
    ///   attempt rather than a second permit for this one.
    ///
    /// Every one of these leaves the attempt untouched, and destroys the
    /// acceptance without producing a permit.
    pub fn decide<T>(
        &self,
        acceptance: Option<DurableIntentAccepted>,
        project: impl FnOnce(&E) -> T,
    ) -> Result<EffectDecision<T>, Conflict> {
        if let Some(accepted) = &acceptance
            && accepted.attempt != self.id
        {
            return Err(Conflict::AttemptPermitMismatch {
                presented: accepted.attempt,
                attempt: self.id,
            });
        }
        match self.state {
            ExternalAttemptState::Completed => {
                let evidence = self
                    .outcome
                    .as_ref()
                    .expect("a completed attempt always carries its evidence");
                Ok(EffectDecision::Replayed(project(evidence)))
            }
            ExternalAttemptState::Ambiguous => Ok(EffectDecision::Reconcile(
                self.reconciliation()
                    .expect("an ambiguous attempt always carries its reason"),
            )),
            ExternalAttemptState::FailedBeforeEffect => Err(Conflict::IllegalAttemptTransition {
                from: self.state,
                event: "decide",
            }),
            ExternalAttemptState::IntentRecorded => {
                if self.dispatched {
                    return Err(Conflict::AttemptAlreadyDispatched { attempt: self.id });
                }
                match acceptance {
                    None => Err(Conflict::ExecuteRequiresDurableIntent { attempt: self.id }),
                    Some(_) => Ok(EffectDecision::Execute(ExternalExecutionPermit {
                        attempt: self.id,
                        class: self.class.clone(),
                        destination: self.destination.clone(),
                        source: self.source.clone(),
                        daemon_epoch: self.daemon_epoch,
                    })),
                }
            }
        }
    }

    /// Reports whether a *new* attempt at the same logical operation may be
    /// opened after this one, and under what condition.
    #[must_use]
    pub const fn retry_admissibility(&self) -> RetryAdmissibility {
        match self.state {
            ExternalAttemptState::IntentRecorded => RetryAdmissibility::NotFinished,
            ExternalAttemptState::Completed => RetryAdmissibility::AlreadyCompleted,
            ExternalAttemptState::FailedBeforeEffect => RetryAdmissibility::ProvenNoEffect,
            ExternalAttemptState::Ambiguous => match self.class {
                ExternalEffectClass::Read => RetryAdmissibility::HarmlessRead,
                ExternalEffectClass::IdempotentWrite { .. } => {
                    RetryAdmissibility::RequiresIdenticalIdempotencyContract
                }
                ExternalEffectClass::NonIdempotentWrite => {
                    RetryAdmissibility::RequiresReconciliation
                }
            },
        }
    }

    /// Admits a *new* attempt at the same logical operation.
    ///
    /// A retry is never a transition of this attempt: ambiguous and completed
    /// are terminal. This only answers whether opening the next attempt is
    /// permitted, and for an ambiguous idempotent write it demands the
    /// identical destination, contract and exact key — a different key would
    /// apply the write twice.
    ///
    /// # Errors
    ///
    /// - [`Conflict::IllegalAttemptTransition`] while the attempt is unfinished;
    /// - [`Conflict::AttemptAlreadyCompleted`] when the effect already landed;
    /// - [`Conflict::RetryRequiresIdempotencyContract`] when the fate is unknown
    ///   and no recorded contract plus exact key covers the proposed retry. This
    ///   is the only answer for an ambiguous non-idempotent write, which is why
    ///   there is no automatic path out of it.
    pub fn admit_retry(
        &self,
        destination: &DestinationRef,
        class: &ExternalEffectClass,
    ) -> Result<(), Conflict> {
        match self.retry_admissibility() {
            RetryAdmissibility::NotFinished => Err(Conflict::IllegalAttemptTransition {
                from: self.state,
                event: "retry",
            }),
            RetryAdmissibility::AlreadyCompleted => {
                Err(Conflict::AttemptAlreadyCompleted { attempt: self.id })
            }
            RetryAdmissibility::ProvenNoEffect | RetryAdmissibility::HarmlessRead => Ok(()),
            RetryAdmissibility::RequiresReconciliation => {
                Err(Conflict::RetryRequiresIdempotencyContract { attempt: self.id })
            }
            RetryAdmissibility::RequiresIdenticalIdempotencyContract => {
                if *destination == self.destination && *class == self.class {
                    Ok(())
                } else {
                    Err(Conflict::RetryRequiresIdempotencyContract { attempt: self.id })
                }
            }
        }
    }

    /// Applies an event, returning a new attempt or a typed conflict.
    ///
    /// The receiver is borrowed and never mutated, so a conflict provably left
    /// nothing half-applied.
    ///
    /// # Errors
    ///
    /// Returns the [`Conflict`] describing why the event is not legal here.
    pub fn apply(&self, event: &ExternalAttemptEvent<E>) -> Outcome<Self> {
        match event {
            ExternalAttemptEvent::CallDispatched { at } => self.dispatch(*at, event.label()),
            ExternalAttemptEvent::Completed { evidence, at } => {
                self.complete(evidence, *at, event.label())
            }
            ExternalAttemptEvent::FailedBeforeEffect { proof, at } => {
                self.fail(*proof, *at, event.label())
            }
            ExternalAttemptEvent::OutcomeUnknown { reason, at } => {
                self.lose_outcome(*reason, *at, event.label())
            }
        }
    }

    fn dispatch(&self, at: Timestamp, label: &'static str) -> Outcome<Self> {
        match self.state {
            ExternalAttemptState::IntentRecorded if self.dispatched => Ok(Transition::Duplicate),
            ExternalAttemptState::IntentRecorded => {
                let mut next = self.clone();
                next.dispatched = true;
                next.dispatched_at = Some(at);
                Ok(Transition::Advanced(next))
            }
            from => Err(Conflict::IllegalAttemptTransition { from, event: label }),
        }
    }

    fn complete(&self, evidence: &E, at: Timestamp, label: &'static str) -> Outcome<Self> {
        match self.state {
            // Completion is reachable only through the dispatch fence: a call
            // that was never issued cannot have landed.
            ExternalAttemptState::IntentRecorded if self.dispatched => {
                let mut next = self.clone();
                next.state = ExternalAttemptState::Completed;
                next.finished_at = Some(at);
                next.outcome = Some(evidence.clone());
                Ok(Transition::Advanced(next))
            }
            ExternalAttemptState::Completed if self.outcome.as_ref() == Some(evidence) => {
                Ok(Transition::Duplicate)
            }
            from => Err(Conflict::IllegalAttemptTransition { from, event: label }),
        }
    }

    fn fail(&self, proof: NoEffectClass, at: Timestamp, label: &'static str) -> Outcome<Self> {
        match self.state {
            ExternalAttemptState::IntentRecorded => {
                let proven = if self.dispatched {
                    proof.proves_no_effect_after_dispatch()
                } else {
                    proof.observable_before_dispatch()
                };
                if !proven {
                    return Err(Conflict::EffectNotProvenAbsent {
                        attempt: self.id,
                        proof,
                        dispatched: self.dispatched,
                    });
                }
                let mut next = self.clone();
                next.state = ExternalAttemptState::FailedBeforeEffect;
                next.finished_at = Some(at);
                next.no_effect = Some(proof);
                Ok(Transition::Advanced(next))
            }
            ExternalAttemptState::FailedBeforeEffect if self.no_effect == Some(proof) => {
                Ok(Transition::Duplicate)
            }
            ExternalAttemptState::Completed => {
                Err(Conflict::AttemptAlreadyCompleted { attempt: self.id })
            }
            from => Err(Conflict::IllegalAttemptTransition { from, event: label }),
        }
    }

    fn lose_outcome(
        &self,
        reason: EffectAmbiguityReason,
        at: Timestamp,
        label: &'static str,
    ) -> Outcome<Self> {
        match self.state {
            ExternalAttemptState::IntentRecorded => {
                let mut next = self.clone();
                next.state = ExternalAttemptState::Ambiguous;
                next.finished_at = Some(at);
                next.ambiguity = Some(reason);
                Ok(Transition::Advanced(next))
            }
            // Already unknown: a second report adds nothing and must not
            // overwrite the first reason with a later, weaker one.
            ExternalAttemptState::Ambiguous => Ok(Transition::Duplicate),
            ExternalAttemptState::Completed => {
                Err(Conflict::AttemptAlreadyCompleted { attempt: self.id })
            }
            from => Err(Conflict::IllegalAttemptTransition { from, event: label }),
        }
    }
}

/// Whether the next attempt at one logical operation may be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetryAdmissibility {
    /// The attempt has not finished; there is nothing to retry yet.
    NotFinished,
    /// The effect landed. Retrying would apply it twice.
    AlreadyCompleted,
    /// Proven not to have happened, so any class may be attempted again.
    ProvenNoEffect,
    /// The fate is unknown but the class changes nothing at the destination.
    HarmlessRead,
    /// The fate is unknown and the recorded contract plus exact key must be
    /// reproduced for the next attempt to be safe.
    RequiresIdenticalIdempotencyContract,
    /// The fate is unknown and nothing makes a repeat safe. Only an explicit
    /// reconciliation decision resolves it.
    RequiresReconciliation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fence::test_support::source;
    use uuid::Uuid;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn attempt_id(n: u128) -> ExternalAttemptId {
        ExternalAttemptId::from_uuid(Uuid::from_u128(n))
    }

    fn token(value: &str) -> SafeToken {
        SafeToken::new(value).expect("fixture tokens are safe")
    }

    fn destination() -> DestinationRef {
        DestinationRef::new(token("worker-host"), token("turn-7"), token("gen-1"))
    }

    fn key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(token(value))
    }

    fn idempotent(value: &str) -> ExternalEffectClass {
        ExternalEffectClass::IdempotentWrite {
            contract: IdempotencyContract::DeduplicatedByKey {
                window: DurationMs::from_millis(60_000),
            },
            key: key(value),
        }
    }

    fn intent(class: ExternalEffectClass) -> RecordedIntent<u8> {
        ExternalAttempt::<u8>::record_intent(
            attempt_id(1),
            class,
            destination(),
            source("worker.resume", "cmd-1", "rev-1"),
            DaemonEpoch::FIRST,
            at(1),
        )
    }

    fn dispatched(class: ExternalEffectClass) -> (ExternalAttempt<u8>, DurableIntentAccepted) {
        let (attempt, acceptance) = intent(class).accept_committed();
        let attempt = attempt
            .apply(&ExternalAttemptEvent::CallDispatched { at: at(2) })
            .expect("dispatch is legal from a recorded intent")
            .advanced()
            .expect("dispatch advances");
        (attempt, acceptance)
    }

    #[test]
    fn a_recorded_intent_alone_yields_no_permit() {
        let recorded = intent(ExternalEffectClass::NonIdempotentWrite);
        let attempt = recorded.attempt();
        let err = attempt
            .decide(None, |value: &u8| *value)
            .expect_err("no acceptance means no permit");
        assert_eq!(err.code(), "execute_requires_durable_intent");
        assert_eq!(attempt.state(), ExternalAttemptState::IntentRecorded);
        assert!(!attempt.dispatched());
    }

    #[test]
    fn a_durable_intent_acceptance_yields_exactly_one_permit() {
        let (attempt, acceptance) = intent(idempotent("k-1")).accept_committed();
        let decision = attempt
            .decide(Some(acceptance), |value: &u8| *value)
            .expect("an accepted intent may execute");
        assert!(decision.is_execute());
        let permit = decision.permit().expect("execute carries the permit");
        assert_eq!(permit.attempt(), attempt.id());
        assert_eq!(permit.destination(), &destination());
        assert_eq!(
            permit.class().idempotency_key(),
            Some(&key("k-1")),
            "the permit carries the exact key, not a payload"
        );
        // The acceptance was consumed by value; there is no second permit to be
        // had from it, and `ExternalExecutionPermit` is neither Clone nor Copy.
    }

    #[test]
    fn an_acceptance_for_another_attempt_is_refused() {
        let (attempt, _) = intent(ExternalEffectClass::NonIdempotentWrite).accept_committed();
        let (_, foreign) = ExternalAttempt::<u8>::record_intent(
            attempt_id(2),
            ExternalEffectClass::NonIdempotentWrite,
            destination(),
            source("worker.resume", "cmd-2", "rev-1"),
            DaemonEpoch::FIRST,
            at(1),
        )
        .accept_committed();
        let err = attempt
            .decide(Some(foreign), |value: &u8| *value)
            .expect_err("an acceptance is bound to its attempt");
        assert_eq!(err.code(), "attempt_permit_mismatch");
    }

    #[test]
    fn a_dispatched_attempt_never_gets_a_second_permit() {
        let (attempt, acceptance) = dispatched(ExternalEffectClass::NonIdempotentWrite);
        let err = attempt
            .decide(Some(acceptance), |value: &u8| *value)
            .expect_err("the call already went out");
        assert_eq!(err.code(), "attempt_already_dispatched");
    }

    #[test]
    fn completion_requires_the_dispatch_fence() {
        let (attempt, _) = intent(ExternalEffectClass::NonIdempotentWrite).accept_committed();
        let err = attempt
            .apply(&ExternalAttemptEvent::Completed {
                evidence: 7,
                at: at(3),
            })
            .expect_err("a call that never went out cannot have landed");
        assert_eq!(err.code(), "illegal_attempt_transition");
        assert_eq!(attempt.state(), ExternalAttemptState::IntentRecorded);
    }

    #[test]
    fn a_completed_attempt_replays_its_recorded_outcome_without_a_permit() {
        let (attempt, _) = dispatched(idempotent("k-1"));
        let completed = attempt
            .apply(&ExternalAttemptEvent::Completed {
                evidence: 42,
                at: at(3),
            })
            .expect("completion after dispatch is legal")
            .advanced()
            .expect("completion advances");
        let decision = completed
            .decide(None, |value: &u8| u32::from(*value) * 2)
            .expect("a completed attempt always replays");
        assert!(!decision.is_execute());
        assert_eq!(decision.replayed(), Some(84));
    }

    #[test]
    fn a_completed_attempt_ignores_a_stale_acceptance_and_still_replays() {
        let (attempt, acceptance) = dispatched(idempotent("k-1"));
        let completed = attempt
            .apply(&ExternalAttemptEvent::Completed {
                evidence: 42,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let decision = completed
            .decide(Some(acceptance), |value: &u8| *value)
            .expect("replay wins over a stale acceptance");
        assert!(
            !decision.is_execute(),
            "a landed effect is never executed again"
        );
    }

    #[test]
    fn failure_before_dispatch_needs_a_pre_dispatch_proof() {
        let (attempt, _) = intent(ExternalEffectClass::NonIdempotentWrite).accept_committed();
        let err = attempt
            .apply(&ExternalAttemptEvent::FailedBeforeEffect {
                proof: NoEffectClass::DestinationRefusedWithoutApplying,
                at: at(3),
            })
            .expect_err("the destination cannot have refused a call never made");
        assert_eq!(err.code(), "effect_not_proven_absent");
        assert_eq!(attempt.state(), ExternalAttemptState::IntentRecorded);
    }

    #[test]
    fn not_attempted_stops_proving_anything_once_dispatch_is_fenced() {
        let (attempt, _) = dispatched(ExternalEffectClass::NonIdempotentWrite);
        let err = attempt
            .apply(&ExternalAttemptEvent::FailedBeforeEffect {
                proof: NoEffectClass::NotAttempted,
                at: at(3),
            })
            .expect_err("after the fence, 'we never tried' is unobservable");
        assert_eq!(err.code(), "effect_not_proven_absent");
    }

    #[test]
    fn a_destination_side_proof_is_accepted_after_dispatch() {
        let (attempt, _) = dispatched(ExternalEffectClass::NonIdempotentWrite);
        let failed = attempt
            .apply(&ExternalAttemptEvent::FailedBeforeEffect {
                proof: NoEffectClass::PreconditionRejectedAtDestination,
                at: at(3),
            })
            .expect("a rejected precondition applies nothing")
            .advanced()
            .unwrap();
        assert_eq!(failed.state(), ExternalAttemptState::FailedBeforeEffect);
        assert_eq!(
            failed.retry_admissibility(),
            RetryAdmissibility::ProvenNoEffect
        );
        failed
            .admit_retry(&destination(), &ExternalEffectClass::NonIdempotentWrite)
            .expect("a proven absent effect may be attempted again");
    }

    #[test]
    fn an_ambiguous_non_idempotent_write_never_admits_a_retry() {
        let (attempt, _) = dispatched(ExternalEffectClass::NonIdempotentWrite);
        let ambiguous = attempt
            .apply(&ExternalAttemptEvent::OutcomeUnknown {
                reason: EffectAmbiguityReason::OrphanedByRestart,
                at: at(3),
            })
            .expect("a lost fate is always recordable")
            .advanced()
            .unwrap();
        assert_eq!(ambiguous.state(), ExternalAttemptState::Ambiguous);
        let err = ambiguous
            .admit_retry(&destination(), &ExternalEffectClass::NonIdempotentWrite)
            .expect_err("nothing makes this repeat safe");
        assert_eq!(err.code(), "retry_requires_idempotency_contract");

        let decision = ambiguous
            .decide(None, |value: &u8| *value)
            .expect("an ambiguous attempt decides to reconcile");
        assert!(!decision.is_execute());
        let EffectDecision::Reconcile(required) = decision else {
            panic!("expected reconciliation");
        };
        assert_eq!(required.attempt(), ambiguous.id());
        assert_eq!(required.reason(), EffectAmbiguityReason::OrphanedByRestart);
    }

    #[test]
    fn an_ambiguous_idempotent_write_admits_only_the_exact_recorded_contract() {
        let (attempt, _) = dispatched(idempotent("k-1"));
        let ambiguous = attempt
            .apply(&ExternalAttemptEvent::OutcomeUnknown {
                reason: EffectAmbiguityReason::ResponseLost,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();

        ambiguous
            .admit_retry(&destination(), &idempotent("k-1"))
            .expect("the exact recorded contract and key are reproduced");

        for wrong in [
            idempotent("k-2"),
            ExternalEffectClass::IdempotentWrite {
                contract: IdempotencyContract::ConditionalOnDestinationFence,
                key: key("k-1"),
            },
            ExternalEffectClass::NonIdempotentWrite,
        ] {
            let err = ambiguous
                .admit_retry(&destination(), &wrong)
                .expect_err("a different contract or key is not the recorded one");
            assert_eq!(err.code(), "retry_requires_idempotency_contract");
        }

        let elsewhere = DestinationRef::new(token("worker-host"), token("turn-9"), token("gen-1"));
        let err = ambiguous
            .admit_retry(&elsewhere, &idempotent("k-1"))
            .expect_err("the key only deduplicates at its own destination");
        assert_eq!(err.code(), "retry_requires_idempotency_contract");
    }

    #[test]
    fn an_ambiguous_read_is_harmless_to_repeat() {
        let (attempt, _) = dispatched(ExternalEffectClass::Read);
        let ambiguous = attempt
            .apply(&ExternalAttemptEvent::OutcomeUnknown {
                reason: EffectAmbiguityReason::DeadlineElapsed,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(
            ambiguous.retry_admissibility(),
            RetryAdmissibility::HarmlessRead
        );
        ambiguous
            .admit_retry(&destination(), &ExternalEffectClass::Read)
            .expect("re-reading changes nothing");
    }

    #[test]
    fn completed_and_ambiguous_are_terminal() {
        let (attempt, _) = dispatched(idempotent("k-1"));
        let completed = attempt
            .apply(&ExternalAttemptEvent::Completed {
                evidence: 5,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert!(
            completed
                .apply(&ExternalAttemptEvent::Completed {
                    evidence: 5,
                    at: at(3)
                })
                .unwrap()
                .is_duplicate()
        );
        for event in [
            ExternalAttemptEvent::OutcomeUnknown {
                reason: EffectAmbiguityReason::ResponseLost,
                at: at(4),
            },
            ExternalAttemptEvent::FailedBeforeEffect {
                proof: NoEffectClass::RejectedBeforeDispatch,
                at: at(4),
            },
        ] {
            let err = completed
                .apply(&event)
                .expect_err("a landed effect is not unlanded later");
            assert_eq!(err.code(), "attempt_already_completed");
        }

        let (attempt, _) = dispatched(idempotent("k-2"));
        let ambiguous = attempt
            .apply(&ExternalAttemptEvent::OutcomeUnknown {
                reason: EffectAmbiguityReason::ResponseLost,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let err = ambiguous
            .apply(&ExternalAttemptEvent::Completed {
                evidence: 5,
                at: at(4),
            })
            .expect_err("ambiguity is terminal for this attempt");
        assert_eq!(err.code(), "illegal_attempt_transition");
        assert!(
            ambiguous
                .apply(&ExternalAttemptEvent::OutcomeUnknown {
                    reason: EffectAmbiguityReason::EvidenceInconclusive,
                    at: at(5),
                })
                .unwrap()
                .is_duplicate(),
            "a second lost-fate report must not overwrite the first reason"
        );
    }

    #[test]
    fn a_failed_attempt_offers_no_decision_of_its_own() {
        let (attempt, _) = intent(ExternalEffectClass::NonIdempotentWrite).accept_committed();
        let failed = attempt
            .apply(&ExternalAttemptEvent::FailedBeforeEffect {
                proof: NoEffectClass::NotAttempted,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let err = failed
            .decide(None, |value: &u8| *value)
            .expect_err("progress needs a new attempt, not a second permit");
        assert_eq!(err.code(), "illegal_attempt_transition");
    }

    #[test]
    fn an_unfinished_attempt_cannot_be_retried() {
        let (attempt, _) = intent(idempotent("k-1")).accept_committed();
        let err = attempt
            .admit_retry(&destination(), &idempotent("k-1"))
            .expect_err("there is nothing to retry yet");
        assert_eq!(err.code(), "illegal_attempt_transition");
    }

    #[test]
    fn a_completed_attempt_cannot_be_retried() {
        let (attempt, _) = dispatched(idempotent("k-1"));
        let completed = attempt
            .apply(&ExternalAttemptEvent::Completed {
                evidence: 1,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let err = completed
            .admit_retry(&destination(), &idempotent("k-1"))
            .expect_err("retrying would apply the write twice");
        assert_eq!(err.code(), "attempt_already_completed");
    }

    #[test]
    fn the_dispatch_fence_is_idempotent() {
        let (attempt, _) = dispatched(ExternalEffectClass::Read);
        assert!(
            attempt
                .apply(&ExternalAttemptEvent::CallDispatched { at: at(9) })
                .unwrap()
                .is_duplicate()
        );
        assert_eq!(attempt.dispatched_at(), Some(at(2)));
    }

    #[test]
    fn effect_class_codes_are_stable() {
        assert_eq!(ExternalEffectClass::Read.code(), "read");
        assert_eq!(idempotent("k").code(), "idempotent_write");
        assert_eq!(
            ExternalEffectClass::NonIdempotentWrite.code(),
            "non_idempotent_write"
        );
        assert!(!ExternalEffectClass::Read.is_consequential());
        assert!(ExternalEffectClass::NonIdempotentWrite.is_consequential());
    }
}
