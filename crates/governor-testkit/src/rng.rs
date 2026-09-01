//! Seeded, reproducible substitutes for the two entropy-shaped ports.
//!
//! # Why identity and randomness must be different streams
//!
//! `docs/testing.md` DEL-001 and invariant 17 rest on one claim: the random
//! `delivery_id` is **not** a function of scheduling metadata, and opaque
//! identities are scheduling metadata. A testkit whose [`SeededIds`] and
//! [`SeededRandom`] drew from one counter would make that claim untestable —
//! the two would agree by construction, and a bug that derived the correlation
//! ID from an identity would pass.
//!
//! So one seed produces two domain-separated streams, and
//! [`SeededPorts::streams_are_independent`] is the assertion that they stay
//! apart.
//!
//! # Why `SplitMix64` and not a crate
//!
//! It is ten lines, it needs no dependency, and it produces exactly the same
//! bytes on every machine and toolchain, which is what a reproducible crash
//! matrix requires. It is **not** a CSPRNG and must never reach a daemon; that
//! is precisely why `governor-core` takes
//! [`SecureRandom`](governor_core::random::SecureRandom) as an injected port
//! rather than reading entropy itself.

use governor_core::id::IdSource;
use governor_core::random::SecureRandom;
use uuid::Uuid;

/// Domain separator mixed into the identity stream's seed.
const ID_DOMAIN: u64 = 0x6367_5f69_6473_0001;
/// Domain separator mixed into the randomness stream's seed.
const RNG_DOMAIN: u64 = 0x6367_5f72_6e67_0001;

/// A deterministic 64-bit generator.
///
/// `SplitMix64`, as published: one additive step and two xor-multiply-shift
/// finalisers. Every state produces a distinct sequence, so two streams seeded
/// apart stay apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// Seeds the generator.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Produces the next value and advances the state.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Produces the next value in a bounded range, `0..bound`.
    ///
    /// # Panics
    ///
    /// Panics when `bound` is zero.
    pub const fn next_below(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "a bound of zero has no values");
        self.next_u64() % bound
    }

    /// Picks one element of a slice.
    ///
    /// # Panics
    ///
    /// Panics on an empty slice.
    pub fn pick<'a, T>(&mut self, values: &'a [T]) -> &'a T {
        assert!(!values.is_empty(), "cannot pick from an empty slice");
        let len = u64::try_from(values.len()).expect("a slice length fits in u64");
        let index = self.next_below(len);
        &values[usize::try_from(index).expect("an index below a length fits")]
    }
}

/// A reproducible identity source.
///
/// Produces opaque 128-bit values from the identity stream. Correctness never
/// depends on parsing one (`docs/data-model.md`), so any distinct values will
/// do — what matters here is that a scenario replayed from one seed mints the
/// same identities twice.
#[derive(Debug, Clone)]
pub struct SeededIds(SplitMix64);

impl SeededIds {
    /// Seeds the identity stream, domain-separated from randomness.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(SplitMix64::new(seed ^ ID_DOMAIN))
    }
}

impl IdSource for SeededIds {
    fn next_uuid(&mut self) -> Uuid {
        let high = u128::from(self.0.next_u64());
        let low = u128::from(self.0.next_u64());
        Uuid::from_u128((high << 64) | low)
    }
}

/// A reproducible byte stream standing in for a CSPRNG.
///
/// Never acceptable in a daemon. It exists so a suite can assert *which*
/// correlation ID a scenario produced, which is what makes "the same seed
/// replays identically, a different seed does not" checkable.
#[derive(Debug, Clone)]
pub struct SeededRandom(SplitMix64);

impl SeededRandom {
    /// Seeds the randomness stream, domain-separated from identity.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(SplitMix64::new(seed ^ RNG_DOMAIN))
    }
}

impl SecureRandom for SeededRandom {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let word = self.0.next_u64().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

/// Both streams for one seed, and the proof that they are two.
#[derive(Debug, Clone)]
pub struct SeededPorts {
    /// The identity stream.
    pub ids: SeededIds,
    /// The randomness stream.
    pub random: SeededRandom,
}

impl SeededPorts {
    /// Derives both streams from one seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            ids: SeededIds::new(seed),
            random: SeededRandom::new(seed),
        }
    }

    /// Reports whether the two streams stay distinct for `seed`.
    ///
    /// Compares the first identities against the first random bytes drawn from
    /// the same seed. Equality would mean the testkit had quietly made identity
    /// *be* randomness, and every invariant-17 assertion built on it would be
    /// vacuous.
    #[must_use]
    pub fn streams_are_independent(seed: u64) -> bool {
        let mut ports = Self::new(seed);
        let mut identities = Vec::new();
        for _ in 0..4 {
            identities.extend_from_slice(ports.ids.next_uuid().as_bytes());
        }
        let mut noise = [0u8; 64];
        ports.random.fill_bytes(&mut noise);
        noise[..identities.len()] != identities[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_seed_replays_exactly() {
        let mut first = SeededRandom::new(7);
        let mut second = SeededRandom::new(7);
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        first.fill_bytes(&mut a);
        second.fill_bytes(&mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_seed_diverges() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        SeededRandom::new(7).fill_bytes(&mut a);
        SeededRandom::new(8).fill_bytes(&mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn identity_is_never_randomness() {
        for seed in 0..512u64 {
            assert!(
                SeededPorts::streams_are_independent(seed),
                "seed {seed} collapsed the two streams into one"
            );
        }
    }

    #[test]
    fn identities_do_not_repeat_within_a_stream() {
        let mut ids = SeededIds::new(1);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..10_000 {
            assert!(seen.insert(ids.next_uuid()), "the identity stream repeated");
        }
    }
}
