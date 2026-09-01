//! Reproducible artifact storage keys.
//!
//! `governor-artifacts` ships no default [`StorageKeySource`] for the same
//! reason the store ships no clock: a component that could reach entropy on its
//! own would make "everything ambient is injected" a convention again. A suite
//! supplies this one, and because the keys are a function of the seed and the
//! call count, a scenario can assert *which* file it expects to find.

use governor_artifacts::{StorageKey, StorageKeySource};

/// Sequential opaque keys of the form `ra-<seed>-<counter>`.
///
/// The seed is in the name so two artifact roots in one scenario cannot produce
/// the same key by accident, which would turn an immutability failure into a
/// confusing `AlreadyPublished`.
#[derive(Debug, Clone)]
pub struct SeededKeys {
    seed: u64,
    next: u64,
}

impl SeededKeys {
    /// Creates a key source for one artifact root.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed, next: 0 }
    }

    /// The key this source will hand out next, without consuming it.
    #[must_use]
    pub fn peek(&self) -> StorageKey {
        Self::format(self.seed, self.next + 1)
    }

    fn format(seed: u64, counter: u64) -> StorageKey {
        StorageKey::parse(&format!("ra-{seed:04x}-{counter:08}"))
            .expect("generated keys are valid single components")
    }
}

impl StorageKeySource for SeededKeys {
    fn next_key(&mut self) -> StorageKey {
        self.next += 1;
        Self::format(self.seed, self.next)
    }
}

/// A key source that hands out one fixed key forever.
///
/// For immutability tests: publishing twice under the same name must fail
/// closed rather than overwrite.
#[derive(Debug, Clone)]
pub struct FixedKey(StorageKey);

impl FixedKey {
    /// Wraps a literal key.
    ///
    /// # Panics
    ///
    /// Panics when `key` is not a legal storage key.
    #[must_use]
    pub fn new(key: &str) -> Self {
        Self(StorageKey::parse(key).expect("fixture key"))
    }
}

impl StorageKeySource for FixedKey {
    fn next_key(&mut self) -> StorageKey {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_sequential_and_predictable() {
        let mut keys = SeededKeys::new(1);
        let peeked = keys.peek();
        let first = keys.next_key();
        assert_eq!(peeked, first);
        assert_ne!(first, keys.next_key());
        assert_ne!(
            SeededKeys::new(1).next_key(),
            SeededKeys::new(2).next_key(),
            "two roots in one scenario never collide"
        );
    }
}
