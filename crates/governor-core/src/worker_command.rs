//! Worker continuations: answers and resumes owed to a worker.
//!
//! Recording a foreman answer proves nothing about the worker. Per
//! [`docs/state-machines.md`] §8 a continuation is an external effect with the
//! same `pending -> claimed -> accepted | failed | ambiguous` discipline as a
//! browser wake, and *transport acceptance still does not restore `running`*.
//!
//! Only [`ConfirmedResumedTurn`] does, and the only way to obtain one is
//! [`WorkerContinuation::confirm_resumed_turn`] with matching resumed-turn
//! evidence for the exact command revision and a live incarnation.
//!
//! [`docs/state-machines.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/state-machines.md

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{CommandRevision, IncarnationGeneration, SafeToken};
use crate::id::{InputRequestId, SessionIncarnationId, WorkerCommandId};
use crate::outbound::{Delivery, DeliveryEvent, DeliveryState};
use crate::time::Timestamp;

/// What a continuation carries to the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WorkerCommandKind {
    /// A structured answer to a deferred input request.
    AnswerInput,
    /// A plain resume of a paused run.
    Resume,
}

/// Transport-level proof that the continuation was handed to the worker.
///
/// This is *acceptance of the delivery*, not evidence that the worker acted on
/// it — hence the separate [`ResumedTurnEvidence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedContinuation {
    run_ref: SafeToken,
}

impl AcceptedContinuation {
    /// Records the opaque managed run the continuation was delivered into.
    #[must_use]
    pub const fn new(run_ref: SafeToken) -> Self {
        Self { run_ref }
    }

    /// Opaque identity of that run.
    #[must_use]
    pub const fn run_ref(&self) -> &SafeToken {
        &self.run_ref
    }
}

/// Native evidence that the worker actually resumed with the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumedTurnEvidence {
    /// Command revision the resumed turn corresponds to.
    pub command_revision: CommandRevision,
    /// Session incarnation the resumed turn was observed in.
    pub incarnation: IncarnationGeneration,
    /// Opaque identity of the resumed managed run.
    pub run_ref: SafeToken,
    /// Input request the resumed turn answered, if it answered one.
    pub answered_input: Option<InputRequestId>,
}

/// Proof that a worker resumed with a specific continuation.
///
/// No public constructor. The obligation's `needs_input -> running` transition
/// requires one, so transport acceptance alone can never produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedResumedTurn {
    command_revision: CommandRevision,
    run_ref: SafeToken,
}

impl ConfirmedResumedTurn {
    /// Command revision that was resumed.
    #[must_use]
    pub const fn command_revision(&self) -> CommandRevision {
        self.command_revision
    }

    /// Opaque identity of the resumed run.
    #[must_use]
    pub const fn run_ref(&self) -> &SafeToken {
        &self.run_ref
    }
}

/// One durable continuation revision owed to a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerContinuation {
    id: WorkerCommandId,
    input_request: Option<InputRequestId>,
    session_incarnation: SessionIncarnationId,
    incarnation: IncarnationGeneration,
    kind: WorkerCommandKind,
    revision: CommandRevision,
    delivery: Delivery<AcceptedContinuation>,
}

impl WorkerContinuation {
    /// Creates a pending continuation for the current incarnation.
    #[must_use]
    pub fn create(
        id: WorkerCommandId,
        input_request: Option<InputRequestId>,
        session_incarnation: SessionIncarnationId,
        incarnation: IncarnationGeneration,
        kind: WorkerCommandKind,
        attempt_budget: u32,
    ) -> Self {
        Self {
            id,
            input_request,
            session_incarnation,
            incarnation,
            kind,
            revision: CommandRevision::FIRST,
            delivery: Delivery::pending(attempt_budget),
        }
    }

    /// Creates the next revision of this continuation.
    ///
    /// As with a browser wake, a later attempt at the same logical work is a
    /// new revision; the old one is never replayed.
    #[must_use]
    pub fn next_revision(&self, id: WorkerCommandId, attempt_budget: u32) -> Self {
        Self {
            id,
            input_request: self.input_request,
            session_incarnation: self.session_incarnation,
            incarnation: self.incarnation,
            kind: self.kind,
            revision: self.revision.next(),
            delivery: Delivery::pending(attempt_budget),
        }
    }

    /// Continuation identity.
    #[must_use]
    pub const fn id(&self) -> WorkerCommandId {
        self.id
    }

    /// Input request this continuation answers, if any.
    #[must_use]
    pub const fn input_request(&self) -> Option<InputRequestId> {
        self.input_request
    }

    /// Session incarnation this continuation targets.
    #[must_use]
    pub const fn session_incarnation(&self) -> SessionIncarnationId {
        self.session_incarnation
    }

    /// Incarnation generation this continuation targets.
    #[must_use]
    pub const fn incarnation(&self) -> IncarnationGeneration {
        self.incarnation
    }

    /// What the continuation carries.
    #[must_use]
    pub const fn kind(&self) -> WorkerCommandKind {
        self.kind
    }

    /// Revision number.
    #[must_use]
    pub const fn revision(&self) -> CommandRevision {
        self.revision
    }

    /// Aggregate delivery projection.
    #[must_use]
    pub const fn state(&self) -> DeliveryState {
        self.delivery.state()
    }

    /// The underlying attempt machine.
    #[must_use]
    pub const fn delivery(&self) -> &Delivery<AcceptedContinuation> {
        &self.delivery
    }

    /// Applies a delivery event.
    ///
    /// # Errors
    ///
    /// Returns the [`Conflict`] from the underlying attempt machine.
    pub fn apply(&self, event: &DeliveryEvent<AcceptedContinuation>) -> Outcome<Self> {
        match self.delivery.apply(event)? {
            Transition::Duplicate => Ok(Transition::Duplicate),
            Transition::Advanced(delivery) => {
                let mut next = self.clone();
                next.delivery = delivery;
                Ok(Transition::Advanced(next))
            }
        }
    }

    /// Confirms a resumed turn, producing the proof the obligation requires.
    ///
    /// Accepts evidence against an `accepted` revision, and promotes an
    /// `ambiguous` one — matching resumed-turn evidence is exactly the exact
    /// reconciliation that ambiguity waits for. A `pending`, `claimed` or
    /// `failed` revision has nothing to confirm.
    ///
    /// # Errors
    ///
    /// - [`Conflict::StaleCommandRevision`] when the evidence is for another
    ///   revision;
    /// - [`Conflict::StaleSessionIncarnation`] when it came from a replaced
    ///   incarnation;
    /// - [`Conflict::IllegalDeliveryTransition`] when the revision was never
    ///   delivered.
    pub fn confirm_resumed_turn(
        &self,
        evidence: &ResumedTurnEvidence,
        at: Timestamp,
    ) -> Result<(Self, ConfirmedResumedTurn), Conflict> {
        if evidence.command_revision != self.revision {
            return Err(Conflict::StaleCommandRevision {
                presented: evidence.command_revision,
                current: self.revision,
            });
        }
        if evidence.incarnation.get() < self.incarnation.get() {
            return Err(Conflict::StaleSessionIncarnation {
                presented: evidence.incarnation,
                current: self.incarnation,
            });
        }
        if evidence.answered_input != self.input_request {
            return Err(Conflict::StaleCommandRevision {
                presented: evidence.command_revision,
                current: self.revision,
            });
        }

        let confirmed = ConfirmedResumedTurn {
            command_revision: self.revision,
            run_ref: evidence.run_ref.clone(),
        };

        match self.delivery.state() {
            DeliveryState::Accepted => Ok((self.clone(), confirmed)),
            DeliveryState::Ambiguous => {
                let promoted = self.apply(&DeliveryEvent::ReconciledAccepted {
                    evidence: AcceptedContinuation::new(evidence.run_ref.clone()),
                    at,
                })?;
                Ok((promoted.or_unchanged(self.clone()), confirmed))
            }
            state => Err(Conflict::IllegalDeliveryTransition {
                from: self
                    .delivery
                    .attempts()
                    .last()
                    .map_or(crate::outbound::AttemptState::Claimed, |attempt| {
                        attempt.state()
                    }),
                event: match state {
                    DeliveryState::Pending => "resumed_turn_without_delivery",
                    DeliveryState::Claimed => "resumed_turn_before_acceptance",
                    _ => "resumed_turn_after_failure",
                },
            }),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{
        AcceptedContinuation, ConfirmedResumedTurn, ResumedTurnEvidence, WorkerCommandKind,
        WorkerContinuation,
    };
    use crate::fence::{AttemptNo, CommandRevision, IncarnationGeneration, SafeToken};
    use crate::id::{SessionIncarnationId, WorkerCommandId};
    use crate::outbound::DeliveryEvent;
    use crate::time::Timestamp;
    use uuid::Uuid;

    pub(crate) fn run_ref() -> SafeToken {
        SafeToken::new("run-2").expect("safe")
    }

    pub(crate) fn pending() -> WorkerContinuation {
        WorkerContinuation::create(
            WorkerCommandId::from_uuid(Uuid::from_u128(51)),
            None,
            SessionIncarnationId::from_uuid(Uuid::from_u128(61)),
            IncarnationGeneration::FIRST,
            WorkerCommandKind::AnswerInput,
            3,
        )
    }

    pub(crate) fn accepted() -> WorkerContinuation {
        pending()
            .apply(&DeliveryEvent::AttemptClaimed {
                at: Timestamp::from_unix_millis(1),
            })
            .unwrap()
            .advanced()
            .unwrap()
            .apply(&DeliveryEvent::ActivationArmed {
                attempt: AttemptNo::FIRST,
                at: Timestamp::from_unix_millis(2),
            })
            .unwrap()
            .advanced()
            .unwrap()
            .apply(&DeliveryEvent::AttemptAccepted {
                attempt: AttemptNo::FIRST,
                evidence: AcceptedContinuation::new(run_ref()),
                at: Timestamp::from_unix_millis(3),
            })
            .unwrap()
            .advanced()
            .unwrap()
    }

    pub(crate) fn evidence() -> ResumedTurnEvidence {
        ResumedTurnEvidence {
            command_revision: CommandRevision::FIRST,
            incarnation: IncarnationGeneration::FIRST,
            run_ref: run_ref(),
            answered_input: None,
        }
    }

    pub(crate) fn confirmed_resumed_turn() -> ConfirmedResumedTurn {
        accepted()
            .confirm_resumed_turn(&evidence(), Timestamp::from_unix_millis(4))
            .expect("accepted delivery plus matching evidence confirms")
            .1
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{accepted, evidence, pending, run_ref};
    use super::*;
    use crate::fence::AttemptNo;
    use crate::outbound::AmbiguityReason;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    #[test]
    fn a_continuation_obeys_the_same_external_effect_discipline() {
        let continuation = pending();
        assert_eq!(continuation.state(), DeliveryState::Pending);
        assert!(continuation.delivery().io_permit().is_none());

        let claimed = continuation
            .apply(&DeliveryEvent::AttemptClaimed { at: at(1) })
            .unwrap()
            .advanced()
            .unwrap();
        assert!(claimed.delivery().io_permit().is_some());
        assert!(claimed.delivery().send_activation().is_none());
    }

    #[test]
    fn transport_acceptance_alone_does_not_confirm_a_resumed_turn() {
        // Acceptance is required, but the proof still needs matching evidence.
        let delivered = accepted();
        assert_eq!(delivered.state(), DeliveryState::Accepted);

        let mismatched = ResumedTurnEvidence {
            command_revision: CommandRevision::new(9),
            ..evidence()
        };
        let err = delivered
            .confirm_resumed_turn(&mismatched, at(4))
            .unwrap_err();
        assert_eq!(err.code(), "stale_command_revision");
    }

    #[test]
    fn an_undelivered_continuation_cannot_confirm_a_resumed_turn() {
        let err = pending()
            .confirm_resumed_turn(&evidence(), at(4))
            .unwrap_err();
        assert_eq!(err.code(), "illegal_delivery_transition");
    }

    #[test]
    fn matching_evidence_promotes_an_ambiguous_continuation() {
        let ambiguous = pending()
            .apply(&DeliveryEvent::AttemptClaimed { at: at(1) })
            .unwrap()
            .advanced()
            .unwrap()
            .apply(&DeliveryEvent::ActivationArmed {
                attempt: AttemptNo::FIRST,
                at: at(2),
            })
            .unwrap()
            .advanced()
            .unwrap()
            .apply(&DeliveryEvent::AttemptAmbiguous {
                attempt: AttemptNo::FIRST,
                reason: AmbiguityReason::ObservationLost,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(ambiguous.state(), DeliveryState::Ambiguous);

        let (reconciled, proof) = ambiguous
            .confirm_resumed_turn(&evidence(), at(4))
            .expect("exact evidence reconciles an ambiguous continuation");
        assert_eq!(reconciled.state(), DeliveryState::Accepted);
        assert_eq!(proof.run_ref(), &run_ref());
        assert_eq!(
            ambiguous.state(),
            DeliveryState::Ambiguous,
            "the prior value is untouched"
        );
    }

    #[test]
    fn evidence_from_a_replaced_incarnation_is_rejected() {
        let delivered = WorkerContinuation {
            incarnation: IncarnationGeneration::new(3),
            ..accepted()
        };
        let stale = ResumedTurnEvidence {
            incarnation: IncarnationGeneration::FIRST,
            ..evidence()
        };
        let err = delivered.confirm_resumed_turn(&stale, at(4)).unwrap_err();
        assert_eq!(err.code(), "stale_session_incarnation");
    }

    #[test]
    fn a_new_revision_starts_a_fresh_delivery() {
        let first = accepted();
        let second = first.next_revision(
            crate::id::WorkerCommandId::from_uuid(uuid::Uuid::from_u128(52)),
            3,
        );
        assert_eq!(second.revision(), CommandRevision::new(2));
        assert_eq!(second.state(), DeliveryState::Pending);
        assert_eq!(
            first.state(),
            DeliveryState::Accepted,
            "the old revision is frozen, not reused"
        );
    }
}
