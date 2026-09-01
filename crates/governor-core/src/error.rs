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

use crate::fence::{
    AttemptNo, BindingGeneration, CommandRevision, DeliveryRevision, IncarnationGeneration,
    ObligationVersion, SourceRef,
};
use crate::foreman_turn::ForemanTurnState;
use crate::id::{ClaimId, ObligationId};
use crate::input::InputRequestState;
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
