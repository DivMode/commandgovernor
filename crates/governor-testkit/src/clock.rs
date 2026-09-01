//! A clock the test moves by hand.
//!
//! `governor-core` never reads a clock and `governor-store-sqlite` ships none,
//! so a suite decides what "now" is. [`FakeClock`] is that decision made
//! explicit: it returns a fixed instant until a test calls [`FakeClock::advance`],
//! which is what lets claim expiry, retention grace and orphan grace be driven
//! exactly rather than raced against the machine's speed.
//!
//! It is shared: a clone handed to the store and a clone kept by the test read
//! and write the same instant, so time can move while a store is open.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use governor_core::time::{DurationMs, Timestamp};
use governor_store_sqlite::Clock;

/// The instant a scenario starts at unless it says otherwise.
///
/// Small and far from any real epoch value, so a timestamp that leaked from a
/// real clock into a fixture would be obvious.
pub const DEFAULT_CLOCK_START_MS: i64 = 1_000;

/// A shared, manually advanced wall clock.
#[derive(Debug, Clone)]
pub struct FakeClock {
    now: Arc<AtomicI64>,
    /// Milliseconds added on every reading, usually zero.
    step: i64,
}

impl FakeClock {
    /// Creates a clock frozen at `start_ms`.
    #[must_use]
    pub fn new(start_ms: i64) -> Self {
        Self {
            now: Arc::new(AtomicI64::new(start_ms)),
            step: 0,
        }
    }

    /// Creates a clock that advances one millisecond per reading.
    ///
    /// Useful where a scenario wants every recorded instant to differ without
    /// the test having to say so at each step; the total elapsed time is still
    /// a function of how many instants were asked for, never of wall time.
    #[must_use]
    pub fn stepping(start_ms: i64) -> Self {
        Self {
            now: Arc::new(AtomicI64::new(start_ms)),
            step: 1,
        }
    }

    /// The current instant, without advancing a stepping clock.
    #[must_use]
    pub fn peek(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.now.load(Ordering::Relaxed))
    }

    /// Moves the clock forward.
    pub fn advance(&self, delta: DurationMs) {
        let millis = i64::try_from(delta.as_millis()).unwrap_or(i64::MAX);
        self.now.fetch_add(millis, Ordering::Relaxed);
    }

    /// Moves the clock to an exact instant.
    pub fn set(&self, at: Timestamp) {
        self.now.store(at.as_unix_millis(), Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        let value = self.now.fetch_add(self.step, Ordering::Relaxed);
        Timestamp::from_unix_millis(value)
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new(DEFAULT_CLOCK_START_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frozen_clock_does_not_move_on_its_own() {
        let clock = FakeClock::new(500);
        assert_eq!(clock.now(), Timestamp::from_unix_millis(500));
        assert_eq!(clock.now(), Timestamp::from_unix_millis(500));
        clock.advance(DurationMs::from_millis(250));
        assert_eq!(clock.now(), Timestamp::from_unix_millis(750));
    }

    #[test]
    fn a_clone_shares_the_same_instant() {
        let clock = FakeClock::new(0);
        let handle = clock.clone();
        handle.advance(DurationMs::from_millis(10));
        assert_eq!(clock.peek(), Timestamp::from_unix_millis(10));
    }

    #[test]
    fn a_stepping_clock_advances_once_per_reading() {
        let clock = FakeClock::stepping(0);
        assert_eq!(clock.now(), Timestamp::from_unix_millis(0));
        assert_eq!(clock.now(), Timestamp::from_unix_millis(1));
        assert_eq!(clock.peek(), Timestamp::from_unix_millis(2));
    }
}
