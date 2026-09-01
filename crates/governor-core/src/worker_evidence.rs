//! Worker evidence classes and their precedence.
//!
//! Per [`docs/adr/0005-worker-lifecycle-and-result-durability.md`], "newest
//! timestamp wins" is rejected: **evidence class plus fence** decides. This
//! module is the pure arbitration, and it is deliberately stingy about what it
//! will call terminal.
//!
//! Two properties are carried by the types rather than by discipline:
//!
//! - [`ConfirmedFinalResult`] has no public constructor, so the only way to
//!   obtain one is [`ManagedRunEvidence::classify`] returning
//!   [`WorkerOutcome::ConfirmedCompletion`]. A `Stop` callback — alone, twice,
//!   or vetoed by another hook — can never produce one (invariants 4 and 5).
//! - [`ConfirmedDeferredRun`] is likewise the only proof a defer actually took
//!   effect, and [`crate::input`] requires one before `needs_input` exists.
//!
//! Nothing here stores prompt text, tool arguments, results, commands, cwd, or
//! transcript paths: every field is an opaque [`SafeToken`], an enum, a bounded
//! number, or a flag.
//!
//! [`docs/adr/0005-worker-lifecycle-and-result-durability.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/adr/0005-worker-lifecycle-and-result-durability.md

use crate::fence::SafeToken;
use crate::health::HealthConditionKind;

/// Class of a worker/runtime observation, ordered by arbitration strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WorkerEvidenceClass {
    /// Final structured programmatic result plus matching child exit receipt.
    StructuredRunOutcome,
    /// Strong native evidence: `StopFailure`, `SessionEnd`, confirmed defer.
    StrongNativeSignal,
    /// A `Stop` hook callback. Another parallel hook may have vetoed the stop.
    StopCandidate,
    /// A `PermissionRequest` decision. No exact tool-use correlation.
    PermissionDecision,
    /// Runtime/session transport observation, such as Herdr `working`.
    RuntimeObservation,
    /// PTY idle or repaint heuristics. Diagnostics only.
    PtyHeuristic,
}

impl WorkerEvidenceClass {
    /// Arbitration rank, 1 strongest.
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::StructuredRunOutcome => 1,
            Self::StrongNativeSignal => 2,
            Self::StopCandidate => 3,
            Self::PermissionDecision => 4,
            Self::RuntimeObservation => 5,
            Self::PtyHeuristic => 6,
        }
    }

    /// Reports whether this class outranks `other` for the same fenced turn.
    #[must_use]
    pub const fn outranks(self, other: Self) -> bool {
        self.precedence() < other.precedence()
    }
}

/// Outcome reported by a complete structured managed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ManagedRunOutcome {
    /// The run produced a complete successful final result.
    Success,
    /// The run reported a structured error.
    Error,
    /// The run stopped with a tool call deferred and still pending.
    ToolDeferred,
    /// The run was interrupted.
    Interrupted,
}

/// The worker-host's bounded receipt for a complete final structured result.
///
/// `complete` is the completeness flag the parser sets only when it saw the
/// whole final record; a truncated stream leaves it `false` and can never be
/// promoted to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalResultReceipt {
    /// Opaque identity of the exact managed run.
    pub run_ref: SafeToken,
    /// Whether the final structured record was received in full.
    pub complete: bool,
    /// The structured outcome the run reported.
    pub outcome: ManagedRunOutcome,
}

/// How a managed child process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ChildExitStatus {
    /// Exited zero.
    Success,
    /// Exited with a non-zero status.
    Nonzero {
        /// Bounded numeric exit code.
        code: i32,
    },
    /// Terminated by a signal.
    Signalled,
    /// The exit status could not be determined.
    Unknown,
}

/// The worker-host's sanitised child-exit receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildExitReceipt {
    /// Opaque identity of the exact managed run, matched against the result.
    pub run_ref: SafeToken,
    /// How the child ended.
    pub status: ChildExitStatus,
}

/// A worker failure class that documented evidence combinations may project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WorkerFailureClass {
    /// The structured run reported an error and the child exited accordingly.
    StructuredError,
    /// A `StopFailure` was corroborated by a non-zero or signalled exit.
    StopFailure,
    /// The run was interrupted with a corroborating exit.
    Interrupted,
}

/// Proof that an exact managed run completed successfully.
///
/// No public constructor: obtaining one requires
/// [`ManagedRunEvidence::classify`] to have seen a complete successful final
/// structured result *and* a matching child exit receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedFinalResult {
    run_ref: SafeToken,
}

impl ConfirmedFinalResult {
    /// Opaque identity of the run this proof belongs to.
    #[must_use]
    pub const fn run_ref(&self) -> &SafeToken {
        &self.run_ref
    }
}

/// Proof that an exact managed run actually stopped with a tool deferred.
///
/// No public constructor, for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedDeferredRun {
    run_ref: SafeToken,
}

impl ConfirmedDeferredRun {
    /// Opaque identity of the run this proof belongs to.
    #[must_use]
    pub const fn run_ref(&self) -> &SafeToken {
        &self.run_ref
    }
}

/// What the runtime/session transport currently claims about the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RuntimeObservation {
    /// The transport reports the session busy.
    Working,
    /// The transport reports the session idle.
    Idle,
    /// The transport reports the session gone.
    Ended,
}

/// Arbitrated result of everything observed about one managed run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkerOutcome {
    /// Nothing conclusive yet. The turn keeps running.
    Indeterminate,
    /// Only `Stop` candidates were seen. Explicitly not completion.
    StopCandidateOnly {
        /// How many `Stop` callbacks fired. Any number is still not completion.
        candidates: u32,
    },
    /// The session ended without a successful result. Not completion.
    SessionEndedWithoutResult,
    /// A complete successful run with a matching exit.
    ConfirmedCompletion(ConfirmedFinalResult),
    /// A complete run that stopped with a tool call deferred and pending.
    ConfirmedDeferred(ConfirmedDeferredRun),
    /// A documented failure combination.
    ConfirmedFailure(WorkerFailureClass),
    /// Evidence is contradictory or incomplete; raise attention, close nothing.
    NeedsReconciliation(HealthConditionKind),
}

/// Everything observed about one managed run, as bounded safe evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedRunEvidence {
    final_result: Option<FinalResultReceipt>,
    child_exit: Option<ChildExitReceipt>,
    stop_candidates: u32,
    stop_failure: bool,
    session_end: bool,
    runtime: Option<RuntimeObservation>,
}

impl ManagedRunEvidence {
    /// Creates an empty evidence set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the bounded final-result receipt.
    #[must_use]
    pub fn with_final_result(mut self, receipt: FinalResultReceipt) -> Self {
        self.final_result = Some(receipt);
        self
    }

    /// Records the sanitised child-exit receipt.
    #[must_use]
    pub fn with_child_exit(mut self, receipt: ChildExitReceipt) -> Self {
        self.child_exit = Some(receipt);
        self
    }

    /// Records one more `Stop` hook callback.
    #[must_use]
    pub fn with_stop_candidate(mut self) -> Self {
        self.stop_candidates = self.stop_candidates.saturating_add(1);
        self
    }

    /// Records a `StopFailure` observation.
    #[must_use]
    pub fn with_stop_failure(mut self) -> Self {
        self.stop_failure = true;
        self
    }

    /// Records a `SessionEnd` observation.
    #[must_use]
    pub fn with_session_end(mut self) -> Self {
        self.session_end = true;
        self
    }

    /// Records the runtime transport's current claim.
    #[must_use]
    pub fn with_runtime(mut self, observation: RuntimeObservation) -> Self {
        self.runtime = Some(observation);
        self
    }

    /// Number of `Stop` callbacks seen.
    #[must_use]
    pub const fn stop_candidates(&self) -> u32 {
        self.stop_candidates
    }

    fn exit_matches_run(&self, run_ref: &SafeToken) -> Option<ChildExitStatus> {
        self.child_exit
            .as_ref()
            .filter(|exit| &exit.run_ref == run_ref)
            .map(|exit| exit.status)
    }

    /// Arbitrates every observation into one outcome.
    ///
    /// The order of the arms *is* the precedence rule: a structured run outcome
    /// is consulted first, native signals next, and a `Stop` callback only
    /// after both have declined to be conclusive.
    #[must_use]
    pub fn classify(&self) -> WorkerOutcome {
        if let Some(result) = &self.final_result {
            if !result.complete {
                // A truncated final record proves nothing at all.
                return WorkerOutcome::NeedsReconciliation(
                    HealthConditionKind::RuntimeStateConflict,
                );
            }
            let exit = self.exit_matches_run(&result.run_ref);
            return match (result.outcome, exit) {
                (ManagedRunOutcome::Success, Some(ChildExitStatus::Success)) => {
                    WorkerOutcome::ConfirmedCompletion(ConfirmedFinalResult {
                        run_ref: result.run_ref.clone(),
                    })
                }
                (ManagedRunOutcome::ToolDeferred, Some(ChildExitStatus::Success)) => {
                    WorkerOutcome::ConfirmedDeferred(ConfirmedDeferredRun {
                        run_ref: result.run_ref.clone(),
                    })
                }
                (ManagedRunOutcome::Error, Some(_)) => {
                    WorkerOutcome::ConfirmedFailure(WorkerFailureClass::StructuredError)
                }
                (ManagedRunOutcome::Interrupted, Some(_)) => {
                    WorkerOutcome::ConfirmedFailure(WorkerFailureClass::Interrupted)
                }
                // A complete result whose exit receipt is missing, unknown, or
                // belongs to another run is not trustworthy terminal evidence.
                (_, None | Some(ChildExitStatus::Unknown)) => {
                    WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
                }
                (ManagedRunOutcome::Success | ManagedRunOutcome::ToolDeferred, Some(_)) => {
                    WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
                }
            };
        }

        if self.stop_failure {
            return match self.child_exit.as_ref().map(|exit| exit.status) {
                Some(ChildExitStatus::Nonzero { .. } | ChildExitStatus::Signalled) => {
                    WorkerOutcome::ConfirmedFailure(WorkerFailureClass::StopFailure)
                }
                _ => WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict),
            };
        }

        // A successful child exit with no complete final result is exactly the
        // shape that must never become completion.
        if matches!(
            self.child_exit.as_ref().map(|exit| exit.status),
            Some(ChildExitStatus::Success)
        ) {
            return WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict);
        }

        if self.session_end {
            return WorkerOutcome::SessionEndedWithoutResult;
        }

        if self.stop_candidates > 0 {
            return WorkerOutcome::StopCandidateOnly {
                candidates: self.stop_candidates,
            };
        }

        WorkerOutcome::Indeterminate
    }

    /// Reports a runtime disagreement that must be recorded as attention.
    ///
    /// A stale `working` sample never vetoes a confirmed structured fact; it
    /// produces a `runtime_state_conflict` alongside the confirmed projection.
    #[must_use]
    pub fn runtime_conflict(&self) -> Option<HealthConditionKind> {
        let confirmed = matches!(
            self.classify(),
            WorkerOutcome::ConfirmedCompletion(_)
                | WorkerOutcome::ConfirmedDeferred(_)
                | WorkerOutcome::ConfirmedFailure(_)
        );
        if confirmed && self.runtime == Some(RuntimeObservation::Working) {
            Some(HealthConditionKind::RuntimeStateConflict)
        } else {
            None
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{
        ChildExitReceipt, ChildExitStatus, ConfirmedDeferredRun, ConfirmedFinalResult,
        FinalResultReceipt, ManagedRunEvidence, ManagedRunOutcome, WorkerOutcome,
    };
    use crate::fence::SafeToken;

    pub(crate) fn run_ref() -> SafeToken {
        SafeToken::new("run-1").expect("safe")
    }

    pub(crate) fn successful_run() -> ManagedRunEvidence {
        ManagedRunEvidence::new()
            .with_final_result(FinalResultReceipt {
                run_ref: run_ref(),
                complete: true,
                outcome: ManagedRunOutcome::Success,
            })
            .with_child_exit(ChildExitReceipt {
                run_ref: run_ref(),
                status: ChildExitStatus::Success,
            })
    }

    pub(crate) fn confirmed_completion() -> ConfirmedFinalResult {
        match successful_run().classify() {
            WorkerOutcome::ConfirmedCompletion(proof) => proof,
            other => panic!("expected confirmed completion, got {other:?}"),
        }
    }

    pub(crate) fn deferred_run() -> ManagedRunEvidence {
        ManagedRunEvidence::new()
            .with_final_result(FinalResultReceipt {
                run_ref: run_ref(),
                complete: true,
                outcome: ManagedRunOutcome::ToolDeferred,
            })
            .with_child_exit(ChildExitReceipt {
                run_ref: run_ref(),
                status: ChildExitStatus::Success,
            })
    }

    pub(crate) fn confirmed_deferred() -> ConfirmedDeferredRun {
        match deferred_run().classify() {
            WorkerOutcome::ConfirmedDeferred(proof) => proof,
            other => panic!("expected confirmed defer, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{run_ref, successful_run};
    use super::*;

    #[test]
    fn precedence_matches_the_documented_order() {
        assert!(
            WorkerEvidenceClass::StructuredRunOutcome
                .outranks(WorkerEvidenceClass::StrongNativeSignal)
        );
        assert!(
            WorkerEvidenceClass::StrongNativeSignal.outranks(WorkerEvidenceClass::StopCandidate)
        );
        assert!(
            WorkerEvidenceClass::StopCandidate.outranks(WorkerEvidenceClass::PermissionDecision)
        );
        assert!(
            WorkerEvidenceClass::PermissionDecision
                .outranks(WorkerEvidenceClass::RuntimeObservation)
        );
        assert!(
            WorkerEvidenceClass::RuntimeObservation.outranks(WorkerEvidenceClass::PtyHeuristic)
        );
    }

    #[test]
    fn complete_result_plus_matching_exit_confirms_completion() {
        let outcome = successful_run().classify();
        match outcome {
            WorkerOutcome::ConfirmedCompletion(proof) => assert_eq!(proof.run_ref(), &run_ref()),
            other => panic!("expected completion, got {other:?}"),
        }
    }

    #[test]
    fn stop_candidate_alone_is_never_completion() {
        let outcome = ManagedRunEvidence::new().with_stop_candidate().classify();
        assert_eq!(outcome, WorkerOutcome::StopCandidateOnly { candidates: 1 });
    }

    #[test]
    fn vetoed_stop_candidates_are_never_completion() {
        // Two candidates with a blocked stop in between and further progress:
        // the count grows and the classification does not move.
        let evidence = ManagedRunEvidence::new()
            .with_stop_candidate()
            .with_stop_candidate();
        assert_eq!(
            evidence.classify(),
            WorkerOutcome::StopCandidateOnly { candidates: 2 }
        );

        // Only the final result plus exit flips it.
        let finished = evidence
            .with_final_result(FinalResultReceipt {
                run_ref: run_ref(),
                complete: true,
                outcome: ManagedRunOutcome::Success,
            })
            .with_child_exit(ChildExitReceipt {
                run_ref: run_ref(),
                status: ChildExitStatus::Success,
            });
        assert!(matches!(
            finished.classify(),
            WorkerOutcome::ConfirmedCompletion(_)
        ));
    }

    #[test]
    fn truncated_final_result_is_never_completion() {
        let outcome = ManagedRunEvidence::new()
            .with_final_result(FinalResultReceipt {
                run_ref: run_ref(),
                complete: false,
                outcome: ManagedRunOutcome::Success,
            })
            .with_child_exit(ChildExitReceipt {
                run_ref: run_ref(),
                status: ChildExitStatus::Success,
            })
            .classify();
        assert_eq!(
            outcome,
            WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
        );
    }

    #[test]
    fn successful_exit_without_a_final_result_is_not_completion() {
        let outcome = ManagedRunEvidence::new()
            .with_child_exit(ChildExitReceipt {
                run_ref: run_ref(),
                status: ChildExitStatus::Success,
            })
            .classify();
        assert_eq!(
            outcome,
            WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
        );
    }

    #[test]
    fn final_result_without_a_trustworthy_exit_is_not_completion() {
        let outcome = ManagedRunEvidence::new()
            .with_final_result(FinalResultReceipt {
                run_ref: run_ref(),
                complete: true,
                outcome: ManagedRunOutcome::Success,
            })
            .classify();
        assert_eq!(
            outcome,
            WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
        );
    }

    #[test]
    fn exit_receipt_for_another_run_does_not_count() {
        let outcome = ManagedRunEvidence::new()
            .with_final_result(FinalResultReceipt {
                run_ref: run_ref(),
                complete: true,
                outcome: ManagedRunOutcome::Success,
            })
            .with_child_exit(ChildExitReceipt {
                run_ref: SafeToken::new("run-2").unwrap(),
                status: ChildExitStatus::Success,
            })
            .classify();
        assert_eq!(
            outcome,
            WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
        );
    }

    #[test]
    fn session_end_alone_is_session_end_not_success() {
        let outcome = ManagedRunEvidence::new().with_session_end().classify();
        assert_eq!(outcome, WorkerOutcome::SessionEndedWithoutResult);
    }

    #[test]
    fn stop_failure_needs_a_corroborating_exit() {
        assert_eq!(
            ManagedRunEvidence::new().with_stop_failure().classify(),
            WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict)
        );
        assert_eq!(
            ManagedRunEvidence::new()
                .with_stop_failure()
                .with_child_exit(ChildExitReceipt {
                    run_ref: run_ref(),
                    status: ChildExitStatus::Nonzero { code: 1 },
                })
                .classify(),
            WorkerOutcome::ConfirmedFailure(WorkerFailureClass::StopFailure)
        );
    }

    #[test]
    fn confirmed_structured_truth_beats_a_stale_working_runtime() {
        let evidence = successful_run().with_runtime(RuntimeObservation::Working);
        assert!(matches!(
            evidence.classify(),
            WorkerOutcome::ConfirmedCompletion(_)
        ));
        assert_eq!(
            evidence.runtime_conflict(),
            Some(HealthConditionKind::RuntimeStateConflict),
            "the disagreement is recorded, not resolved in the runtime's favour"
        );
    }

    #[test]
    fn no_evidence_is_indeterminate() {
        assert_eq!(
            ManagedRunEvidence::new().classify(),
            WorkerOutcome::Indeterminate
        );
    }
}
