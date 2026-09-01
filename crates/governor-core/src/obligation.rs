//! The durable obligation: what work is still owed.
//!
//! ```text
//! created -> running -> needs_input | failed | completed_unprocessed
//!                              -> claimed_by_foreman -> processing -> acknowledged
//! ```
//!
//! The mission invariant lives here. Worker completion, browser delivery, and
//! ChatGPT settlement do **not** close an obligation; only an explicit fenced
//! disposition does, and the only events that produce a closed state are
//! [`ObligationEvent::ForemanAcked`] with a valid disposition,
//! [`ObligationEvent::CancelledByUser`], and [`ObligationEvent::Superseded`].
//!
//! `suspected_stall` is deliberately absent from [`ObligationState`]: it is a
//! [`crate::health::HealthConditionKind`] layered on `running`, exactly as the
//! data model stores it, so it has no way to become a closing state.
//!
//! Every transition takes `&self` and returns a new value, so a rejected event
//! provably mutates nothing.

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{BindingGeneration, IncarnationGeneration, ObligationVersion, SourceRef};
use crate::id::{ClaimId, InputRequestId, ObligationId, ResultArtifactId, TaskId, TurnId};
use crate::input::ConfirmedDefer;
use crate::time::Timestamp;
use crate::worker_command::ConfirmedResumedTurn;
use crate::worker_evidence::{ConfirmedFinalResult, WorkerFailureClass};

/// Lifecycle state of an obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObligationState {
    /// Recorded, worker start not yet verified.
    Created,
    /// A worker turn is verified to be running.
    Running,
    /// A confirmed durable defer boundary is waiting for an answer.
    NeedsInput,
    /// A verified terminal worker failure. Unprocessed work, not a closure.
    Failed,
    /// A confirmed final result with a durable artifact. Still open.
    CompletedUnprocessed,
    /// A current foreman claim holds the obligation.
    ClaimedByForeman,
    /// The result or input request has been handed to the current foreman.
    Processing,
    /// Closed by an explicit fenced disposition.
    Acknowledged,
    /// Closed by the user.
    CancelledByUser,
    /// Closed because a later obligation replaced it.
    Superseded,
}

impl ObligationState {
    /// Reports whether the obligation still owes somebody something.
    #[must_use]
    pub const fn is_open(self) -> bool {
        !self.is_closed()
    }

    /// Reports whether the obligation has been closed by a disposition.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(
            self,
            Self::Acknowledged | Self::CancelledByUser | Self::Superseded
        )
    }

    /// Reports whether the obligation is in a foreman-claimable attention state.
    #[must_use]
    pub const fn attention(self) -> Option<AttentionState> {
        match self {
            Self::NeedsInput => Some(AttentionState::NeedsInput),
            Self::Failed => Some(AttentionState::Failed),
            Self::CompletedUnprocessed => Some(AttentionState::CompletedUnprocessed),
            _ => None,
        }
    }
}

/// The subset of open states a foreman may claim.
///
/// Claim expiry returns the obligation to exactly the one it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttentionState {
    /// Input is owed.
    NeedsInput,
    /// A verified worker failure awaits disposition.
    Failed,
    /// A confirmed result awaits review.
    CompletedUnprocessed,
}

impl From<AttentionState> for ObligationState {
    fn from(value: AttentionState) -> Self {
        match value {
            AttentionState::NeedsInput => Self::NeedsInput,
            AttentionState::Failed => Self::Failed,
            AttentionState::CompletedUnprocessed => Self::CompletedUnprocessed,
        }
    }
}

/// What an obligation is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ObligationKind {
    /// Work delegated to a worker turn, awaiting foreman review.
    WorkerTurn,
}

/// The semantic decision a foreman makes when closing an obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Disposition {
    /// The result was reviewed and accepted.
    Accepted,
    /// The result was reviewed and rejected; follow-up work is separate.
    RejectedNeedsRework,
    /// A worker failure was reviewed and acknowledged.
    FailureAcknowledged,
    /// The work is being abandoned deliberately.
    Abandoned,
}

impl Disposition {
    /// Reports whether this disposition may close an obligation claimed from
    /// `attention`.
    ///
    /// A success disposition cannot close a failure, a failure disposition
    /// cannot close a successful result, and an outstanding input request can
    /// only be closed by abandoning it — answering it is not a closure.
    #[must_use]
    pub const fn closes(self, attention: AttentionState) -> bool {
        match attention {
            AttentionState::CompletedUnprocessed => matches!(
                self,
                Self::Accepted | Self::RejectedNeedsRework | Self::Abandoned
            ),
            AttentionState::Failed => matches!(
                self,
                Self::FailureAcknowledged | Self::RejectedNeedsRework | Self::Abandoned
            ),
            AttentionState::NeedsInput => matches!(self, Self::Abandoned),
        }
    }
}

/// The exact fences an ACK must present.
///
/// Every field is checked and any mismatch is a typed conflict with zero
/// mutation ([`docs/adr/0004-foreman-mcp-and-binding.md`], "ACK semantics").
///
/// [`docs/adr/0004-foreman-mcp-and-binding.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/adr/0004-foreman-mcp-and-binding.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckRequest {
    /// Obligation being closed.
    pub obligation: ObligationId,
    /// Exact current obligation version.
    pub expected_version: ObligationVersion,
    /// Exact current source fact backing the obligation.
    pub expected_source: SourceRef,
    /// Current binding generation.
    pub binding_generation: BindingGeneration,
    /// Current foreman claim.
    pub claim: ClaimId,
    /// Semantic decision.
    pub disposition: Disposition,
    /// Observation instant.
    pub at: Timestamp,
}

/// The ACK that actually closed an obligation.
///
/// Retained so an exact repeat can return idempotent success rather than a
/// stale-version conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedAck {
    version_at_ack: ObligationVersion,
    source_at_ack: SourceRef,
    binding_generation: BindingGeneration,
    claim: ClaimId,
    disposition: Disposition,
}

impl CommittedAck {
    /// Obligation version the ACK presented.
    #[must_use]
    pub const fn version_at_ack(&self) -> ObligationVersion {
        self.version_at_ack
    }

    /// Disposition that closed the obligation.
    #[must_use]
    pub const fn disposition(&self) -> Disposition {
        self.disposition
    }

    /// Claim the ACK was made under.
    #[must_use]
    pub const fn claim(&self) -> ClaimId {
        self.claim
    }

    /// Reports whether `request` is an exact repeat of this committed ACK.
    ///
    /// Every fence must match, the claim included, so a different caller
    /// cannot ride an earlier ACK's idempotency.
    #[must_use]
    pub fn matches(&self, request: &AckRequest) -> bool {
        self.version_at_ack == request.expected_version
            && self.source_at_ack == request.expected_source
            && self.binding_generation == request.binding_generation
            && self.claim == request.claim
            && self.disposition == request.disposition
    }
}

/// An event applied to an obligation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ObligationEvent {
    /// A worker turn was verified to have started, or re-attached.
    WorkerStarted {
        /// Source fact behind the observation.
        source: SourceRef,
        /// Session incarnation the observation came from.
        incarnation: IncarnationGeneration,
        /// Observation instant.
        at: Timestamp,
    },
    /// A confirmed durable defer boundary produced an input request.
    ///
    /// Carrying [`ConfirmedDefer`] by value is the point: there is no path to
    /// `needs_input` from a multi-tool or unconfirmed defer.
    InputBoundaryConfirmed {
        /// Source fact behind the observation.
        source: SourceRef,
        /// Session incarnation the observation came from.
        incarnation: IncarnationGeneration,
        /// The input request that was opened.
        input_request: InputRequestId,
        /// Proof the defer actually took effect.
        defer: ConfirmedDefer,
        /// Observation instant.
        at: Timestamp,
    },
    /// A verified terminal worker failure. This is unprocessed work.
    WorkerFailed {
        /// Source fact behind the observation.
        source: SourceRef,
        /// Session incarnation the observation came from.
        incarnation: IncarnationGeneration,
        /// Documented failure class.
        failure: WorkerFailureClass,
        /// Observation instant.
        at: Timestamp,
    },
    /// A confirmed final result whose artifact is already durable.
    ///
    /// Requires [`ConfirmedFinalResult`], which only
    /// [`crate::worker_evidence::ManagedRunEvidence::classify`] can produce, so
    /// a `Stop` callback — vetoed or not — cannot reach this state.
    ResultPublished {
        /// Source fact behind the observation.
        source: SourceRef,
        /// Session incarnation the observation came from.
        incarnation: IncarnationGeneration,
        /// Proof of a complete run with a matching exit.
        proof: ConfirmedFinalResult,
        /// Artifact made durable before this event was committed.
        artifact: ResultArtifactId,
        /// Observation instant.
        at: Timestamp,
    },
    /// Matching resumed-turn evidence returned the worker to running.
    WorkerResumed {
        /// Source fact behind the observation.
        source: SourceRef,
        /// Session incarnation the observation came from.
        incarnation: IncarnationGeneration,
        /// Proof the worker actually resumed with the answer.
        proof: ConfirmedResumedTurn,
        /// Observation instant.
        at: Timestamp,
    },
    /// A foreman claim was minted against the current version and source.
    ForemanClaimed {
        /// The new claim.
        claim: ClaimId,
        /// Binding generation the claim was minted under.
        binding_generation: BindingGeneration,
        /// Version the caller presented.
        expected_version: ObligationVersion,
        /// Source fact the caller presented.
        expected_source: SourceRef,
        /// Observation instant.
        at: Timestamp,
    },
    /// The result or input request was handed to the claiming foreman.
    HandoffDelivered {
        /// Claim the handoff belongs to.
        claim: ClaimId,
        /// Observation instant.
        at: Timestamp,
    },
    /// An explicit fenced disposition. The only normal way work closes.
    ForemanAcked(Box<AckRequest>),
    /// A claim's bound lifetime elapsed.
    ///
    /// Internal coordination only: it returns the obligation to the attention
    /// state it came from and can never close it or release an artifact.
    ClaimExpired {
        /// Claim that expired.
        claim: ClaimId,
        /// Observation instant.
        at: Timestamp,
    },
    /// The user cancelled the work.
    CancelledByUser {
        /// Source fact behind the decision.
        source: SourceRef,
        /// Observation instant.
        at: Timestamp,
    },
    /// A later obligation replaced this one.
    Superseded {
        /// Source fact behind the decision.
        source: SourceRef,
        /// Obligation that replaced this one.
        replacement: ObligationId,
        /// Observation instant.
        at: Timestamp,
    },
}

impl ObligationEvent {
    const fn label(&self) -> &'static str {
        match self {
            Self::WorkerStarted { .. } => "worker_started",
            Self::InputBoundaryConfirmed { .. } => "input_boundary_confirmed",
            Self::WorkerFailed { .. } => "worker_failed",
            Self::ResultPublished { .. } => "result_published",
            Self::WorkerResumed { .. } => "worker_resumed",
            Self::ForemanClaimed { .. } => "foreman_claimed",
            Self::HandoffDelivered { .. } => "handoff_delivered",
            Self::ForemanAcked(_) => "foreman_acked",
            Self::ClaimExpired { .. } => "claim_expired",
            Self::CancelledByUser { .. } => "cancelled_by_user",
            Self::Superseded { .. } => "superseded",
        }
    }

    const fn worker_fence(&self) -> Option<(&SourceRef, IncarnationGeneration)> {
        match self {
            Self::WorkerStarted {
                source,
                incarnation,
                ..
            }
            | Self::InputBoundaryConfirmed {
                source,
                incarnation,
                ..
            }
            | Self::WorkerFailed {
                source,
                incarnation,
                ..
            }
            | Self::ResultPublished {
                source,
                incarnation,
                ..
            }
            | Self::WorkerResumed {
                source,
                incarnation,
                ..
            } => Some((source, *incarnation)),
            _ => None,
        }
    }
}

/// A durable obligation projection.
///
/// # Construction is replay
///
/// The fields are private and there is no field-wise constructor: the only way
/// to reach any state but `created` is to fold [`ObligationEvent`]s over
/// [`Obligation::created`]. That is deliberate, and the store depends on it —
/// a projection built by replay cannot be internally inconsistent, which is why
/// the two `prior_attention` reads inside [`Obligation::apply`] are
/// unreachable rather than merely unlikely.
///
/// A store that wants to rehydrate a row *without* replaying its events would
/// need a validating constructor added here, returning an error for a row whose
/// state and fences disagree. Adding an unchecked one would make those reads
/// reachable and must not be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obligation {
    id: ObligationId,
    task: TaskId,
    turn: Option<TurnId>,
    kind: ObligationKind,
    state: ObligationState,
    version: ObligationVersion,
    source: SourceRef,
    incarnation: IncarnationGeneration,
    result_artifact: Option<ResultArtifactId>,
    input_request: Option<InputRequestId>,
    prior_attention: Option<AttentionState>,
    binding_generation: Option<BindingGeneration>,
    claim: Option<ClaimId>,
    committed_ack: Option<CommittedAck>,
}

impl Obligation {
    /// Creates an obligation in `created`.
    #[must_use]
    pub const fn created(
        id: ObligationId,
        task: TaskId,
        turn: Option<TurnId>,
        kind: ObligationKind,
        source: SourceRef,
        incarnation: IncarnationGeneration,
    ) -> Self {
        Self {
            id,
            task,
            turn,
            kind,
            state: ObligationState::Created,
            version: ObligationVersion::FIRST,
            source,
            incarnation,
            result_artifact: None,
            input_request: None,
            prior_attention: None,
            binding_generation: None,
            claim: None,
            committed_ack: None,
        }
    }

    /// Obligation identity.
    #[must_use]
    pub const fn id(&self) -> ObligationId {
        self.id
    }

    /// Task this obligation belongs to.
    #[must_use]
    pub const fn task(&self) -> TaskId {
        self.task
    }

    /// Turn this obligation belongs to, if any.
    #[must_use]
    pub const fn turn(&self) -> Option<TurnId> {
        self.turn
    }

    /// What the obligation is about.
    #[must_use]
    pub const fn kind(&self) -> ObligationKind {
        self.kind
    }

    /// Current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> ObligationState {
        self.state
    }

    /// Compare-and-swap version. Every accepted transition advances it.
    #[must_use]
    pub const fn version(&self) -> ObligationVersion {
        self.version
    }

    /// Current source fact backing the obligation.
    #[must_use]
    pub const fn source(&self) -> &SourceRef {
        &self.source
    }

    /// Session incarnation the obligation's worker facts came from.
    #[must_use]
    pub const fn incarnation(&self) -> IncarnationGeneration {
        self.incarnation
    }

    /// Artifact this obligation requires for review, if any.
    #[must_use]
    pub const fn result_artifact(&self) -> Option<ResultArtifactId> {
        self.result_artifact
    }

    /// Input request this obligation is blocked on, if any.
    #[must_use]
    pub const fn input_request(&self) -> Option<InputRequestId> {
        self.input_request
    }

    /// Current claim, if one is held.
    #[must_use]
    pub const fn claim(&self) -> Option<ClaimId> {
        self.claim
    }

    /// Binding generation of the current claim, if one is held.
    #[must_use]
    pub const fn binding_generation(&self) -> Option<BindingGeneration> {
        self.binding_generation
    }

    /// The ACK that closed this obligation, if it is closed by one.
    #[must_use]
    pub const fn committed_ack(&self) -> Option<&CommittedAck> {
        self.committed_ack.as_ref()
    }

    /// Reports whether the obligation still owes somebody something.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// The attention state a foreman could claim right now, if any.
    #[must_use]
    pub const fn attention(&self) -> Option<AttentionState> {
        self.state.attention()
    }

    /// Verifies a presented version against the current one.
    ///
    /// # Errors
    ///
    /// Returns [`Conflict::StaleObligationVersion`] on any mismatch.
    pub const fn require_version(&self, presented: ObligationVersion) -> Result<(), Conflict> {
        if presented.get() == self.version.get() {
            Ok(())
        } else {
            Err(Conflict::StaleObligationVersion {
                presented,
                current: self.version,
            })
        }
    }

    /// Verifies a presented source fence against the current one.
    ///
    /// # Errors
    ///
    /// Returns [`Conflict::StaleSourceFence`] on any mismatch.
    pub fn require_source(&self, presented: &SourceRef) -> Result<(), Conflict> {
        if presented == &self.source {
            Ok(())
        } else {
            Err(Conflict::StaleSourceFence {
                presented: Box::new(presented.clone()),
                current: Box::new(self.source.clone()),
            })
        }
    }

    /// Verifies a presented claim against the current one.
    ///
    /// # Errors
    ///
    /// - [`Conflict::NoCurrentClaim`] when nothing holds the obligation;
    /// - [`Conflict::StaleClaim`] when a different claim holds it.
    pub fn require_claim(&self, presented: ClaimId) -> Result<(), Conflict> {
        match self.claim {
            None => Err(Conflict::NoCurrentClaim {
                obligation: self.id,
            }),
            Some(current) if current == presented => Ok(()),
            Some(_) => Err(Conflict::StaleClaim {
                presented,
                obligation: self.id,
            }),
        }
    }

    fn advance(&self, state: ObligationState) -> Self {
        let mut next = self.clone();
        next.state = state;
        next.version = self.version.next();
        next
    }

    fn require_current_incarnation(
        &self,
        presented: IncarnationGeneration,
    ) -> Result<(), Conflict> {
        if presented.get() < self.incarnation.get() {
            Err(Conflict::StaleSessionIncarnation {
                presented,
                current: self.incarnation,
            })
        } else {
            Ok(())
        }
    }

    /// Applies an event, returning a new obligation or a typed conflict.
    ///
    /// # Errors
    ///
    /// Returns the [`Conflict`] describing why the event cannot apply. The
    /// receiver is never mutated.
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive match over the lifecycle is clearer than \
                  scattering the legality rules across helpers"
    )]
    pub fn apply(&self, event: &ObligationEvent) -> Outcome<Self> {
        // A closed obligation accepts nothing but an exact ACK repeat.
        if self.state.is_closed() {
            if let ObligationEvent::ForemanAcked(request) = event
                && self
                    .committed_ack
                    .as_ref()
                    .is_some_and(|ack| ack.matches(request))
            {
                return Ok(Transition::Duplicate);
            }
            return Err(Conflict::ObligationClosed { state: self.state });
        }

        // Delayed events from a replaced incarnation are history, never a
        // mutation of current work.
        if let Some((source, incarnation)) = event.worker_fence() {
            self.require_current_incarnation(incarnation)?;
            if source == &self.source {
                // The same source fact has already been applied.
                return Ok(Transition::Duplicate);
            }
        }

        match event {
            ObligationEvent::WorkerStarted {
                source,
                incarnation,
                ..
            } => match self.state {
                ObligationState::Created | ObligationState::Running => {
                    let mut next = self.advance(ObligationState::Running);
                    next.source = source.clone();
                    next.incarnation = *incarnation;
                    Ok(Transition::Advanced(next))
                }
                from => Err(Conflict::IllegalObligationTransition {
                    from,
                    event: event.label(),
                }),
            },

            ObligationEvent::InputBoundaryConfirmed {
                source,
                input_request,
                ..
            } => match self.state {
                ObligationState::Running => {
                    let mut next = self.advance(ObligationState::NeedsInput);
                    next.source = source.clone();
                    next.input_request = Some(*input_request);
                    Ok(Transition::Advanced(next))
                }
                from => Err(Conflict::IllegalObligationTransition {
                    from,
                    event: event.label(),
                }),
            },

            ObligationEvent::WorkerFailed { source, .. } => match self.state {
                ObligationState::Running | ObligationState::NeedsInput => {
                    let mut next = self.advance(ObligationState::Failed);
                    next.source = source.clone();
                    Ok(Transition::Advanced(next))
                }
                from => Err(Conflict::IllegalObligationTransition {
                    from,
                    event: event.label(),
                }),
            },

            ObligationEvent::ResultPublished {
                source, artifact, ..
            } => match self.state {
                ObligationState::Running | ObligationState::NeedsInput => {
                    let mut next = self.advance(ObligationState::CompletedUnprocessed);
                    next.source = source.clone();
                    next.result_artifact = Some(*artifact);
                    Ok(Transition::Advanced(next))
                }
                from => Err(Conflict::IllegalObligationTransition {
                    from,
                    event: event.label(),
                }),
            },

            ObligationEvent::WorkerResumed { source, .. } => match self.state {
                ObligationState::NeedsInput => {
                    let mut next = self.advance(ObligationState::Running);
                    next.source = source.clone();
                    next.input_request = None;
                    Ok(Transition::Advanced(next))
                }
                from => Err(Conflict::IllegalObligationTransition {
                    from,
                    event: event.label(),
                }),
            },

            ObligationEvent::ForemanClaimed {
                claim,
                binding_generation,
                expected_version,
                expected_source,
                ..
            } => {
                let Some(attention) = self.state.attention() else {
                    if let Some(holder) = self.claim {
                        return Err(Conflict::ObligationAlreadyClaimed {
                            obligation: self.id,
                            holder,
                        });
                    }
                    return Err(Conflict::IllegalObligationTransition {
                        from: self.state,
                        event: event.label(),
                    });
                };
                self.require_version(*expected_version)?;
                self.require_source(expected_source)?;
                let mut next = self.advance(ObligationState::ClaimedByForeman);
                next.prior_attention = Some(attention);
                next.claim = Some(*claim);
                next.binding_generation = Some(*binding_generation);
                Ok(Transition::Advanced(next))
            }

            ObligationEvent::HandoffDelivered { claim, .. } => match self.state {
                ObligationState::ClaimedByForeman => {
                    self.require_claim(*claim)?;
                    Ok(Transition::Advanced(
                        self.advance(ObligationState::Processing),
                    ))
                }
                ObligationState::Processing if self.claim == Some(*claim) => {
                    Ok(Transition::Duplicate)
                }
                from => Err(Conflict::IllegalObligationTransition {
                    from,
                    event: event.label(),
                }),
            },

            ObligationEvent::ForemanAcked(request) => self.acknowledge(request, event.label()),

            ObligationEvent::ClaimExpired { claim, .. } => match self.state {
                ObligationState::ClaimedByForeman | ObligationState::Processing => {
                    self.require_claim(*claim)?;
                    let restored = self
                        .prior_attention
                        .expect("a claimed obligation always records its prior attention state");
                    let mut next = self.advance(restored.into());
                    next.claim = None;
                    next.binding_generation = None;
                    next.prior_attention = None;
                    Ok(Transition::Advanced(next))
                }
                // Expiry of a claim that is already released changes nothing.
                _ => Ok(Transition::Duplicate),
            },

            ObligationEvent::CancelledByUser { source, .. } => {
                let mut next = self.advance(ObligationState::CancelledByUser);
                next.source = source.clone();
                next.claim = None;
                Ok(Transition::Advanced(next))
            }

            ObligationEvent::Superseded { source, .. } => {
                let mut next = self.advance(ObligationState::Superseded);
                next.source = source.clone();
                next.claim = None;
                Ok(Transition::Advanced(next))
            }
        }
    }

    fn acknowledge(&self, request: &AckRequest, label: &'static str) -> Outcome<Self> {
        if self.state != ObligationState::Processing {
            return Err(Conflict::IllegalObligationTransition {
                from: self.state,
                event: label,
            });
        }
        if request.obligation != self.id {
            return Err(Conflict::StaleClaim {
                presented: request.claim,
                obligation: self.id,
            });
        }
        self.require_version(request.expected_version)?;
        self.require_source(&request.expected_source)?;
        self.require_claim(request.claim)?;
        match self.binding_generation {
            Some(active) if active == request.binding_generation => {}
            Some(active) => {
                return Err(Conflict::StaleBindingGeneration {
                    presented: request.binding_generation,
                    active,
                });
            }
            None => return Err(Conflict::NoActiveBinding),
        }
        let attention = self
            .prior_attention
            .expect("a processing obligation always records its prior attention state");
        if !request.disposition.closes(attention) {
            return Err(Conflict::InvalidDisposition);
        }

        let mut next = self.advance(ObligationState::Acknowledged);
        next.committed_ack = Some(CommittedAck {
            version_at_ack: request.expected_version,
            source_at_ack: request.expected_source.clone(),
            binding_generation: request.binding_generation,
            claim: request.claim,
            disposition: request.disposition,
        });
        next.claim = None;
        Ok(Transition::Advanced(next))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{Obligation, ObligationEvent, ObligationKind, ObligationState};
    use crate::fence::{IncarnationGeneration, SourceRef, test_support::source};
    use crate::id::{ObligationId, ResultArtifactId, TaskId, TurnId};
    use crate::time::Timestamp;
    use crate::worker_evidence::test_support as evidence_support;
    use uuid::Uuid;

    pub(crate) fn obligation_id() -> ObligationId {
        ObligationId::from_uuid(Uuid::from_u128(11))
    }

    pub(crate) fn artifact_id() -> ResultArtifactId {
        ResultArtifactId::from_uuid(Uuid::from_u128(41))
    }

    pub(crate) fn defer_source() -> SourceRef {
        source("claude.hook", "pretooluse-1", "defer")
    }

    pub(crate) fn created() -> Obligation {
        Obligation::created(
            obligation_id(),
            TaskId::from_uuid(Uuid::from_u128(1)),
            Some(TurnId::from_uuid(Uuid::from_u128(21))),
            ObligationKind::WorkerTurn,
            source("cg.internal", "obl-created", "v1"),
            IncarnationGeneration::FIRST,
        )
    }

    pub(crate) fn running() -> Obligation {
        created()
            .apply(&ObligationEvent::WorkerStarted {
                source: source("claude.init", "run-1", "start"),
                incarnation: IncarnationGeneration::FIRST,
                at: Timestamp::from_unix_millis(1),
            })
            .expect("start is legal from created")
            .advanced()
            .expect("start advances")
    }

    /// An obligation in `completed_unprocessed` with a durable artifact.
    pub(crate) fn completed() -> Obligation {
        running()
            .apply(&ObligationEvent::ResultPublished {
                source: source("claude.result", "run-1", "final"),
                incarnation: IncarnationGeneration::FIRST,
                proof: evidence_support::confirmed_completion(),
                artifact: artifact_id(),
                at: Timestamp::from_unix_millis(2),
            })
            .expect("publication is legal from running")
            .advanced()
            .expect("publication advances")
    }

    pub(crate) fn cancelled(from: &Obligation) -> Obligation {
        from.apply(&ObligationEvent::CancelledByUser {
            source: source("cg.cli", "cancel-1", "user"),
            at: Timestamp::from_unix_millis(9),
        })
        .expect("cancellation is legal while open")
        .advanced()
        .expect("cancellation advances")
    }

    /// A fully closed obligation, for retention tests.
    pub(crate) fn acknowledged() -> Obligation {
        let closed = cancelled(&completed());
        assert_eq!(closed.state(), ObligationState::CancelledByUser);
        closed
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{artifact_id, completed, created, defer_source, running};
    use super::*;
    use crate::fence::test_support::source;
    use crate::input::test_support as input_support;
    use crate::worker_command::test_support as command_support;
    use crate::worker_evidence::test_support as evidence_support;
    use uuid::Uuid;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn claim_id(n: u128) -> ClaimId {
        ClaimId::from_uuid(Uuid::from_u128(n))
    }

    fn claimed(obligation: &Obligation) -> Obligation {
        obligation
            .apply(&ObligationEvent::ForemanClaimed {
                claim: claim_id(100),
                binding_generation: BindingGeneration::FIRST,
                expected_version: obligation.version(),
                expected_source: obligation.source().clone(),
                at: at(10),
            })
            .expect("claiming an attention state is legal")
            .advanced()
            .expect("claim advances")
    }

    fn processing(obligation: &Obligation) -> Obligation {
        claimed(obligation)
            .apply(&ObligationEvent::HandoffDelivered {
                claim: claim_id(100),
                at: at(11),
            })
            .expect("handoff is legal from claimed")
            .advanced()
            .expect("handoff advances")
    }

    fn ack_for(obligation: &Obligation, disposition: Disposition) -> AckRequest {
        AckRequest {
            obligation: obligation.id(),
            expected_version: obligation.version(),
            expected_source: obligation.source().clone(),
            binding_generation: BindingGeneration::FIRST,
            claim: claim_id(100),
            disposition,
            at: at(12),
        }
    }

    #[test]
    fn the_happy_path_runs_created_to_acknowledged() {
        let obligation = created();
        assert_eq!(obligation.state(), ObligationState::Created);

        let running = running();
        assert_eq!(running.state(), ObligationState::Running);

        let completed = completed();
        assert_eq!(completed.state(), ObligationState::CompletedUnprocessed);
        assert!(completed.is_open(), "worker completion does not close work");
        assert_eq!(completed.result_artifact(), Some(artifact_id()));

        let claimed = claimed(&completed);
        assert_eq!(claimed.state(), ObligationState::ClaimedByForeman);
        assert!(claimed.is_open());

        let processing = processing(&completed);
        assert_eq!(processing.state(), ObligationState::Processing);
        assert!(processing.is_open());

        let acknowledged = processing
            .apply(&ObligationEvent::ForemanAcked(Box::new(ack_for(
                &processing,
                Disposition::Accepted,
            ))))
            .expect("a fully fenced ACK closes the obligation")
            .advanced()
            .expect("ACK advances");
        assert_eq!(acknowledged.state(), ObligationState::Acknowledged);
        assert!(!acknowledged.is_open());
    }

    #[test]
    fn every_transition_advances_the_version() {
        let created = created();
        let running = running();
        let completed = completed();
        let processing = processing(&completed);
        assert!(running.version() > created.version());
        assert!(completed.version() > running.version());
        assert!(processing.version() > completed.version());
    }

    #[test]
    fn needs_input_requires_a_confirmed_defer_boundary() {
        // The type system does the work: `InputBoundaryConfirmed` cannot be
        // constructed without a `ConfirmedDefer`, and `ConfirmedDefer` cannot
        // be constructed from a multi-tool or unconfirmed defer.
        let needs_input = running()
            .apply(&ObligationEvent::InputBoundaryConfirmed {
                source: defer_source(),
                incarnation: IncarnationGeneration::FIRST,
                input_request: input_support::pending_request().id(),
                defer: input_support::confirmed_defer(),
                at: at(5),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(needs_input.state(), ObligationState::NeedsInput);
        assert!(needs_input.is_open());
        assert!(needs_input.input_request().is_some());
    }

    #[test]
    fn only_confirmed_resumed_turn_evidence_restores_running() {
        let needs_input = running()
            .apply(&ObligationEvent::InputBoundaryConfirmed {
                source: defer_source(),
                incarnation: IncarnationGeneration::FIRST,
                input_request: input_support::pending_request().id(),
                defer: input_support::confirmed_defer(),
                at: at(5),
            })
            .unwrap()
            .advanced()
            .unwrap();

        let resumed = needs_input
            .apply(&ObligationEvent::WorkerResumed {
                source: source("claude.init", "run-2", "resumed"),
                incarnation: IncarnationGeneration::FIRST,
                proof: command_support::confirmed_resumed_turn(),
                at: at(6),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(resumed.state(), ObligationState::Running);
        assert!(resumed.input_request().is_none());
    }

    #[test]
    fn worker_failure_is_unprocessed_work_not_a_closure() {
        let failed = running()
            .apply(&ObligationEvent::WorkerFailed {
                source: source("claude.result", "run-1", "error"),
                incarnation: IncarnationGeneration::FIRST,
                failure: crate::worker_evidence::WorkerFailureClass::StructuredError,
                at: at(4),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(failed.state(), ObligationState::Failed);
        assert!(failed.is_open());
        assert_eq!(failed.attention(), Some(AttentionState::Failed));
    }

    #[test]
    fn a_duplicate_terminal_source_event_is_idempotent() {
        let completed = completed();
        let duplicate = ObligationEvent::ResultPublished {
            source: source("claude.result", "run-1", "final"),
            incarnation: IncarnationGeneration::FIRST,
            proof: evidence_support::confirmed_completion(),
            artifact: artifact_id(),
            at: at(2),
        };
        for _ in 0..100 {
            assert!(completed.apply(&duplicate).unwrap().is_duplicate());
        }
        assert_eq!(completed.version(), ObligationVersion::new(3));
    }

    #[test]
    fn a_stale_incarnation_event_cannot_mutate_current_work() {
        let reattached = running()
            .apply(&ObligationEvent::WorkerStarted {
                source: source("claude.init", "run-2", "start"),
                incarnation: IncarnationGeneration::new(2),
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(reattached.incarnation(), IncarnationGeneration::new(2));

        let err = reattached
            .apply(&ObligationEvent::ResultPublished {
                source: source("claude.result", "run-1", "final"),
                incarnation: IncarnationGeneration::FIRST,
                proof: evidence_support::confirmed_completion(),
                artifact: artifact_id(),
                at: at(4),
            })
            .unwrap_err();
        assert_eq!(err.code(), "stale_session_incarnation");
        assert_eq!(reattached.state(), ObligationState::Running);
    }

    #[test]
    fn a_stale_version_cannot_claim() {
        let completed = completed();
        let err = completed
            .apply(&ObligationEvent::ForemanClaimed {
                claim: claim_id(100),
                binding_generation: BindingGeneration::FIRST,
                expected_version: ObligationVersion::FIRST,
                expected_source: completed.source().clone(),
                at: at(10),
            })
            .unwrap_err();
        assert_eq!(err.code(), "stale_obligation_version");
        assert_eq!(completed.state(), ObligationState::CompletedUnprocessed);
    }

    #[test]
    fn a_stale_source_fence_cannot_claim() {
        let completed = completed();
        let err = completed
            .apply(&ObligationEvent::ForemanClaimed {
                claim: claim_id(100),
                binding_generation: BindingGeneration::FIRST,
                expected_version: completed.version(),
                expected_source: source("claude.result", "run-0", "final"),
                at: at(10),
            })
            .unwrap_err();
        assert_eq!(err.code(), "stale_source_fence");
    }

    #[test]
    fn ack_rejects_every_stale_fence_one_field_at_a_time() {
        let processing = processing(&completed());
        let good = ack_for(&processing, Disposition::Accepted);

        let stale_version = AckRequest {
            expected_version: ObligationVersion::FIRST,
            ..good.clone()
        };
        let stale_source = AckRequest {
            expected_source: source("claude.result", "run-0", "final"),
            ..good.clone()
        };
        let stale_generation = AckRequest {
            binding_generation: BindingGeneration::new(2),
            ..good.clone()
        };
        let stale_claim = AckRequest {
            claim: claim_id(999),
            ..good.clone()
        };
        let bad_disposition = AckRequest {
            disposition: Disposition::FailureAcknowledged,
            ..good.clone()
        };

        for (request, code) in [
            (stale_version, "stale_obligation_version"),
            (stale_source, "stale_source_fence"),
            (stale_generation, "stale_binding_generation"),
            (stale_claim, "stale_claim"),
            (bad_disposition, "invalid_disposition"),
        ] {
            let err = processing
                .apply(&ObligationEvent::ForemanAcked(Box::new(request)))
                .unwrap_err();
            assert_eq!(err.code(), code);
            assert_eq!(
                processing.state(),
                ObligationState::Processing,
                "a rejected ACK mutates nothing"
            );
        }
    }

    #[test]
    fn an_exact_repeat_of_a_committed_ack_is_idempotent() {
        let processing = processing(&completed());
        let request = ack_for(&processing, Disposition::Accepted);
        let acknowledged = processing
            .apply(&ObligationEvent::ForemanAcked(Box::new(request.clone())))
            .unwrap()
            .advanced()
            .unwrap();

        let repeat = acknowledged
            .apply(&ObligationEvent::ForemanAcked(Box::new(request)))
            .unwrap();
        assert!(repeat.is_duplicate());

        // A *different* ACK against a closed obligation is refused.
        let err = acknowledged
            .apply(&ObligationEvent::ForemanAcked(Box::new(ack_for(
                &acknowledged,
                Disposition::Abandoned,
            ))))
            .unwrap_err();
        assert_eq!(err.code(), "obligation_closed");
    }

    #[test]
    fn claim_expiry_restores_attention_and_never_closes() {
        for start in [completed(), failed_obligation()] {
            let expected = start.attention().expect("an attention state");
            let processing = processing(&start);
            let expired = processing
                .apply(&ObligationEvent::ClaimExpired {
                    claim: claim_id(100),
                    at: at(20),
                })
                .unwrap()
                .advanced()
                .unwrap();
            assert_eq!(expired.state(), ObligationState::from(expected));
            assert!(expired.is_open());
            assert!(expired.claim().is_none());
            assert_eq!(
                expired.result_artifact(),
                start.result_artifact(),
                "expiry never releases a required artifact"
            );
        }
    }

    #[test]
    fn an_expired_claim_cannot_later_ack() {
        let processing = processing(&completed());
        let request = ack_for(&processing, Disposition::Accepted);
        let expired = processing
            .apply(&ObligationEvent::ClaimExpired {
                claim: claim_id(100),
                at: at(20),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let err = expired
            .apply(&ObligationEvent::ForemanAcked(Box::new(request)))
            .unwrap_err();
        assert_eq!(err.code(), "illegal_obligation_transition");
        assert_eq!(expired.state(), ObligationState::CompletedUnprocessed);
    }

    #[test]
    fn a_second_claim_cannot_displace_a_live_one() {
        let claimed = claimed(&completed());
        let err = claimed
            .apply(&ObligationEvent::ForemanClaimed {
                claim: claim_id(200),
                binding_generation: BindingGeneration::FIRST,
                expected_version: claimed.version(),
                expected_source: claimed.source().clone(),
                at: at(15),
            })
            .unwrap_err();
        assert_eq!(err.code(), "obligation_already_claimed");
    }

    #[test]
    fn disposition_must_match_the_attention_it_closes() {
        assert!(Disposition::Accepted.closes(AttentionState::CompletedUnprocessed));
        assert!(!Disposition::Accepted.closes(AttentionState::Failed));
        assert!(Disposition::FailureAcknowledged.closes(AttentionState::Failed));
        assert!(!Disposition::FailureAcknowledged.closes(AttentionState::CompletedUnprocessed));
        assert!(Disposition::Abandoned.closes(AttentionState::NeedsInput));
        assert!(!Disposition::Accepted.closes(AttentionState::NeedsInput));
    }

    #[test]
    fn no_event_other_than_a_disposition_closes_an_obligation() {
        let completed = completed();
        // Everything the browser and the assistant can do leaves it open.
        let claimed = claimed(&completed);
        let processing = processing(&completed);
        for obligation in [&completed, &claimed, &processing] {
            assert!(obligation.is_open());
        }
    }

    fn failed_obligation() -> Obligation {
        running()
            .apply(&ObligationEvent::WorkerFailed {
                source: source("claude.result", "run-1", "error"),
                incarnation: IncarnationGeneration::FIRST,
                failure: crate::worker_evidence::WorkerFailureClass::StructuredError,
                at: at(4),
            })
            .unwrap()
            .advanced()
            .unwrap()
    }
}
