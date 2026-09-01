//! The ports a write operation may use — and only *before* it opens a
//! transaction.
//!
//! # Why this module is the whole "no I/O inside a transaction" rule
//!
//! `docs/data-model.md` principle 5 and `docs/architecture.md` both require that
//! no browser, network, worker, GitHub or runtime I/O happens while a SQLite
//! transaction is held. A comment saying so is worth nothing; the structural
//! version is that **inside a transaction there is nothing to call**.
//!
//! Everything ambient this crate can reach lives behind [`StorePorts`]:
//! the clock, the CSPRNG, and identity minting. [`StorePorts`] is handed to
//! [`WriteOp::prepare`](crate::tx::WriteOp::prepare), which runs before
//! `BEGIN IMMEDIATE`, and is *not* a parameter of
//! [`WriteOp::commit`](crate::tx::WriteOp::commit), which runs inside it. A
//! transaction body therefore cannot read a clock, draw entropy, mint an
//! identity, or reach any adapter, because none of them is in scope.
//!
//! The crate itself performs no filesystem, network or process I/O at all —
//! rusqlite owns the only file handle — and `no_ambient_io.rs` in the test suite
//! scans `src/` to keep it that way.
//!
//! # No ambient implementations
//!
//! This crate deliberately ships *no* real clock and *no* real CSPRNG. A store
//! that could reach the system clock on its own would make the rule above a
//! convention again. The daemon supplies real implementations at composition
//! time; tests supply deterministic ones, which is what makes the crash and
//! replay suites reproducible.

use governor_core::delivery::DeliveryId;
use governor_core::id::{Id, IdKind, IdSource};
use governor_core::lease::LeaseToken;
use governor_core::random::SecureRandom;
use governor_core::time::Timestamp;

/// A source of wall-clock instants.
///
/// Wall-clock time is evidence and diagnostics, never ordering authority: the
/// daemon-assigned event sequence orders history.
pub trait Clock: Send {
    /// Returns the current instant.
    fn now(&self) -> Timestamp;
}

impl<T: Clock + ?Sized> Clock for Box<T> {
    fn now(&self) -> Timestamp {
        (**self).now()
    }
}

/// The complete set of non-SQLite capabilities the store may use.
///
/// Held by the writer actor and lent only to `prepare`.
pub struct StorePorts {
    clock: Box<dyn Clock>,
    rng: Box<dyn SecureRandom + Send>,
    ids: Box<dyn IdSource + Send>,
}

impl StorePorts {
    /// Bundles the three ports a store needs.
    #[must_use]
    pub fn new(
        clock: Box<dyn Clock>,
        rng: Box<dyn SecureRandom + Send>,
        ids: Box<dyn IdSource + Send>,
    ) -> Self {
        Self { clock, rng, ids }
    }

    /// Reads the current instant.
    pub(crate) fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// Mints the next opaque identity of a family.
    pub(crate) fn next_id<K: IdKind>(&mut self) -> Id<K> {
        Id::from_uuid(self.ids.next_uuid())
    }

    /// Draws a fresh browser wake correlation ID from the CSPRNG.
    ///
    /// The caller may discard the result: a delivery that turns out to already
    /// exist keeps the correlation ID it was created with, and this one is
    /// thrown away rather than persisted. See
    /// [`crate::ops::delivery::CreateOrClaimDelivery`].
    pub(crate) fn draw_delivery_id(&mut self) -> DeliveryId {
        DeliveryId::generate(self.rng.as_mut())
    }

    /// Draws a fresh resource-lease possession token from the CSPRNG.
    ///
    /// Same shape as [`Self::draw_delivery_id`], and for the same reason: the
    /// value must be unguessable, and the transaction body that persists it has
    /// no entropy source in scope.
    pub(crate) fn draw_lease_token(&mut self) -> LeaseToken {
        LeaseToken::generate(self.rng.as_mut())
    }
}

impl std::fmt::Debug for StorePorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StorePorts { .. }")
    }
}
