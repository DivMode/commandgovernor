//! The production implementations of the ports the kernel and store require.
//!
//! `governor-core` reads no clock and no entropy source, and
//! `governor-store-sqlite` deliberately ships neither a real clock nor a real
//! CSPRNG — its `ports` module says so in as many words, and a test scans its
//! source to keep it true. This is the one place in the workspace where those
//! capabilities are real, which is what makes every crash, replay and
//! determinism suite reproducible everywhere else.
//!
//! Three ports, three choices:
//!
//! - **clock**: the system wall clock, in Unix milliseconds. Wall-clock time is
//!   evidence and diagnostics, never ordering authority — the daemon-assigned
//!   event sequence orders history — so a clock that jumps is a diagnostic
//!   nuisance rather than a correctness problem;
//! - **entropy**: `getrandom`, the OS entropy source directly. Not a userspace
//!   generator: there is no seed to get wrong, no state to share across a fork,
//!   and nothing to reseed. It backs the browser wake `delivery_id` and the
//!   resource-lease possession token, both of which must be unguessable;
//! - **identity**: UUIDv7. `docs/data-model.md` permits it for generated public
//!   IDs, and `governor-core` depends on the *type* only, so the `v7` feature
//!   is enabled here rather than there. Nothing branches on an identity's bits;
//!   the time ordering is a storage-locality convenience.

use governor_artifacts::{StorageKey, StorageKeySource};
use governor_core::fence::SafeToken;
use governor_core::id::IdSource;
use governor_core::random::SecureRandom;
use governor_core::time::Timestamp;
use governor_store_sqlite::{Clock, StorePorts};
use uuid::Uuid;

/// The system wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let millis = match since_epoch {
            Ok(delta) => i64::try_from(delta.as_millis()).unwrap_or(i64::MAX),
            // A clock set before 1970 is nonsense, but a panic here would take
            // the daemon down over a diagnostic. Saturating keeps every
            // `saturating_elapsed_since` non-negative, which is what the domain
            // machines rely on.
            Err(_) => 0,
        };
        Timestamp::from_unix_millis(millis)
    }
}

/// The operating system's entropy source.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsRandom;

impl SecureRandom for OsRandom {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        // The trait has no error channel, and rightly: a process that cannot
        // draw entropy must not carry on minting values whose whole purpose is
        // to be unguessable. Failing loudly is the fail-closed behaviour.
        getrandom::fill(dest).expect("the operating system entropy source must be available");
    }
}

/// UUIDv7 identity minting.
#[derive(Debug, Clone, Copy, Default)]
pub struct Uuidv7Ids;

impl IdSource for Uuidv7Ids {
    fn next_uuid(&mut self) -> Uuid {
        Uuid::now_v7()
    }
}

/// Opaque, daemon-allocated result-artifact keys.
///
/// `docs/data-model.md`: *the daemon allocates `storage_ref`; workers never
/// supply filesystem paths*. This is that allocator: a fixed prefix plus a
/// UUIDv7, which is a single path component by construction.
#[derive(Debug, Clone, Copy, Default)]
pub struct Uuidv7Keys;

impl StorageKeySource for Uuidv7Keys {
    fn next_key(&mut self) -> StorageKey {
        let text = format!("ra-{}", Uuid::now_v7().simple());
        let token = SafeToken::new(&text).expect("a hyphenated hex key is a safe token");
        StorageKey::new(token).expect("a hyphenated hex key is a single path component")
    }
}

/// The complete set of ports a production store runs on.
#[must_use]
pub fn production_ports() -> StorePorts {
    StorePorts::new(
        Box::new(SystemClock),
        Box::new(OsRandom),
        Box::new(Uuidv7Ids),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_clock_reports_a_plausible_present() {
        // 2020-01-01, chosen so the assertion outlives the project rather than
        // pinning a value that goes stale.
        const AFTER: i64 = 1_577_836_800_000;
        assert!(SystemClock.now().as_unix_millis() > AFTER);
    }

    #[test]
    fn the_clock_never_moves_backwards_within_one_read_pair() {
        let first = SystemClock.now();
        let second = SystemClock.now();
        assert!(second.saturating_elapsed_since(first).as_millis() < 60_000);
    }

    #[test]
    fn entropy_fills_the_whole_buffer_and_does_not_repeat() {
        let mut rng = OsRandom;
        let mut seen = BTreeSet::new();
        for _ in 0..32 {
            let mut buffer = [0_u8; 24];
            rng.fill_bytes(&mut buffer);
            assert!(buffer.iter().any(|byte| *byte != 0), "all-zero draw");
            assert!(seen.insert(buffer), "the CSPRNG repeated a 192-bit draw");
        }
    }

    #[test]
    fn identities_are_distinct_and_version_seven() {
        let mut ids = Uuidv7Ids;
        let first = ids.next_uuid();
        let second = ids.next_uuid();
        assert_ne!(first, second);
        assert_eq!(first.get_version_num(), 7);
    }

    #[test]
    fn artifact_keys_are_distinct_single_components() {
        let mut keys = Uuidv7Keys;
        let first = keys.next_key();
        let second = keys.next_key();
        assert_ne!(first, second);
        for key in [&first, &second] {
            let text = key.as_token().as_str();
            assert!(!text.contains('/'), "{text} is not a single component");
            assert!(text.starts_with("ra-"));
        }
    }
}
