//! Time as data.
//!
//! `governor-core` never reads a clock. Every transition that depends on time —
//! claim expiry, the watchdog stall threshold, resume backoff — receives the
//! current instant as an explicit argument, which is what makes those machines
//! replayable and their tests deterministic.

use core::fmt;

/// A wall-clock instant, in milliseconds since the Unix epoch.
///
/// Per [`docs/data-model.md`], wall-clock time is *evidence and diagnostics*,
/// never cross-process ordering authority; the durable event sequence orders
/// history. Comparing two timestamps is therefore only ever an input to a
/// policy decision, never a fence.
///
/// [`docs/data-model.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/data-model.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Builds an instant from Unix milliseconds.
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Returns Unix milliseconds, for persistence and diagnostics.
    #[must_use]
    pub const fn as_unix_millis(self) -> i64 {
        self.0
    }

    /// Returns this instant advanced by `delta`, saturating at the bounds.
    ///
    /// Saturation, rather than wrapping, keeps a nonsense clock from producing
    /// an expiry in the past.
    #[must_use]
    pub const fn saturating_add(self, delta: DurationMs) -> Self {
        let millis = if delta.0 > i64::MAX as u64 {
            i64::MAX
        } else {
            #[expect(
                clippy::cast_possible_wrap,
                reason = "guarded above: the value is within i64 range"
            )]
            let signed = delta.0 as i64;
            signed
        };
        Self(self.0.saturating_add(millis))
    }

    /// Returns how long this instant is after `earlier`, or zero if it is not.
    ///
    /// Clamping at zero means a clock that jumps backwards cannot manufacture a
    /// negative elapsed time and, through it, a false stall or a false expiry.
    #[must_use]
    pub const fn saturating_elapsed_since(self, earlier: Self) -> DurationMs {
        let delta = self.0.saturating_sub(earlier.0);
        if delta <= 0 {
            DurationMs::ZERO
        } else {
            #[expect(
                clippy::cast_sign_loss,
                reason = "guarded above: delta is strictly positive"
            )]
            let unsigned = delta as u64;
            DurationMs(unsigned)
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// A non-negative duration in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationMs(u64);

impl DurationMs {
    /// The zero duration.
    pub const ZERO: Self = Self(0);

    /// Builds a duration from milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    /// Returns the duration in milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }
}

impl fmt::Display for DurationMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_clamps_a_backwards_clock_to_zero() {
        let later = Timestamp::from_unix_millis(1_000);
        let earlier = Timestamp::from_unix_millis(5_000);
        assert_eq!(later.saturating_elapsed_since(earlier), DurationMs::ZERO);
    }

    #[test]
    fn elapsed_measures_forward_progress() {
        let start = Timestamp::from_unix_millis(1_000);
        let now = Timestamp::from_unix_millis(4_500);
        assert_eq!(
            now.saturating_elapsed_since(start),
            DurationMs::from_millis(3_500)
        );
    }

    #[test]
    fn adding_saturates_rather_than_wrapping() {
        let near_max = Timestamp::from_unix_millis(i64::MAX - 5);
        let bumped = near_max.saturating_add(DurationMs::from_millis(u64::MAX));
        assert_eq!(bumped.as_unix_millis(), i64::MAX);
        assert!(bumped >= near_max);
    }
}
