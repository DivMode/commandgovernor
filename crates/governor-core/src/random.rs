//! Injectable cryptographic randomness.
//!
//! `governor-core` reads no entropy source of its own. The one value in the
//! domain that must be unguessable — the browser wake `delivery_id` — is drawn
//! through this port, so the crate stays pure and tests can drive the exact
//! byte stream a scenario needs.

/// A cryptographically secure random byte source.
///
/// Implementations must be a CSPRNG. A test double may be deterministic, and
/// that is precisely why it must never be wired into a daemon: the security
/// property of [`DeliveryId`](crate::delivery::DeliveryId) is the caller's to
/// supply.
pub trait SecureRandom {
    /// Fills `dest` completely with cryptographically secure random bytes.
    fn fill_bytes(&mut self, dest: &mut [u8]);
}

impl<T: SecureRandom + ?Sized> SecureRandom for &mut T {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        (**self).fill_bytes(dest);
    }
}
