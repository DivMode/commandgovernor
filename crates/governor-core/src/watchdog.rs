//! The progress watchdog.
//!
//! For a running turn:
//!
//! ```text
//! last_verified_progress_at + threshold < now
//!   AND no confirmed terminal/input boundary
//!   -> suspected_stall attention
//! ```
//!
//! Invariant 16 is carried by [`WatchdogOutcome`]: it has no variant for
//! completion, failure, interruption, or spawning anything. Silence produces
//! attention and nothing else, and later verified progress resolves it.

use crate::health::HealthConditionKind;
use crate::time::{DurationMs, Timestamp};

/// What the watchdog observed about a running turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressWindow {
    /// When the turn was verified to start.
    pub started_at: Timestamp,
    /// The most recent verified progress, if any has been seen.
    ///
    /// "Verified" means structured or native tool progress. Screen repaint and
    /// PTY activity are not progress.
    pub last_verified_progress_at: Option<Timestamp>,
    /// Whether a confirmed terminal or input boundary has been reached.
    pub confirmed_boundary: bool,
    /// Whether a stall is already being reported for this turn.
    pub stall_already_open: bool,
}

impl ProgressWindow {
    /// The most recent instant that counts as progress.
    #[must_use]
    pub fn last_activity(&self) -> Timestamp {
        self.last_verified_progress_at
            .map_or(self.started_at, |progress| progress.max(self.started_at))
    }

    /// How long the turn has been silent at `now`.
    #[must_use]
    pub fn silence_at(&self, now: Timestamp) -> DurationMs {
        now.saturating_elapsed_since(self.last_activity())
    }
}

/// The only things the watchdog is allowed to conclude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogOutcome {
    /// Nothing to do.
    NoChange,
    /// Raise `suspected_stall` attention against the turn.
    RaiseSuspectedStall,
    /// Resolve the `suspected_stall` attention.
    ResolveSuspectedStall,
}

impl WatchdogOutcome {
    /// The health condition this outcome concerns.
    #[must_use]
    pub const fn condition(self) -> Option<HealthConditionKind> {
        match self {
            Self::NoChange => None,
            Self::RaiseSuspectedStall | Self::ResolveSuspectedStall => {
                Some(HealthConditionKind::SuspectedStall)
            }
        }
    }
}

/// Evaluates a running turn against the stall threshold.
///
/// Time arrives as data: `now` is an argument, never a clock read.
#[must_use]
pub fn evaluate(window: &ProgressWindow, threshold: DurationMs, now: Timestamp) -> WatchdogOutcome {
    // A confirmed boundary means the turn is not silently stuck; whatever
    // happens next is the lifecycle's business, not the watchdog's.
    if window.confirmed_boundary {
        return if window.stall_already_open {
            WatchdogOutcome::ResolveSuspectedStall
        } else {
            WatchdogOutcome::NoChange
        };
    }

    let silent_for = window.silence_at(now);
    if silent_for.as_millis() > threshold.as_millis() {
        if window.stall_already_open {
            WatchdogOutcome::NoChange
        } else {
            WatchdogOutcome::RaiseSuspectedStall
        }
    } else if window.stall_already_open {
        WatchdogOutcome::ResolveSuspectedStall
    } else {
        WatchdogOutcome::NoChange
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn threshold() -> DurationMs {
        DurationMs::from_millis(1_000)
    }

    fn window() -> ProgressWindow {
        ProgressWindow {
            started_at: at(0),
            last_verified_progress_at: Some(at(500)),
            confirmed_boundary: false,
            stall_already_open: false,
        }
    }

    #[test]
    fn progress_within_the_threshold_is_not_a_stall() {
        assert_eq!(
            evaluate(&window(), threshold(), at(1_400)),
            WatchdogOutcome::NoChange
        );
    }

    #[test]
    fn a_long_build_with_progress_never_stalls() {
        // Ten minutes of work, progress every 900ms: never a stall.
        let mut now = 0i64;
        while now < 600_000 {
            let window = ProgressWindow {
                started_at: at(0),
                last_verified_progress_at: Some(at(now)),
                confirmed_boundary: false,
                stall_already_open: false,
            };
            assert_eq!(
                evaluate(&window, threshold(), at(now + 900)),
                WatchdogOutcome::NoChange
            );
            now += 900;
        }
    }

    #[test]
    fn silence_beyond_the_threshold_raises_attention_once() {
        let window = window();
        assert_eq!(
            evaluate(&window, threshold(), at(2_000)),
            WatchdogOutcome::RaiseSuspectedStall
        );

        let already = ProgressWindow {
            stall_already_open: true,
            ..window
        };
        assert_eq!(
            evaluate(&already, threshold(), at(2_000)),
            WatchdogOutcome::NoChange,
            "the attention is raised once, not repeatedly"
        );
    }

    #[test]
    fn later_verified_progress_resolves_the_attention() {
        let recovered = ProgressWindow {
            last_verified_progress_at: Some(at(2_500)),
            stall_already_open: true,
            ..window()
        };
        assert_eq!(
            evaluate(&recovered, threshold(), at(3_000)),
            WatchdogOutcome::ResolveSuspectedStall
        );
    }

    #[test]
    fn a_confirmed_boundary_resolves_rather_than_concludes() {
        let finished = ProgressWindow {
            confirmed_boundary: true,
            stall_already_open: true,
            ..window()
        };
        assert_eq!(
            evaluate(&finished, threshold(), at(10_000_000)),
            WatchdogOutcome::ResolveSuspectedStall
        );
    }

    #[test]
    fn the_watchdog_can_only_ever_produce_attention() {
        // Exhaustive over the observable input space shape: whatever the
        // combination, the outcome is one of three attention verdicts. There is
        // no variant that could fabricate completion, failure or an interrupt.
        for confirmed in [false, true] {
            for open in [false, true] {
                for progress in [None, Some(at(500))] {
                    for now in [0i64, 999, 1_001, i64::from(u32::MAX)] {
                        let window = ProgressWindow {
                            started_at: at(0),
                            last_verified_progress_at: progress,
                            confirmed_boundary: confirmed,
                            stall_already_open: open,
                        };
                        let outcome = evaluate(&window, threshold(), at(now));
                        assert!(matches!(
                            outcome,
                            WatchdogOutcome::NoChange
                                | WatchdogOutcome::RaiseSuspectedStall
                                | WatchdogOutcome::ResolveSuspectedStall
                        ));
                        assert!(matches!(
                            outcome.condition(),
                            None | Some(HealthConditionKind::SuspectedStall)
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn a_backwards_clock_cannot_manufacture_a_stall() {
        let window = ProgressWindow {
            last_verified_progress_at: Some(at(10_000)),
            ..window()
        };
        assert_eq!(
            evaluate(&window, threshold(), at(1_000)),
            WatchdogOutcome::NoChange
        );
    }
}
