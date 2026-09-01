//! The shared external-effect discipline.
//!
//! Browser wakes and worker continuations are different deliveries with
//! different evidence, but they obey one rule set
//! ([`docs/state-machines.md`] §4 and §8):
//!
//! ```text
//! pending -> claimed -> accepted | failed | ambiguous
//! ```
//!
//! - `claimed` is durable **before any** external I/O;
//! - `activation_armed` is durable **before** the exact submit action;
//! - `failed` is only reachable with proof that no submission happened;
//! - anything else after arming is `ambiguous`;
//! - `accepted` and `ambiguous` freeze the revision forever — a later resume is
//!   a *new* revision, never a replay of this one.
//!
//! [`docs/state-machines.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/state-machines.md

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::AttemptNo;
use crate::time::Timestamp;

/// Aggregate projection of one delivery revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeliveryState {
    /// Durably intended, no attempt claimed yet. No I/O has happened.
    Pending,
    /// An attempt owns the external effect. I/O may proceed.
    Claimed,
    /// Exact semantic evidence proves the intended effect landed. Frozen.
    Accepted,
    /// Proven not to have happened. A bounded retry may be safe.
    Failed,
    /// It may or may not have happened. Frozen; never replayed.
    Ambiguous,
}

impl DeliveryState {
    /// Reports whether this revision may never produce another external effect.
    #[must_use]
    pub const fn is_frozen(self) -> bool {
        matches!(self, Self::Accepted | Self::Ambiguous)
    }

    /// Reports whether the revision has reached a terminal projection.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Failed | Self::Ambiguous)
    }
}

/// Lifecycle of a single attempt within a revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttemptState {
    /// Committed before any external I/O for this attempt.
    Claimed,
    /// Committed immediately before the exact submit activation.
    ActivationArmed,
    /// The submit is proven to have landed.
    Accepted,
    /// The submit is proven not to have happened.
    Failed,
    /// The outcome is unknown and must be assumed to have happened.
    Ambiguous,
}

impl AttemptState {
    /// Reports whether this attempt can still change state.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Claimed | Self::ActivationArmed)
    }
}

/// Why an attempt is proven not to have submitted anything.
///
/// Every variant is a *pre-submit* class. Only the subset reported by
/// [`FailureClass::proves_no_submit_after_arming`] may be claimed once the Send
/// ambiguity fence is armed; anything else at that point is
/// [`Conflict::FailureNotProven`] and the caller must record `ambiguous`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FailureClass {
    /// The delivery target could not be resolved at all.
    TargetNotFound,
    /// The wake's target obligation snapshot went stale before submission.
    StaleTarget,
    /// The resolved surface is not the exact bound conversation.
    WrongConversation,
    /// The Command Governor app was not selected for this exact message.
    AppNotSelected,
    /// The composer was not in a state that could be staged.
    ComposerNotReady,
    /// Navigation to the bound surface was blocked or redirected.
    NavigationBlocked,
    /// The activation call itself was refused before any submission.
    ActivationRefused,
    /// The transport rejected the call synchronously, before submission.
    TransportRejectedBeforeSend,
}

impl FailureClass {
    /// Reports whether this class still proves "no submission" after arming.
    ///
    /// Once the Send ambiguity fence is armed, a report that the target was
    /// missing or the composer was not ready no longer proves anything about
    /// whether a submission raced ahead of it. Only a synchronous refusal of
    /// the activation itself does.
    #[must_use]
    pub const fn proves_no_submit_after_arming(self) -> bool {
        matches!(
            self,
            Self::ActivationRefused | Self::TransportRejectedBeforeSend
        )
    }
}

/// Why an attempt's outcome could not be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum AmbiguityReason {
    /// The process died between arming the fence and observing an outcome.
    OrphanedByRestart,
    /// The observation channel was lost while the submit was in flight.
    ObservationLost,
    /// Evidence arrived but was too weak to identify the exact message.
    EvidenceInconclusive,
    /// The activation call neither confirmed nor refused within its bound.
    ActivationTimedOut,
}

/// One attempt at producing the external effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    number: AttemptNo,
    state: AttemptState,
    armed: bool,
    claimed_at: Timestamp,
    armed_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    failure: Option<FailureClass>,
    ambiguity: Option<AmbiguityReason>,
}

impl Attempt {
    /// Attempt number within the revision, starting at one.
    #[must_use]
    pub const fn number(&self) -> AttemptNo {
        self.number
    }

    /// Current attempt state.
    #[must_use]
    pub const fn state(&self) -> AttemptState {
        self.state
    }

    /// Reports whether this attempt ever crossed the Send ambiguity fence.
    #[must_use]
    pub const fn armed(&self) -> bool {
        self.armed
    }

    /// Instant the attempt was claimed.
    #[must_use]
    pub const fn claimed_at(&self) -> Timestamp {
        self.claimed_at
    }

    /// Instant the Send ambiguity fence was armed, if it was.
    #[must_use]
    pub const fn armed_at(&self) -> Option<Timestamp> {
        self.armed_at
    }

    /// Instant the attempt reached a terminal state, if it has.
    #[must_use]
    pub const fn finished_at(&self) -> Option<Timestamp> {
        self.finished_at
    }

    /// Proven failure class, if the attempt failed.
    #[must_use]
    pub const fn failure(&self) -> Option<FailureClass> {
        self.failure
    }

    /// Ambiguity reason, if the attempt is ambiguous.
    #[must_use]
    pub const fn ambiguity(&self) -> Option<AmbiguityReason> {
        self.ambiguity
    }
}

/// Capability proving an attempt is durably `claimed`.
///
/// An adapter cannot perform any external I/O for a delivery without one, and
/// the only way to obtain one is [`Delivery::io_permit`], which requires a live
/// attempt. This is [`docs/state-machines.md`] invariant 10 expressed in the
/// type system rather than in a comment.
///
/// [`docs/state-machines.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/state-machines.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoPermit {
    attempt: AttemptNo,
}

impl IoPermit {
    /// Attempt this permit belongs to.
    #[must_use]
    pub const fn attempt(&self) -> AttemptNo {
        self.attempt
    }
}

/// Capability proving the Send ambiguity fence is durably armed.
///
/// The exact submit action must require one. This is invariant 11 in the type
/// system: there is no constructor other than [`Delivery::send_activation`],
/// which yields `Some` only for an `activation_armed` attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendActivation {
    attempt: AttemptNo,
}

impl SendActivation {
    /// Attempt this activation belongs to.
    #[must_use]
    pub const fn attempt(&self) -> AttemptNo {
        self.attempt
    }
}

/// An event applied to a delivery revision.
///
/// `E` is the acceptance evidence type of the concrete transport.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliveryEvent<E> {
    /// Commit an attempt before any external I/O.
    AttemptClaimed {
        /// Observation instant.
        at: Timestamp,
    },
    /// Commit the Send ambiguity fence immediately before the submit action.
    ActivationArmed {
        /// Attempt being armed.
        attempt: AttemptNo,
        /// Observation instant.
        at: Timestamp,
    },
    /// Exact semantic evidence proves the submission landed.
    AttemptAccepted {
        /// Attempt that submitted.
        attempt: AttemptNo,
        /// Exact evidence binding the effect to its intended target.
        evidence: E,
        /// Observation instant.
        at: Timestamp,
    },
    /// The attempt is proven not to have submitted anything.
    AttemptFailed {
        /// Attempt that failed.
        attempt: AttemptNo,
        /// Proven pre-submit failure class.
        failure: FailureClass,
        /// Observation instant.
        at: Timestamp,
    },
    /// The attempt's outcome cannot be determined.
    AttemptAmbiguous {
        /// Attempt whose outcome is unknown.
        attempt: AttemptNo,
        /// Why the outcome is unknown.
        reason: AmbiguityReason,
        /// Observation instant.
        at: Timestamp,
    },
    /// Startup quarantine of an attempt orphaned by a previous process.
    ///
    /// Applied *before* transport recovery, so a live browser or runtime can
    /// never resume an attempt whose outcome was lost.
    OrphanQuarantined {
        /// Observation instant.
        at: Timestamp,
    },
    /// Exact later evidence promotes an ambiguous revision to accepted.
    ///
    /// This is the only automatic escape from `ambiguous`, and it produces no
    /// external effect.
    ReconciledAccepted {
        /// Exact evidence binding the effect to its intended target.
        evidence: E,
        /// Observation instant.
        at: Timestamp,
    },
}

impl<E> DeliveryEvent<E> {
    const fn label(&self) -> &'static str {
        match self {
            Self::AttemptClaimed { .. } => "attempt_claimed",
            Self::ActivationArmed { .. } => "activation_armed",
            Self::AttemptAccepted { .. } => "attempt_accepted",
            Self::AttemptFailed { .. } => "attempt_failed",
            Self::AttemptAmbiguous { .. } => "attempt_ambiguous",
            Self::OrphanQuarantined { .. } => "orphan_quarantined",
            Self::ReconciledAccepted { .. } => "reconciled_accepted",
        }
    }
}

/// One delivery revision and its bounded attempt history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery<E> {
    state: DeliveryState,
    attempts: Vec<Attempt>,
    attempt_budget: u32,
    accepted: Option<E>,
}

impl<E: Clone + PartialEq> Delivery<E> {
    /// Creates a pending revision with a bounded attempt budget.
    ///
    /// # Panics
    ///
    /// Panics when `attempt_budget` is zero: a delivery that may never attempt
    /// anything is a configuration error, not a runtime state.
    #[must_use]
    pub fn pending(attempt_budget: u32) -> Self {
        assert!(attempt_budget > 0, "attempt budget must be at least one");
        Self {
            state: DeliveryState::Pending,
            attempts: Vec::new(),
            attempt_budget,
            accepted: None,
        }
    }

    /// Current aggregate projection.
    #[must_use]
    pub const fn state(&self) -> DeliveryState {
        self.state
    }

    /// Attempt history, oldest first.
    #[must_use]
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// The exact acceptance evidence, once accepted.
    #[must_use]
    pub fn accepted_evidence(&self) -> Option<&E> {
        self.accepted.as_ref()
    }

    /// Configured bounded attempt budget.
    #[must_use]
    pub const fn attempt_budget(&self) -> u32 {
        self.attempt_budget
    }

    /// The live attempt, if one owns the external effect right now.
    #[must_use]
    pub fn live_attempt(&self) -> Option<&Attempt> {
        self.attempts.last().filter(|a| a.state.is_live())
    }

    /// Permit to perform external I/O, available only while an attempt is live.
    #[must_use]
    pub fn io_permit(&self) -> Option<IoPermit> {
        self.live_attempt().map(|a| IoPermit { attempt: a.number })
    }

    /// Capability to invoke the exact submit, only once the fence is armed.
    #[must_use]
    pub fn send_activation(&self) -> Option<SendActivation> {
        self.attempts
            .last()
            .filter(|a| a.state == AttemptState::ActivationArmed)
            .map(|a| SendActivation { attempt: a.number })
    }

    /// Reports whether a bounded retry may create another attempt.
    #[must_use]
    pub fn may_retry(&self) -> bool {
        self.retry_conflict().is_none()
    }

    fn retry_conflict(&self) -> Option<Conflict> {
        if self.state.is_frozen() {
            return Some(Conflict::DeliveryRevisionFrozen { state: self.state });
        }
        match self.attempts.last() {
            None => None,
            Some(last) if last.armed => Some(Conflict::RetryAfterAmbiguityFence {
                attempt: last.number,
            }),
            Some(last) if last.state != AttemptState::Failed => {
                Some(Conflict::IllegalDeliveryTransition {
                    from: last.state,
                    event: "attempt_claimed",
                })
            }
            Some(_) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the attempt budget bounds the vector to u32 range"
                )]
                let used = self.attempts.len() as u32;
                if used >= self.attempt_budget {
                    Some(Conflict::RetryBudgetExhausted {
                        budget: self.attempt_budget,
                    })
                } else {
                    None
                }
            }
        }
    }

    fn attempt_index(&self, number: AttemptNo) -> Result<usize, Conflict> {
        self.attempts
            .iter()
            .position(|a| a.number == number)
            .ok_or(Conflict::UnknownAttempt { attempt: number })
    }

    /// Applies an event, returning a new revision or a typed conflict.
    ///
    /// The receiver is borrowed and never mutated: a conflict therefore cannot
    /// have left a partially applied state behind.
    ///
    /// # Errors
    ///
    /// Returns the [`Conflict`] describing why the event is not legal here.
    pub fn apply(&self, event: &DeliveryEvent<E>) -> Outcome<Self> {
        match event {
            DeliveryEvent::AttemptClaimed { at } => self.claim_attempt(*at),
            DeliveryEvent::ActivationArmed { attempt, at } => self.arm(*attempt, *at),
            DeliveryEvent::AttemptAccepted {
                attempt,
                evidence,
                at,
            } => self.accept(*attempt, evidence, *at),
            DeliveryEvent::AttemptFailed {
                attempt,
                failure,
                at,
            } => self.fail(*attempt, *failure, *at),
            DeliveryEvent::AttemptAmbiguous {
                attempt,
                reason,
                at,
            } => self.mark_ambiguous(*attempt, *reason, *at),
            DeliveryEvent::OrphanQuarantined { at } => self.quarantine(*at),
            DeliveryEvent::ReconciledAccepted { evidence, at } => {
                self.reconcile(evidence, *at, event.label())
            }
        }
    }

    fn claim_attempt(&self, at: Timestamp) -> Outcome<Self> {
        if let Some(conflict) = self.retry_conflict() {
            return Err(conflict);
        }
        let number = self
            .attempts
            .last()
            .map_or(AttemptNo::FIRST, |a| a.number.next());
        let mut next = self.clone();
        next.attempts.push(Attempt {
            number,
            state: AttemptState::Claimed,
            armed: false,
            claimed_at: at,
            armed_at: None,
            finished_at: None,
            failure: None,
            ambiguity: None,
        });
        next.state = DeliveryState::Claimed;
        Ok(Transition::Advanced(next))
    }

    fn arm(&self, number: AttemptNo, at: Timestamp) -> Outcome<Self> {
        let index = self.attempt_index(number)?;
        match self.attempts[index].state {
            AttemptState::ActivationArmed => Ok(Transition::Duplicate),
            AttemptState::Claimed => {
                let mut next = self.clone();
                let attempt = &mut next.attempts[index];
                attempt.state = AttemptState::ActivationArmed;
                attempt.armed = true;
                attempt.armed_at = Some(at);
                Ok(Transition::Advanced(next))
            }
            from => Err(Conflict::IllegalDeliveryTransition {
                from,
                event: "activation_armed",
            }),
        }
    }

    fn accept(&self, number: AttemptNo, evidence: &E, at: Timestamp) -> Outcome<Self> {
        let index = self.attempt_index(number)?;
        match self.attempts[index].state {
            // Acceptance is only reachable through the armed fence: a submit
            // that was never armed cannot have happened.
            AttemptState::ActivationArmed => {
                let mut next = self.clone();
                let attempt = &mut next.attempts[index];
                attempt.state = AttemptState::Accepted;
                attempt.finished_at = Some(at);
                next.state = DeliveryState::Accepted;
                next.accepted = Some(evidence.clone());
                Ok(Transition::Advanced(next))
            }
            AttemptState::Accepted if self.accepted.as_ref() == Some(evidence) => {
                Ok(Transition::Duplicate)
            }
            from => Err(Conflict::IllegalDeliveryTransition {
                from,
                event: "attempt_accepted",
            }),
        }
    }

    fn fail(&self, number: AttemptNo, failure: FailureClass, at: Timestamp) -> Outcome<Self> {
        let index = self.attempt_index(number)?;
        let current = self.attempts[index].state;
        match current {
            AttemptState::Claimed => {}
            AttemptState::ActivationArmed => {
                if !failure.proves_no_submit_after_arming() {
                    return Err(Conflict::FailureNotProven { from: current });
                }
            }
            AttemptState::Failed if self.attempts[index].failure == Some(failure) => {
                return Ok(Transition::Duplicate);
            }
            from => {
                return Err(Conflict::IllegalDeliveryTransition {
                    from,
                    event: "attempt_failed",
                });
            }
        }
        let mut next = self.clone();
        let attempt = &mut next.attempts[index];
        attempt.state = AttemptState::Failed;
        attempt.finished_at = Some(at);
        attempt.failure = Some(failure);
        next.state = DeliveryState::Failed;
        Ok(Transition::Advanced(next))
    }

    fn mark_ambiguous(
        &self,
        number: AttemptNo,
        reason: AmbiguityReason,
        at: Timestamp,
    ) -> Outcome<Self> {
        let index = self.attempt_index(number)?;
        match self.attempts[index].state {
            AttemptState::Claimed | AttemptState::ActivationArmed => {
                let mut next = self.clone();
                let attempt = &mut next.attempts[index];
                attempt.state = AttemptState::Ambiguous;
                attempt.finished_at = Some(at);
                attempt.ambiguity = Some(reason);
                next.state = DeliveryState::Ambiguous;
                Ok(Transition::Advanced(next))
            }
            AttemptState::Ambiguous => Ok(Transition::Duplicate),
            from => Err(Conflict::IllegalDeliveryTransition {
                from,
                event: "attempt_ambiguous",
            }),
        }
    }

    fn quarantine(&self, at: Timestamp) -> Outcome<Self> {
        let live: Vec<usize> = self
            .attempts
            .iter()
            .enumerate()
            .filter(|(_, a)| a.state.is_live())
            .map(|(i, _)| i)
            .collect();
        if live.is_empty() {
            // Nothing was left in flight; startup has nothing to quarantine.
            return Ok(Transition::Duplicate);
        }
        let mut next = self.clone();
        for index in live {
            let attempt = &mut next.attempts[index];
            attempt.state = AttemptState::Ambiguous;
            attempt.finished_at = Some(at);
            attempt.ambiguity = Some(AmbiguityReason::OrphanedByRestart);
        }
        next.state = DeliveryState::Ambiguous;
        Ok(Transition::Advanced(next))
    }

    fn reconcile(&self, evidence: &E, at: Timestamp, label: &'static str) -> Outcome<Self> {
        match self.state {
            DeliveryState::Ambiguous => {
                let mut next = self.clone();
                if let Some(index) = next
                    .attempts
                    .iter()
                    .rposition(|a| a.state == AttemptState::Ambiguous)
                {
                    let attempt = &mut next.attempts[index];
                    attempt.state = AttemptState::Accepted;
                    attempt.finished_at = Some(at);
                    attempt.ambiguity = None;
                }
                next.state = DeliveryState::Accepted;
                next.accepted = Some(evidence.clone());
                Ok(Transition::Advanced(next))
            }
            DeliveryState::Accepted if self.accepted.as_ref() == Some(evidence) => {
                Ok(Transition::Duplicate)
            }
            DeliveryState::Accepted => Err(Conflict::DeliveryRevisionFrozen { state: self.state }),
            _ => Err(Conflict::IllegalDeliveryTransition {
                from: self
                    .attempts
                    .last()
                    .map_or(AttemptState::Claimed, |a| a.state),
                event: label,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn claimed(budget: u32) -> Delivery<u8> {
        Delivery::pending(budget)
            .apply(&DeliveryEvent::AttemptClaimed { at: at(1) })
            .expect("first claim is legal")
            .advanced()
            .expect("claim advances")
    }

    fn armed(budget: u32) -> Delivery<u8> {
        claimed(budget)
            .apply(&DeliveryEvent::ActivationArmed {
                attempt: AttemptNo::FIRST,
                at: at(2),
            })
            .expect("arming a claimed attempt is legal")
            .advanced()
            .expect("arming advances")
    }

    #[test]
    fn no_io_permit_before_an_attempt_is_claimed() {
        let pending = Delivery::<u8>::pending(3);
        assert_eq!(pending.state(), DeliveryState::Pending);
        assert!(
            pending.io_permit().is_none(),
            "invariant 10: no browser I/O before claimed"
        );
        assert!(pending.send_activation().is_none());
    }

    #[test]
    fn claiming_grants_io_but_not_send() {
        let delivery = claimed(3);
        assert_eq!(delivery.state(), DeliveryState::Claimed);
        assert!(delivery.io_permit().is_some());
        assert!(
            delivery.send_activation().is_none(),
            "invariant 11: Send needs the armed fence"
        );
    }

    #[test]
    fn arming_grants_send_activation() {
        let delivery = armed(3);
        let activation = delivery.send_activation().expect("armed attempt may send");
        assert_eq!(activation.attempt(), AttemptNo::FIRST);
    }

    #[test]
    fn acceptance_requires_the_armed_fence() {
        let delivery = claimed(3);
        let err = delivery
            .apply(&DeliveryEvent::AttemptAccepted {
                attempt: AttemptNo::FIRST,
                evidence: 7,
                at: at(3),
            })
            .expect_err("accepting an unarmed attempt is illegal");
        assert_eq!(err.code(), "illegal_delivery_transition");
        assert_eq!(delivery.state(), DeliveryState::Claimed, "zero mutation");
    }

    #[test]
    fn pre_fence_failure_permits_a_bounded_retry() {
        let failed = claimed(3)
            .apply(&DeliveryEvent::AttemptFailed {
                attempt: AttemptNo::FIRST,
                failure: FailureClass::ComposerNotReady,
                at: at(3),
            })
            .expect("proven pre-submit failure is legal")
            .advanced()
            .expect("failure advances");
        assert_eq!(failed.state(), DeliveryState::Failed);
        assert!(failed.may_retry());

        let retried = failed
            .apply(&DeliveryEvent::AttemptClaimed { at: at(4) })
            .expect("retry is legal")
            .advanced()
            .expect("retry advances");
        assert_eq!(retried.attempts().len(), 2);
        assert_eq!(retried.state(), DeliveryState::Claimed);
    }

    #[test]
    fn retry_budget_is_bounded() {
        let mut delivery = claimed(2);
        delivery = delivery
            .apply(&DeliveryEvent::AttemptFailed {
                attempt: AttemptNo::FIRST,
                failure: FailureClass::TargetNotFound,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        delivery = delivery
            .apply(&DeliveryEvent::AttemptClaimed { at: at(4) })
            .unwrap()
            .advanced()
            .unwrap();
        delivery = delivery
            .apply(&DeliveryEvent::AttemptFailed {
                attempt: AttemptNo::new(2),
                failure: FailureClass::TargetNotFound,
                at: at(5),
            })
            .unwrap()
            .advanced()
            .unwrap();

        let err = delivery
            .apply(&DeliveryEvent::AttemptClaimed { at: at(6) })
            .expect_err("budget of two is spent");
        assert_eq!(err.code(), "retry_budget_exhausted");
    }

    #[test]
    fn failure_after_arming_must_be_proven() {
        let delivery = armed(3);
        let err = delivery
            .apply(&DeliveryEvent::AttemptFailed {
                attempt: AttemptNo::FIRST,
                failure: FailureClass::ComposerNotReady,
                at: at(3),
            })
            .expect_err("an unprovable class cannot claim failure after arming");
        assert_eq!(err.code(), "failure_not_proven");
        assert_eq!(delivery.state(), DeliveryState::Claimed, "zero mutation");
    }

    #[test]
    fn proven_failure_after_arming_never_retries() {
        let failed = armed(3)
            .apply(&DeliveryEvent::AttemptFailed {
                attempt: AttemptNo::FIRST,
                failure: FailureClass::ActivationRefused,
                at: at(3),
            })
            .expect("a synchronous activation refusal proves no submit")
            .advanced()
            .unwrap();
        assert_eq!(failed.state(), DeliveryState::Failed);
        let err = failed
            .apply(&DeliveryEvent::AttemptClaimed { at: at(4) })
            .expect_err("the fence was crossed, so retry is forbidden");
        assert_eq!(err.code(), "retry_after_ambiguity_fence");
    }

    #[test]
    fn accepted_is_frozen_forever() {
        let accepted = armed(3)
            .apply(&DeliveryEvent::AttemptAccepted {
                attempt: AttemptNo::FIRST,
                evidence: 9,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(accepted.state(), DeliveryState::Accepted);
        for event in [
            DeliveryEvent::AttemptClaimed { at: at(4) },
            DeliveryEvent::AttemptClaimed { at: at(5_000_000) },
        ] {
            let err = accepted.apply(&event).expect_err("accepted never resends");
            assert_eq!(err.code(), "delivery_revision_frozen");
        }
        // Startup quarantine leaves an accepted revision exactly as it was.
        assert!(
            accepted
                .apply(&DeliveryEvent::OrphanQuarantined { at: at(6) })
                .unwrap()
                .is_duplicate()
        );
    }

    #[test]
    fn ambiguous_is_frozen_and_never_auto_resent() {
        let ambiguous = armed(3)
            .apply(&DeliveryEvent::AttemptAmbiguous {
                attempt: AttemptNo::FIRST,
                reason: AmbiguityReason::ObservationLost,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(ambiguous.state(), DeliveryState::Ambiguous);
        let err = ambiguous
            .apply(&DeliveryEvent::AttemptClaimed { at: at(4) })
            .expect_err("ambiguous never resends");
        assert_eq!(err.code(), "delivery_revision_frozen");
        assert!(ambiguous.send_activation().is_none());
    }

    #[test]
    fn startup_quarantines_orphaned_claimed_and_armed_attempts() {
        for orphan in [claimed(3), armed(3)] {
            let quarantined = orphan
                .apply(&DeliveryEvent::OrphanQuarantined { at: at(10) })
                .expect("quarantine is always legal for a live attempt")
                .advanced()
                .expect("quarantine advances");
            assert_eq!(quarantined.state(), DeliveryState::Ambiguous);
            assert!(
                quarantined.io_permit().is_none(),
                "invariant 12: no recovery may touch a quarantined attempt"
            );
            assert!(quarantined.send_activation().is_none());
            assert_eq!(
                quarantined.attempts()[0].ambiguity(),
                Some(AmbiguityReason::OrphanedByRestart)
            );
        }
    }

    #[test]
    fn reconciliation_only_promotes_ambiguous_to_accepted() {
        let ambiguous = armed(3)
            .apply(&DeliveryEvent::AttemptAmbiguous {
                attempt: AttemptNo::FIRST,
                reason: AmbiguityReason::ObservationLost,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let promoted = ambiguous
            .apply(&DeliveryEvent::ReconciledAccepted {
                evidence: 11,
                at: at(4),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(promoted.state(), DeliveryState::Accepted);
        assert_eq!(promoted.accepted_evidence(), Some(&11));

        // A failed or pending revision is never promoted by reconciliation.
        let pending = Delivery::<u8>::pending(3);
        assert!(
            pending
                .apply(&DeliveryEvent::ReconciledAccepted {
                    evidence: 11,
                    at: at(4)
                })
                .is_err()
        );
    }

    #[test]
    fn duplicate_terminal_events_are_idempotent() {
        let accepted = armed(3)
            .apply(&DeliveryEvent::AttemptAccepted {
                attempt: AttemptNo::FIRST,
                evidence: 3,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let repeat = accepted
            .apply(&DeliveryEvent::AttemptAccepted {
                attempt: AttemptNo::FIRST,
                evidence: 3,
                at: at(3),
            })
            .unwrap();
        assert!(repeat.is_duplicate());
    }

    #[test]
    fn unknown_attempt_is_rejected() {
        let err = claimed(3)
            .apply(&DeliveryEvent::ActivationArmed {
                attempt: AttemptNo::new(9),
                at: at(3),
            })
            .expect_err("attempt nine does not exist");
        assert_eq!(err.code(), "unknown_attempt");
    }
}
