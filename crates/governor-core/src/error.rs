//! Typed, machine-classifiable domain conflicts.
//!
//! Every rejection in this crate is a [`Conflict`], and every [`Conflict`] has
//! a stable [`ConflictKind`] code. Two properties matter and are tested:
//!
//! 1. **Zero mutation.** Transition functions take `&State` and return a *new*
//!    state, so a rejected event cannot have changed anything — there is no
//!    mutable state for it to have touched.
//! 2. **Classifiable.** A caller (MCP tool, CLI, health projection) branches on
//!    [`ConflictKind`], never on a formatted string.

use crate::effect::{ExternalAttemptState, NoEffectClass};
use crate::fence::{
    AttemptNo, BindingGeneration, CommandRevision, DaemonEpoch, DeliveryRevision,
    IncarnationGeneration, ObligationVersion, SourceRef,
};
use crate::foreman_turn::ForemanTurnState;
use crate::id::{
    ActorId, ClaimId, ExternalAttemptId, MutationCommandId, ObligationId, ResourceLeaseId,
};
use crate::input::InputRequestState;
use crate::lease::{IncarnationMismatch, LeaseState};
use crate::mutation::MutationCommandStatus;
use crate::obligation::ObligationState;
use crate::outbound::{AttemptState, DeliveryState};

/// Stable machine-readable classification of a [`Conflict`].
///
/// The `snake_case` codes are part of the contract with the CLI, the health
/// projection, and the acceptance suite; they are not derived from the Rust
/// variant names at runtime and must not drift silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ConflictKind {
    /// The presented binding generation is older than the active one.
    StaleBindingGeneration,
    /// The presented binding generation does not exist yet.
    UnknownBindingGeneration,
    /// No foreman binding is active, so no fenced mutation is possible.
    NoActiveBinding,
    /// The presented obligation version is not the current one.
    StaleObligationVersion,
    /// The presented source fence is not the obligation's current source fact.
    StaleSourceFence,
    /// The presented claim is not the obligation's current claim.
    StaleClaim,
    /// The obligation has no current claim to mutate under.
    NoCurrentClaim,
    /// The presented claim has passed its expiry instant.
    ExpiredClaim,
    /// The obligation is already held by a live claim.
    ObligationAlreadyClaimed,
    /// The presented random delivery correlation ID matched no accepted wake.
    UnknownDeliveryId,
    /// An event from a superseded session incarnation cannot mutate current work.
    StaleSessionIncarnation,
    /// The wake's target obligation snapshot no longer matches current state.
    StaleDeliveryTarget,
    /// The event is not legal from the obligation's current state.
    IllegalObligationTransition,
    /// The obligation is closed and cannot be mutated further.
    ObligationClosed,
    /// The delivery revision reached `accepted`/`ambiguous` and is frozen.
    DeliveryRevisionFrozen,
    /// An earlier revision for the obligation has not reached a terminal state.
    DeliveryRevisionStillLive,
    /// A newer revision exists; the presented one may never act again.
    DeliveryRevisionSuperseded,
    /// The event is not legal from the delivery attempt's current state.
    IllegalDeliveryTransition,
    /// The referenced attempt does not exist on this delivery.
    UnknownAttempt,
    /// A retry was requested but the bounded budget is spent.
    RetryBudgetExhausted,
    /// A retry was requested after the Send ambiguity fence was armed.
    RetryAfterAmbiguityFence,
    /// `failed` was claimed without proof that no submission happened.
    FailureNotProven,
    /// The foreman surface has an active or unobserved turn.
    ForemanTurnNotQuiescent,
    /// A second, different answer was supplied for one input request.
    ConflictingInputAnswer,
    /// The input request is not in a state that accepts this event.
    IllegalInputTransition,
    /// A worker continuation revision was presented out of order.
    StaleCommandRevision,
    /// The proposed disposition cannot close this obligation.
    InvalidDisposition,

    // -- consequential external effects ([`crate::effect`]) ------------------
    /// An execution permit was requested without a durable-intent acceptance.
    ExecuteRequiresDurableIntent,
    /// The event is not legal from the external attempt's current state.
    IllegalAttemptTransition,
    /// The effect already landed; it must not be produced a second time.
    AttemptAlreadyCompleted,
    /// The dispatch fence is already committed, so a second permit is refused.
    AttemptAlreadyDispatched,
    /// A presented acceptance or permit belongs to a different attempt.
    AttemptPermitMismatch,
    /// `failed_before_effect` was claimed without proof of non-occurrence.
    EffectNotProvenAbsent,
    /// A retry was requested without the recorded contract and exact key.
    RetryRequiresIdempotencyContract,

    // -- mutation command receipts ([`crate::mutation`]) ---------------------
    /// The command was received but no safe result is committed. Never replay.
    MutationResultUncertain,
    /// A command identity was presented for a different operation.
    MutationCommandMismatch,
    /// The event is not legal from the command's current journal status.
    IllegalMutationTransition,
    /// A receipt ACK was presented for a command with no committed result.
    MutationNotCompleted,

    // -- resource ownership ([`crate::lease`]) -------------------------------
    /// The presented lease token is not the current lease's token.
    StaleLeaseToken,
    /// The presented process incarnation is not the one that holds the lease.
    StaleProcessIncarnation,
    /// The presented daemon epoch is older than the one that owns the record.
    StaleDaemonEpoch,
    /// A live lease already holds the resource exclusively.
    ResourceAlreadyLeased,
    /// The resource has no lease to renew or release.
    NoCurrentLease,
    /// The event is not legal from the lease's current state.
    IllegalLeaseTransition,
}

impl ConflictKind {
    /// Returns the stable `snake_case` code for this classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StaleBindingGeneration => "stale_binding_generation",
            Self::UnknownBindingGeneration => "unknown_binding_generation",
            Self::NoActiveBinding => "no_active_binding",
            Self::StaleObligationVersion => "stale_obligation_version",
            Self::StaleSourceFence => "stale_source_fence",
            Self::StaleClaim => "stale_claim",
            Self::NoCurrentClaim => "no_current_claim",
            Self::ExpiredClaim => "expired_claim",
            Self::ObligationAlreadyClaimed => "obligation_already_claimed",
            Self::UnknownDeliveryId => "unknown_delivery_id",
            Self::StaleSessionIncarnation => "stale_session_incarnation",
            Self::StaleDeliveryTarget => "stale_delivery_target",
            Self::IllegalObligationTransition => "illegal_obligation_transition",
            Self::ObligationClosed => "obligation_closed",
            Self::DeliveryRevisionFrozen => "delivery_revision_frozen",
            Self::DeliveryRevisionStillLive => "delivery_revision_still_live",
            Self::DeliveryRevisionSuperseded => "delivery_revision_superseded",
            Self::IllegalDeliveryTransition => "illegal_delivery_transition",
            Self::UnknownAttempt => "unknown_attempt",
            Self::RetryBudgetExhausted => "retry_budget_exhausted",
            Self::RetryAfterAmbiguityFence => "retry_after_ambiguity_fence",
            Self::FailureNotProven => "failure_not_proven",
            Self::ForemanTurnNotQuiescent => "foreman_turn_not_quiescent",
            Self::ConflictingInputAnswer => "conflicting_input_answer",
            Self::IllegalInputTransition => "illegal_input_transition",
            Self::StaleCommandRevision => "stale_command_revision",
            Self::InvalidDisposition => "invalid_disposition",
            Self::ExecuteRequiresDurableIntent => "execute_requires_durable_intent",
            Self::IllegalAttemptTransition => "illegal_attempt_transition",
            Self::AttemptAlreadyCompleted => "attempt_already_completed",
            Self::AttemptAlreadyDispatched => "attempt_already_dispatched",
            Self::AttemptPermitMismatch => "attempt_permit_mismatch",
            Self::EffectNotProvenAbsent => "effect_not_proven_absent",
            Self::RetryRequiresIdempotencyContract => "retry_requires_idempotency_contract",
            Self::MutationResultUncertain => "mutation_result_uncertain",
            Self::MutationCommandMismatch => "mutation_command_mismatch",
            Self::IllegalMutationTransition => "illegal_mutation_transition",
            Self::MutationNotCompleted => "mutation_not_completed",
            Self::StaleLeaseToken => "stale_lease_token",
            Self::StaleProcessIncarnation => "stale_process_incarnation",
            Self::StaleDaemonEpoch => "stale_daemon_epoch",
            Self::ResourceAlreadyLeased => "resource_already_leased",
            Self::NoCurrentLease => "no_current_lease",
            Self::IllegalLeaseTransition => "illegal_lease_transition",
        }
    }
}

/// A rejected domain transition.
///
/// Returning one of these always means *nothing changed*.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Conflict {
    /// A superseded binding generation tried to act on current work.
    #[error("stale binding generation {presented}, active generation is {active}")]
    StaleBindingGeneration {
        /// Generation the caller presented.
        presented: BindingGeneration,
        /// Generation currently active.
        active: BindingGeneration,
    },

    /// A binding generation ahead of the active one was presented.
    #[error("unknown binding generation {presented}, active generation is {active}")]
    UnknownBindingGeneration {
        /// Generation the caller presented.
        presented: BindingGeneration,
        /// Generation currently active.
        active: BindingGeneration,
    },

    /// There is no active foreman binding.
    #[error("no active foreman binding")]
    NoActiveBinding,

    /// The obligation moved on since the caller last observed it.
    #[error("stale obligation version {presented}, current version is {current}")]
    StaleObligationVersion {
        /// Version the caller presented.
        presented: ObligationVersion,
        /// Version the obligation is actually at.
        current: ObligationVersion,
    },

    /// The obligation's underlying source fact changed.
    #[error("stale source fence {presented}, current source is {current}")]
    StaleSourceFence {
        /// Source identity the caller presented.
        presented: Box<SourceRef>,
        /// Source identity currently backing the obligation.
        current: Box<SourceRef>,
    },

    /// The presented claim is not the obligation's current claim.
    #[error("stale claim {presented} on obligation {obligation}")]
    StaleClaim {
        /// Claim the caller presented.
        presented: ClaimId,
        /// Obligation the claim was presented against.
        obligation: ObligationId,
    },

    /// The obligation has no live claim.
    #[error("obligation {obligation} has no current claim")]
    NoCurrentClaim {
        /// Obligation that was addressed.
        obligation: ObligationId,
    },

    /// The claim has expired and can no longer authorise a mutation.
    #[error("claim {claim} expired")]
    ExpiredClaim {
        /// Claim that was presented.
        claim: ClaimId,
    },

    /// Another live claim already holds the obligation.
    #[error("obligation {obligation} is already claimed by {holder}")]
    ObligationAlreadyClaimed {
        /// Obligation that was addressed.
        obligation: ObligationId,
        /// Claim currently holding it.
        holder: ClaimId,
    },

    /// The presented wake correlation ID matched no accepted current delivery.
    ///
    /// Deliberately undifferentiated: a caller must not be able to learn
    /// whether a delivery exists by probing correlation IDs.
    #[error("unknown or non-current delivery correlation id")]
    UnknownDeliveryId,

    /// A delayed event from a replaced session incarnation.
    #[error("stale session incarnation {presented}, current is {current}")]
    StaleSessionIncarnation {
        /// Incarnation generation the event came from.
        presented: IncarnationGeneration,
        /// Incarnation generation currently live.
        current: IncarnationGeneration,
    },

    /// The wake's snapshot of its target no longer matches the obligation.
    #[error("delivery revision {revision} targets a superseded obligation snapshot")]
    StaleDeliveryTarget {
        /// Revision whose snapshot went stale.
        revision: DeliveryRevision,
    },

    /// The event is not a legal obligation transition.
    #[error("obligation event {event} is not legal from state {from:?}")]
    IllegalObligationTransition {
        /// State the obligation was in.
        from: ObligationState,
        /// Event class that was refused.
        event: &'static str,
    },

    /// The obligation is closed; only an idempotent repeat is accepted.
    #[error("obligation is closed in state {state:?}")]
    ObligationClosed {
        /// Closed state the obligation is in.
        state: ObligationState,
    },

    /// The delivery revision is frozen and must never be replayed.
    #[error("delivery revision is frozen in state {state:?} and is never resent")]
    DeliveryRevisionFrozen {
        /// Terminal projection state that froze the revision.
        state: DeliveryState,
    },

    /// Another revision for this obligation and generation is still live.
    ///
    /// At most one delivery revision may be able to produce an external
    /// effect at a time; a new revision may only be created once every
    /// earlier one has reached a terminal projection.
    #[error("delivery revision {live} for this obligation is still live")]
    DeliveryRevisionStillLive {
        /// The revision that has not reached a terminal projection.
        live: DeliveryRevision,
    },

    /// A newer revision exists, so this one may never act again.
    ///
    /// A failed revision with attempt budget left is normally retryable, but
    /// once a successor revision has been created the older one is
    /// superseded — resurrecting it would put two revisions in a position to
    /// produce the same external effect.
    #[error("delivery revision {presented} is superseded by revision {newest}")]
    DeliveryRevisionSuperseded {
        /// The revision the caller tried to act on.
        presented: DeliveryRevision,
        /// The newest revision recorded for the obligation and generation.
        newest: DeliveryRevision,
    },

    /// The event is not a legal delivery attempt transition.
    #[error("delivery event {event} is not legal from attempt state {from:?}")]
    IllegalDeliveryTransition {
        /// Attempt state the delivery was in.
        from: AttemptState,
        /// Event class that was refused.
        event: &'static str,
    },

    /// The referenced attempt does not exist.
    #[error("delivery has no attempt {attempt}")]
    UnknownAttempt {
        /// Attempt number that was referenced.
        attempt: AttemptNo,
    },

    /// The bounded retry budget for this revision is spent.
    #[error("retry budget of {budget} attempts is exhausted")]
    RetryBudgetExhausted {
        /// Configured attempt budget.
        budget: u32,
    },

    /// A retry was requested after the Send ambiguity fence had been armed.
    #[error("attempt {attempt} armed the Send fence, so retry is forbidden")]
    RetryAfterAmbiguityFence {
        /// Attempt that had armed the fence.
        attempt: AttemptNo,
    },

    /// `failed` requires proof that no submission occurred.
    #[error("failure is unproven from state {from:?}; the outcome is ambiguous")]
    FailureNotProven {
        /// Attempt state the claim of failure was made from.
        from: AttemptState,
    },

    /// The bound foreman surface is mid-turn or unobserved.
    #[error("foreman surface is in state {state:?}; no new wake may activate")]
    ForemanTurnNotQuiescent {
        /// Observed physical turn state.
        state: ForemanTurnState,
    },

    /// A different second answer arrived for one input request.
    #[error("input request already holds a different answer")]
    ConflictingInputAnswer,

    /// The event is not legal for the input request's current state.
    #[error("input event {event} is not legal from state {from:?}")]
    IllegalInputTransition {
        /// State the input request was in.
        from: InputRequestState,
        /// Event class that was refused.
        event: &'static str,
    },

    /// A worker continuation revision was proposed out of order.
    #[error("worker command revision {presented} is not the current revision {current}")]
    StaleCommandRevision {
        /// Revision the caller presented.
        presented: CommandRevision,
        /// Revision currently outstanding.
        current: CommandRevision,
    },

    /// The disposition cannot close this obligation.
    #[error("disposition is not valid for this obligation")]
    InvalidDisposition,

    /// An execution permit was requested without a durable-intent acceptance.
    ///
    /// This is the type-level "intent before I/O" rule failing closed: the
    /// intent row must be committed, and its acceptance presented, before any
    /// consequential call is authorised.
    #[error("attempt {attempt} has no accepted durable intent, so it cannot execute")]
    ExecuteRequiresDurableIntent {
        /// Attempt that was asked to execute.
        attempt: ExternalAttemptId,
    },

    /// The event is not a legal external-attempt transition.
    #[error("external attempt event {event} is not legal from state {from:?}")]
    IllegalAttemptTransition {
        /// State the attempt was in.
        from: ExternalAttemptState,
        /// Event class that was refused.
        event: &'static str,
    },

    /// The effect already landed and must not be produced again.
    #[error("attempt {attempt} already completed")]
    AttemptAlreadyCompleted {
        /// Attempt whose effect already landed.
        attempt: ExternalAttemptId,
    },

    /// The dispatch fence is committed, so a second permit is refused.
    #[error("attempt {attempt} already crossed the dispatch fence")]
    AttemptAlreadyDispatched {
        /// Attempt that already dispatched.
        attempt: ExternalAttemptId,
    },

    /// A presented acceptance or permit belongs to a different attempt.
    #[error("presented acceptance is for attempt {presented}, not {attempt}")]
    AttemptPermitMismatch {
        /// Attempt the acceptance vouches for.
        presented: ExternalAttemptId,
        /// Attempt it was presented against.
        attempt: ExternalAttemptId,
    },

    /// `failed_before_effect` requires proof that the effect did not happen.
    #[error(
        "proof {proof:?} does not establish absence for attempt {attempt} (dispatched: {dispatched})"
    )]
    EffectNotProvenAbsent {
        /// Attempt the claim was made about.
        attempt: ExternalAttemptId,
        /// Proof class that was offered.
        proof: NoEffectClass,
        /// Whether the dispatch fence had been committed.
        dispatched: bool,
    },

    /// A retry was requested without the recorded contract and exact key.
    ///
    /// Also the answer for an ambiguous non-idempotent write, which has no
    /// contract at all and therefore no admissible automatic retry.
    #[error("attempt {attempt} may not be retried without its recorded idempotency contract")]
    RetryRequiresIdempotencyContract {
        /// Attempt whose fate is unknown.
        attempt: ExternalAttemptId,
    },

    /// The command was received but no safe result is committed.
    ///
    /// The caller must surface the uncertainty. It must never redispatch.
    #[error("mutation {command} for actor {actor} has no committed result")]
    MutationResultUncertain {
        /// Actor that issued the command.
        actor: ActorId,
        /// Command identity that is uncertain.
        command: MutationCommandId,
    },

    /// A command identity was presented for a different operation.
    #[error("mutation {command} for actor {actor} was minted for a different operation")]
    MutationCommandMismatch {
        /// Actor that was presented.
        actor: ActorId,
        /// Command identity that was presented.
        command: MutationCommandId,
    },

    /// The event is not legal from the command's current journal status.
    #[error("mutation event {event} is not legal from status {from:?}")]
    IllegalMutationTransition {
        /// Journal status the command was in.
        from: MutationCommandStatus,
        /// Event class that was refused.
        event: &'static str,
    },

    /// A receipt ACK was presented for a command with no committed result.
    #[error("mutation {command} for actor {actor} has no committed result to acknowledge")]
    MutationNotCompleted {
        /// Actor that was presented.
        actor: ActorId,
        /// Command identity that was presented.
        command: MutationCommandId,
    },

    /// The presented lease token is not the current lease's token.
    #[error("presented token is not the token of lease {lease}")]
    StaleLeaseToken {
        /// Lease that currently owns the resource.
        lease: ResourceLeaseId,
    },

    /// The presented process incarnation is not the lease holder's.
    #[error("lease {lease} is held by a different process incarnation ({mismatch:?})")]
    StaleProcessIncarnation {
        /// Lease that was addressed.
        lease: ResourceLeaseId,
        /// How the presented incarnation differed.
        mismatch: IncarnationMismatch,
    },

    /// The presented daemon epoch is older than the record's.
    #[error("stale daemon epoch {presented}, record is at epoch {current}")]
    StaleDaemonEpoch {
        /// Epoch the caller presented.
        presented: DaemonEpoch,
        /// Epoch the record was written under.
        current: DaemonEpoch,
    },

    /// A live lease already holds the resource exclusively.
    #[error("resource is already leased by {holder} under lease {lease}")]
    ResourceAlreadyLeased {
        /// Lease currently holding the resource.
        lease: ResourceLeaseId,
        /// Semantic holder of that lease.
        holder: ActorId,
    },

    /// The resource has no lease to renew or release.
    #[error("resource has no current lease")]
    NoCurrentLease,

    /// The event is not legal from the lease's current state.
    #[error("lease event {event} is not legal from state {from:?}")]
    IllegalLeaseTransition {
        /// State the lease was in.
        from: LeaseState,
        /// Event class that was refused.
        event: &'static str,
    },
}

impl Conflict {
    /// Returns the stable machine-readable classification.
    #[must_use]
    pub const fn kind(&self) -> ConflictKind {
        match self {
            Self::StaleBindingGeneration { .. } => ConflictKind::StaleBindingGeneration,
            Self::UnknownBindingGeneration { .. } => ConflictKind::UnknownBindingGeneration,
            Self::NoActiveBinding => ConflictKind::NoActiveBinding,
            Self::StaleObligationVersion { .. } => ConflictKind::StaleObligationVersion,
            Self::StaleSourceFence { .. } => ConflictKind::StaleSourceFence,
            Self::StaleClaim { .. } => ConflictKind::StaleClaim,
            Self::NoCurrentClaim { .. } => ConflictKind::NoCurrentClaim,
            Self::ExpiredClaim { .. } => ConflictKind::ExpiredClaim,
            Self::ObligationAlreadyClaimed { .. } => ConflictKind::ObligationAlreadyClaimed,
            Self::UnknownDeliveryId => ConflictKind::UnknownDeliveryId,
            Self::StaleSessionIncarnation { .. } => ConflictKind::StaleSessionIncarnation,
            Self::StaleDeliveryTarget { .. } => ConflictKind::StaleDeliveryTarget,
            Self::IllegalObligationTransition { .. } => ConflictKind::IllegalObligationTransition,
            Self::ObligationClosed { .. } => ConflictKind::ObligationClosed,
            Self::DeliveryRevisionFrozen { .. } => ConflictKind::DeliveryRevisionFrozen,
            Self::DeliveryRevisionStillLive { .. } => ConflictKind::DeliveryRevisionStillLive,
            Self::DeliveryRevisionSuperseded { .. } => ConflictKind::DeliveryRevisionSuperseded,
            Self::IllegalDeliveryTransition { .. } => ConflictKind::IllegalDeliveryTransition,
            Self::UnknownAttempt { .. } => ConflictKind::UnknownAttempt,
            Self::RetryBudgetExhausted { .. } => ConflictKind::RetryBudgetExhausted,
            Self::RetryAfterAmbiguityFence { .. } => ConflictKind::RetryAfterAmbiguityFence,
            Self::FailureNotProven { .. } => ConflictKind::FailureNotProven,
            Self::ForemanTurnNotQuiescent { .. } => ConflictKind::ForemanTurnNotQuiescent,
            Self::ConflictingInputAnswer => ConflictKind::ConflictingInputAnswer,
            Self::IllegalInputTransition { .. } => ConflictKind::IllegalInputTransition,
            Self::StaleCommandRevision { .. } => ConflictKind::StaleCommandRevision,
            Self::InvalidDisposition => ConflictKind::InvalidDisposition,
            Self::ExecuteRequiresDurableIntent { .. } => ConflictKind::ExecuteRequiresDurableIntent,
            Self::IllegalAttemptTransition { .. } => ConflictKind::IllegalAttemptTransition,
            Self::AttemptAlreadyCompleted { .. } => ConflictKind::AttemptAlreadyCompleted,
            Self::AttemptAlreadyDispatched { .. } => ConflictKind::AttemptAlreadyDispatched,
            Self::AttemptPermitMismatch { .. } => ConflictKind::AttemptPermitMismatch,
            Self::EffectNotProvenAbsent { .. } => ConflictKind::EffectNotProvenAbsent,
            Self::RetryRequiresIdempotencyContract { .. } => {
                ConflictKind::RetryRequiresIdempotencyContract
            }
            Self::MutationResultUncertain { .. } => ConflictKind::MutationResultUncertain,
            Self::MutationCommandMismatch { .. } => ConflictKind::MutationCommandMismatch,
            Self::IllegalMutationTransition { .. } => ConflictKind::IllegalMutationTransition,
            Self::MutationNotCompleted { .. } => ConflictKind::MutationNotCompleted,
            Self::StaleLeaseToken { .. } => ConflictKind::StaleLeaseToken,
            Self::StaleProcessIncarnation { .. } => ConflictKind::StaleProcessIncarnation,
            Self::StaleDaemonEpoch { .. } => ConflictKind::StaleDaemonEpoch,
            Self::ResourceAlreadyLeased { .. } => ConflictKind::ResourceAlreadyLeased,
            Self::NoCurrentLease => ConflictKind::NoCurrentLease,
            Self::IllegalLeaseTransition { .. } => ConflictKind::IllegalLeaseTransition,
        }
    }

    /// Returns the stable `snake_case` code for this conflict.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind().code()
    }
}

/// Outcome of applying an event to a pure state machine.
///
/// `Advanced` carries a *new* state value; the caller's previous value is
/// untouched, which is how "a rejected or duplicate event mutates nothing" is
/// enforced structurally rather than by discipline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition<S> {
    /// The event was accepted and produced this new state.
    Advanced(S),
    /// The event was an exact duplicate of one already applied. No change.
    Duplicate,
}

impl<S> Transition<S> {
    /// Returns the new state, or `None` for an idempotent duplicate.
    pub fn advanced(self) -> Option<S> {
        match self {
            Self::Advanced(state) => Some(state),
            Self::Duplicate => None,
        }
    }

    /// Returns the new state, falling back to `previous` for a duplicate.
    pub fn or_unchanged(self, previous: S) -> S {
        self.advanced().unwrap_or(previous)
    }

    /// Reports whether the event was an idempotent duplicate.
    pub const fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate)
    }
}

/// Shorthand for a pure transition result.
pub type Outcome<S> = Result<Transition<S>, Conflict>;
