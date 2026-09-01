//! Exclusive ownership of a local resource: canonical identity, a random lease
//! token, and the incarnation fences that make a stale holder harmless.
//!
//! ```text
//! unowned --(acquire)--> held --(renew)--> held --(release)--> released
//!                          ^                                      |
//!                          +--------------(acquire)---------------+
//! ```
//!
//! A lease binds five things, and all five are checked:
//!
//! | Part | Answers |
//! | --- | --- |
//! | [`ResourceIdentity`] | *which* resource, canonically |
//! | [`LeaseToken`] | does the caller actually hold the lease |
//! | [`ProcessIncarnation`] | is the caller the same OS process instance |
//! | [`DaemonEpoch`] | is the caller from the current daemon lifetime |
//! | [`crate::id::ActorId`] | who, semantically, holds it |
//!
//! # PID reuse is the whole point of the incarnation
//!
//! [`ProcessIncarnation`] is a [`ProcessSlot`] — the conceptual OS process
//! number — *plus* an opaque [`ProcessStartRef`]. Equality requires both, so a
//! recycled process number paired with a different start identity is a
//! **different** incarnation and cannot renew or release the old lease. This is
//! the one distributed-systems mistake a local daemon actually makes, and it is
//! why the process number alone is never an identity here.
//!
//! # Deliberately local-first
//!
//! No quorum, no consensus, no fencing-token service. Per the research review's
//! "do not overbuild" note, the global daemon/state-root lock stays simpler than
//! a lease; this machinery exists for resources where a second process
//! legitimately participates. Expiry is a liveness hint, not an authority: an
//! expired lease may be taken over, and the takeover is what invalidates the old
//! token, not the passage of time on its own.

use core::fmt;

use sha2::{Digest, Sha256};

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{DaemonEpoch, SafeToken};
use crate::id::{ActorId, ResourceLeaseId};
use crate::random::SecureRandom;
use crate::time::{DurationMs, Timestamp};

/// Domain-separation label for [`ResourceIdentity`] digests.
///
/// Changing this string changes every derived identity and is a protocol break.
pub const RESOURCE_IDENTITY_DOMAIN: &str = "command-governor/resource-identity/v1";

/// Width of a lease token, in bytes.
///
/// 256 bits, matching the wake correlation ID: a lease token is a possession
/// fence and must not be guessable.
pub const LEASE_TOKEN_BYTES: usize = 32;

/// Canonical identity of an exclusively-owned resource.
///
/// A resource is usually named by something the control plane must not store —
/// a canonical filesystem path, a socket location, a profile directory. So the
/// identity is a namespace plus a *digest* of that canonical name: two callers
/// naming the same resource agree, and the ledger never holds the name itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceIdentity {
    namespace: ResourceNamespace,
    digest: [u8; 32],
}

impl ResourceIdentity {
    /// Derives the identity from a namespace and the resource's canonical name.
    ///
    /// The caller canonicalises first — resolving symlinks, case and relative
    /// segments — because two spellings of one resource must produce one
    /// identity. The name is hashed and dropped; only the digest survives.
    #[must_use]
    pub fn canonical(namespace: ResourceNamespace, canonical_name: &str) -> Self {
        let mut hasher = Sha256::new();
        let mut absorb = |bytes: &[u8]| {
            let len = u64::try_from(bytes.len()).expect("bounded name length fits in u64");
            hasher.update(len.to_be_bytes());
            hasher.update(bytes);
        };
        absorb(RESOURCE_IDENTITY_DOMAIN.as_bytes());
        absorb(namespace.as_token().as_str().as_bytes());
        absorb(canonical_name.as_bytes());
        Self {
            namespace,
            digest: hasher.finalize().into(),
        }
    }

    /// Rehydrates an identity the store previously persisted.
    #[must_use]
    pub const fn from_persisted(namespace: ResourceNamespace, digest: [u8; 32]) -> Self {
        Self { namespace, digest }
    }

    /// The resource-class namespace.
    #[must_use]
    pub const fn namespace(&self) -> &ResourceNamespace {
        &self.namespace
    }

    /// The digest of the canonical name, for persistence.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

/// The class of resource an identity belongs to.
///
/// A [`SafeToken`] rather than a closed enum: the daemon owns the vocabulary of
/// resources it locks, and `governor-core` has no business enumerating them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceNamespace(SafeToken);

impl ResourceNamespace {
    /// Wraps the opaque namespace label.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the label, for persistence and diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

impl fmt::Display for ResourceNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The conceptual OS process number of a lease holder.
///
/// It is **not** an identity on its own, and nothing in this module treats it
/// as one. It exists so a mismatch can be classified as a reused number rather
/// than an unrelated process, which is a materially different diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessSlot(u32);

impl ProcessSlot {
    /// Wraps an observed process number.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the process number, for diagnostics.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ProcessSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Opaque evidence of *when* a process instance started.
///
/// The daemon supplies whatever its platform can prove — a boot-relative start
/// tick, a process start time, a kernel-assigned generation. `governor-core`
/// only compares it for equality, so the exact source is the adapter's choice
/// and never a correctness dependency here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessStartRef(SafeToken);

impl ProcessStartRef {
    /// Wraps the opaque start identity.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the start identity, for persistence and diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// Identity of one OS process instance.
///
/// Two incarnations are equal only when the process number *and* the start
/// identity match. A recycled process number is therefore a different
/// incarnation, which is exactly the impersonation this type exists to stop.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessIncarnation {
    slot: ProcessSlot,
    start: ProcessStartRef,
}

impl ProcessIncarnation {
    /// Builds an incarnation from an observed process number and start identity.
    #[must_use]
    pub const fn new(slot: ProcessSlot, start: ProcessStartRef) -> Self {
        Self { slot, start }
    }

    /// The conceptual process number.
    #[must_use]
    pub const fn slot(&self) -> ProcessSlot {
        self.slot
    }

    /// The opaque start identity.
    #[must_use]
    pub const fn start(&self) -> &ProcessStartRef {
        &self.start
    }

    /// Classifies how `other` differs from this incarnation.
    #[must_use]
    pub fn classify(&self, other: &Self) -> Option<IncarnationMismatch> {
        if self == other {
            None
        } else if self.slot == other.slot {
            Some(IncarnationMismatch::SlotReused)
        } else {
            Some(IncarnationMismatch::DifferentProcess)
        }
    }
}

impl fmt::Display for ProcessIncarnation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.slot, self.start.0)
    }
}

/// How a presented incarnation differs from the recorded one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncarnationMismatch {
    /// Same process number, different start identity: the number was recycled.
    SlotReused,
    /// A different process number entirely.
    DifferentProcess,
}

impl IncarnationMismatch {
    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SlotReused => "slot_reused",
            Self::DifferentProcess => "different_process",
        }
    }
}

/// Unguessable possession fence for one lease.
///
/// Mirrors [`crate::delivery::DeliveryId`]: minted only from the injected
/// CSPRNG, redacted in `Debug`, no `Display`, and compared without early exit.
/// The store persists the raw bytes as a blob; there is no text form, so it
/// cannot end up in a log line by way of a formatter.
#[derive(Clone)]
pub struct LeaseToken([u8; LEASE_TOKEN_BYTES]);

impl LeaseToken {
    /// Draws a new token from the injected CSPRNG.
    ///
    /// This is the only way to mint one. Note what it does *not* take: no
    /// resource, no process number, no epoch — nothing a stale holder could
    /// recompute from what it already knows.
    #[must_use]
    pub fn generate(rng: &mut dyn SecureRandom) -> Self {
        let mut bytes = [0u8; LEASE_TOKEN_BYTES];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Rehydrates a token the store previously persisted.
    #[must_use]
    pub const fn from_persisted_bytes(bytes: [u8; LEASE_TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    /// Reveals the raw bytes, for persistence only.
    ///
    /// Named `expose_` so every call site reads as a deliberate disclosure.
    #[must_use]
    pub const fn expose_bytes(&self) -> &[u8; LEASE_TOKEN_BYTES] {
        &self.0
    }
}

impl PartialEq for LeaseToken {
    /// Compares in constant time with respect to the byte contents.
    ///
    /// The loop always runs to completion and accumulates differences instead
    /// of returning early, so a caller probing tokens learns nothing from how
    /// long the comparison took. Best effort in safe Rust, and layered on the
    /// incarnation and epoch fences rather than relied on alone.
    fn eq(&self, other: &Self) -> bool {
        let mut difference = 0u8;
        for (left, right) in self.0.iter().zip(other.0.iter()) {
            difference |= left ^ right;
        }
        difference == 0
    }
}

impl Eq for LeaseToken {}

impl core::hash::Hash for LeaseToken {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for LeaseToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LeaseToken(<redacted>)")
    }
}

/// Lifecycle of one lease record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LeaseState {
    /// The lease currently owns the resource.
    Held,
    /// The holder gave the resource back.
    Released,
}

impl LeaseState {
    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Released => "released",
        }
    }
}

/// One lease over one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLease {
    id: ResourceLeaseId,
    resource: ResourceIdentity,
    token: LeaseToken,
    holder: ActorId,
    incarnation: ProcessIncarnation,
    daemon_epoch: DaemonEpoch,
    state: LeaseState,
    acquired_at: Timestamp,
    renewed_at: Timestamp,
    expires_at: Timestamp,
    released_at: Option<Timestamp>,
}

impl ResourceLease {
    /// Lease identity.
    #[must_use]
    pub const fn id(&self) -> ResourceLeaseId {
        self.id
    }

    /// Resource this lease owns.
    #[must_use]
    pub const fn resource(&self) -> &ResourceIdentity {
        &self.resource
    }

    /// Reveals the possession token, for persistence only.
    #[must_use]
    pub const fn expose_token(&self) -> &LeaseToken {
        &self.token
    }

    /// Semantic holder of the lease.
    #[must_use]
    pub const fn holder(&self) -> ActorId {
        self.holder
    }

    /// Process instance that acquired the lease.
    #[must_use]
    pub const fn incarnation(&self) -> &ProcessIncarnation {
        &self.incarnation
    }

    /// Daemon epoch the lease was acquired under.
    #[must_use]
    pub const fn daemon_epoch(&self) -> DaemonEpoch {
        self.daemon_epoch
    }

    /// Current lease state.
    #[must_use]
    pub const fn state(&self) -> LeaseState {
        self.state
    }

    /// Instant the lease was first acquired.
    #[must_use]
    pub const fn acquired_at(&self) -> Timestamp {
        self.acquired_at
    }

    /// Instant the lease was last renewed, or acquired if never renewed.
    #[must_use]
    pub const fn renewed_at(&self) -> Timestamp {
        self.renewed_at
    }

    /// Instant the lease stops asserting liveness.
    #[must_use]
    pub const fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Instant the lease was released, if it was.
    #[must_use]
    pub const fn released_at(&self) -> Option<Timestamp> {
        self.released_at
    }

    /// Reports whether the lease has passed its expiry at `now`.
    #[must_use]
    pub const fn is_expired_at(&self, now: Timestamp) -> bool {
        now.as_unix_millis() >= self.expires_at.as_unix_millis()
    }

    /// Reports whether the lease is holding the resource at `now`.
    #[must_use]
    pub const fn is_live_at(&self, now: Timestamp) -> bool {
        matches!(self.state, LeaseState::Held) && !self.is_expired_at(now)
    }
}

/// The fences an acquire must present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRequest {
    /// Semantic holder asking for the resource.
    pub holder: ActorId,
    /// Process instance making the request.
    pub incarnation: ProcessIncarnation,
    /// Daemon epoch the request is made under.
    pub daemon_epoch: DaemonEpoch,
    /// How long the lease asserts liveness before it may be taken over.
    pub ttl: DurationMs,
}

/// The fences a renew or release must present.
///
/// All three are required, and all three are checked. Holding the token is not
/// enough: a stale process incarnation or a superseded daemon epoch cannot
/// mutate current ownership even with the right bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseHolderProof {
    /// The possession token the holder was given.
    pub token: LeaseToken,
    /// The process instance claiming to be the holder.
    pub incarnation: ProcessIncarnation,
    /// The daemon epoch the claim is made under.
    pub daemon_epoch: DaemonEpoch,
}

/// A newly granted lease, and the ownership record that now carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseGranted {
    /// Ownership record advanced to the new lease.
    pub ownership: ResourceOwnership,
    /// The possession token the holder must keep and present.
    pub token: LeaseToken,
}

/// Exclusive-ownership record for one resource.
///
/// The record keeps the *last* lease even after release, so a superseded holder
/// can still be told precisely why it lost, and so the daemon epoch fence has
/// something to compare against on the next acquire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceOwnership {
    resource: ResourceIdentity,
    current: Option<ResourceLease>,
}

impl ResourceOwnership {
    /// Creates an ownership record for a resource nobody holds.
    #[must_use]
    pub const fn unowned(resource: ResourceIdentity) -> Self {
        Self {
            resource,
            current: None,
        }
    }

    /// The resource this record is about.
    #[must_use]
    pub const fn resource(&self) -> &ResourceIdentity {
        &self.resource
    }

    /// The most recent lease, held or released.
    #[must_use]
    pub const fn current(&self) -> Option<&ResourceLease> {
        self.current.as_ref()
    }

    /// Reports whether a live lease holds the resource at `now`.
    #[must_use]
    pub fn is_held_at(&self, now: Timestamp) -> bool {
        self.current
            .as_ref()
            .is_some_and(|lease| lease.is_live_at(now))
    }

    /// Acquires the resource, minting a fresh possession token.
    ///
    /// A resource with no lease, a released lease, or a lease whose liveness
    /// has expired may be acquired; the grant supersedes whatever came before,
    /// and the old token stops matching from that moment.
    ///
    /// # Errors
    ///
    /// - [`Conflict::StaleDaemonEpoch`] when the request comes from a daemon
    ///   lifetime older than the one that last held the resource;
    /// - [`Conflict::ResourceAlreadyLeased`] when a live lease holds it.
    ///
    /// Both leave the record untouched, and neither mints a token.
    pub fn acquire(
        &self,
        request: &LeaseRequest,
        id: ResourceLeaseId,
        rng: &mut dyn SecureRandom,
        now: Timestamp,
    ) -> Result<LeaseGranted, Conflict> {
        if let Some(current) = &self.current {
            if request.daemon_epoch < current.daemon_epoch {
                return Err(Conflict::StaleDaemonEpoch {
                    presented: request.daemon_epoch,
                    current: current.daemon_epoch,
                });
            }
            if current.is_live_at(now) {
                return Err(Conflict::ResourceAlreadyLeased {
                    lease: current.id,
                    holder: current.holder,
                });
            }
        }
        let token = LeaseToken::generate(rng);
        let lease = ResourceLease {
            id,
            resource: self.resource.clone(),
            token: token.clone(),
            holder: request.holder,
            incarnation: request.incarnation.clone(),
            daemon_epoch: request.daemon_epoch,
            state: LeaseState::Held,
            acquired_at: now,
            renewed_at: now,
            expires_at: now.saturating_add(request.ttl),
            released_at: None,
        };
        Ok(LeaseGranted {
            ownership: Self {
                resource: self.resource.clone(),
                current: Some(lease),
            },
            token,
        })
    }

    /// Extends the current lease's liveness.
    ///
    /// An expired-but-not-superseded lease may still be renewed by its exact
    /// holder: expiry is a takeover *opportunity*, and until somebody takes it,
    /// the holder has not lost anything.
    ///
    /// # Errors
    ///
    /// Returns the first failing fence — [`Conflict::NoCurrentLease`],
    /// [`Conflict::IllegalLeaseTransition`], [`Conflict::StaleLeaseToken`],
    /// [`Conflict::StaleProcessIncarnation`], [`Conflict::StaleDaemonEpoch`] —
    /// and leaves the record untouched.
    pub fn renew(
        &self,
        proof: &LeaseHolderProof,
        ttl: DurationMs,
        now: Timestamp,
    ) -> Outcome<Self> {
        let current = self.require_held("renew")?;
        Self::check_proof(current, proof)?;
        let mut renewed = current.clone();
        renewed.renewed_at = now;
        renewed.expires_at = now.saturating_add(ttl);
        Ok(Transition::Advanced(Self {
            resource: self.resource.clone(),
            current: Some(renewed),
        }))
    }

    /// Releases the current lease.
    ///
    /// An exact repeat of a release that already happened is an idempotent
    /// duplicate, so a holder retrying after a lost reply does not see a
    /// spurious conflict.
    ///
    /// # Errors
    ///
    /// Returns the first failing fence — [`Conflict::NoCurrentLease`],
    /// [`Conflict::StaleLeaseToken`], [`Conflict::StaleProcessIncarnation`],
    /// [`Conflict::StaleDaemonEpoch`] — and leaves the record untouched.
    pub fn release(&self, proof: &LeaseHolderProof, now: Timestamp) -> Outcome<Self> {
        let Some(current) = &self.current else {
            return Err(Conflict::NoCurrentLease);
        };
        // The fences are checked before the state, so a stale holder is told it
        // is stale rather than told the lease is already gone.
        Self::check_proof(current, proof)?;
        match current.state {
            LeaseState::Released => Ok(Transition::Duplicate),
            LeaseState::Held => {
                let mut released = current.clone();
                released.state = LeaseState::Released;
                released.released_at = Some(now);
                Ok(Transition::Advanced(Self {
                    resource: self.resource.clone(),
                    current: Some(released),
                }))
            }
        }
    }

    fn require_held(&self, event: &'static str) -> Result<&ResourceLease, Conflict> {
        let Some(current) = &self.current else {
            return Err(Conflict::NoCurrentLease);
        };
        if current.state == LeaseState::Held {
            Ok(current)
        } else {
            Err(Conflict::IllegalLeaseTransition {
                from: current.state,
                event,
            })
        }
    }

    /// Checks possession, then process incarnation, then daemon epoch.
    ///
    /// The order matters for what a caller is told: a holder that lost the
    /// resource to a takeover learns its token is stale, while a holder whose
    /// token is genuinely current but whose process was replaced learns the
    /// far more specific thing — that its incarnation is not the one that
    /// acquired the lease.
    fn check_proof(current: &ResourceLease, proof: &LeaseHolderProof) -> Result<(), Conflict> {
        if current.token != proof.token {
            return Err(Conflict::StaleLeaseToken { lease: current.id });
        }
        if let Some(mismatch) = current.incarnation.classify(&proof.incarnation) {
            return Err(Conflict::StaleProcessIncarnation {
                lease: current.id,
                mismatch,
            });
        }
        if proof.daemon_epoch != current.daemon_epoch {
            return Err(Conflict::StaleDaemonEpoch {
                presented: proof.daemon_epoch,
                current: current.daemon_epoch,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{
        LeaseHolderProof, LeaseToken, ProcessIncarnation, ProcessSlot, ProcessStartRef,
        ResourceIdentity, ResourceNamespace,
    };
    use crate::fence::{DaemonEpoch, SafeToken};

    pub(crate) fn namespace(value: &str) -> ResourceNamespace {
        ResourceNamespace::new(SafeToken::new(value).expect("fixture namespaces are safe"))
    }

    pub(crate) fn resource(name: &str) -> ResourceIdentity {
        ResourceIdentity::canonical(namespace("session"), name)
    }

    pub(crate) fn incarnation(slot: u32, start: &str) -> ProcessIncarnation {
        ProcessIncarnation::new(
            ProcessSlot::new(slot),
            ProcessStartRef::new(SafeToken::new(start).expect("fixture start refs are safe")),
        )
    }

    pub(crate) fn proof(
        token: &LeaseToken,
        slot: u32,
        start: &str,
        epoch: DaemonEpoch,
    ) -> LeaseHolderProof {
        LeaseHolderProof {
            token: token.clone(),
            incarnation: incarnation(slot, start),
            daemon_epoch: epoch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{incarnation, proof, resource};
    use super::*;
    use uuid::Uuid;

    struct StreamRng {
        next: u8,
    }

    impl SecureRandom for StreamRng {
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for slot in dest.iter_mut() {
                *slot = self.next;
                self.next = self.next.wrapping_add(1);
            }
        }
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn actor(n: u128) -> ActorId {
        ActorId::from_uuid(Uuid::from_u128(n))
    }

    fn lease_id(n: u128) -> ResourceLeaseId {
        ResourceLeaseId::from_uuid(Uuid::from_u128(n))
    }

    fn request(epoch: DaemonEpoch) -> LeaseRequest {
        LeaseRequest {
            holder: actor(1),
            incarnation: incarnation(4242, "start-a"),
            daemon_epoch: epoch,
            ttl: DurationMs::from_millis(30_000),
        }
    }

    fn held() -> (ResourceOwnership, LeaseToken) {
        let mut rng = StreamRng { next: 1 };
        let granted = ResourceOwnership::unowned(resource("/canonical/session/a"))
            .acquire(&request(DaemonEpoch::FIRST), lease_id(1), &mut rng, at(100))
            .expect("an unowned resource is acquirable");
        (granted.ownership, granted.token)
    }

    #[test]
    fn canonical_identity_is_deterministic_and_hides_the_name() {
        let a = resource("/Volumes/Data/state/session-a");
        let b = resource("/Volumes/Data/state/session-a");
        let c = resource("/Volumes/Data/state/session-b");
        assert_eq!(a, b);
        assert_ne!(a, c);
        // The digest is what is stored; the path is not recoverable from it and
        // is not present in the value.
        assert_eq!(a.digest().len(), 32);
        assert_eq!(format!("{}", a.namespace()), "session");
    }

    #[test]
    fn acquiring_an_unowned_resource_mints_a_live_lease() {
        let (ownership, token) = held();
        let lease = ownership.current().expect("the resource is now owned");
        assert_eq!(lease.state(), LeaseState::Held);
        assert_eq!(lease.holder(), actor(1));
        assert_eq!(lease.expires_at(), at(30_100));
        assert!(ownership.is_held_at(at(200)));
        assert!(!ownership.is_held_at(at(30_100)));
        assert_eq!(lease.expose_token(), &token);
    }

    #[test]
    fn a_live_lease_blocks_a_second_acquire() {
        let (ownership, _) = held();
        let mut rng = StreamRng { next: 200 };
        let err = ownership
            .acquire(&request(DaemonEpoch::FIRST), lease_id(2), &mut rng, at(200))
            .expect_err("the resource is exclusively owned");
        assert_eq!(err.code(), "resource_already_leased");
        assert!(
            ownership.is_held_at(at(200)),
            "the refused acquire changed nothing"
        );
    }

    #[test]
    fn an_expired_lease_may_be_taken_over_and_the_old_token_stops_working() {
        let (ownership, old_token) = held();
        let mut rng = StreamRng { next: 200 };
        let granted = ownership
            .acquire(
                &request(DaemonEpoch::FIRST),
                lease_id(2),
                &mut rng,
                at(30_100),
            )
            .expect("an expired lease may be taken over");
        assert_ne!(granted.token, old_token);

        let err = granted
            .ownership
            .release(
                &proof(&old_token, 4242, "start-a", DaemonEpoch::FIRST),
                at(30_200),
            )
            .expect_err("the superseded holder cannot release");
        assert_eq!(err.code(), "stale_lease_token");
    }

    #[test]
    fn a_reused_process_slot_cannot_renew_or_release() {
        let (ownership, token) = held();
        // Same conceptual PID, different process-start identity: an unrelated
        // process that happens to have inherited the number.
        let impostor = proof(&token, 4242, "start-b", DaemonEpoch::FIRST);

        let err = ownership
            .renew(&impostor, DurationMs::from_millis(30_000), at(200))
            .expect_err("a recycled process number is a different incarnation");
        assert_eq!(err.code(), "stale_process_incarnation");
        assert_eq!(
            err,
            Conflict::StaleProcessIncarnation {
                lease: lease_id(1),
                mismatch: IncarnationMismatch::SlotReused,
            }
        );

        let err = ownership
            .release(&impostor, at(200))
            .expect_err("and it cannot release either");
        assert_eq!(err.code(), "stale_process_incarnation");
        assert_eq!(
            ownership.current().unwrap().state(),
            LeaseState::Held,
            "zero mutation on conflict"
        );
    }

    #[test]
    fn a_different_process_is_classified_separately() {
        let (ownership, token) = held();
        let other = proof(&token, 5151, "start-c", DaemonEpoch::FIRST);
        let err = ownership.release(&other, at(200)).unwrap_err();
        assert_eq!(
            err,
            Conflict::StaleProcessIncarnation {
                lease: lease_id(1),
                mismatch: IncarnationMismatch::DifferentProcess,
            }
        );
        assert_eq!(IncarnationMismatch::SlotReused.code(), "slot_reused");
        assert_eq!(
            IncarnationMismatch::DifferentProcess.code(),
            "different_process"
        );
    }

    #[test]
    fn a_stale_daemon_epoch_cannot_mutate_or_release() {
        let mut rng = StreamRng { next: 1 };
        let granted = ResourceOwnership::unowned(resource("/canonical/session/a"))
            .acquire(
                &request(DaemonEpoch::new(4)),
                lease_id(1),
                &mut rng,
                at(100),
            )
            .expect("acquire under epoch four");
        let ownership = granted.ownership;
        let stale = proof(&granted.token, 4242, "start-a", DaemonEpoch::new(3));

        for err in [
            ownership
                .renew(&stale, DurationMs::from_millis(1_000), at(200))
                .unwrap_err(),
            ownership.release(&stale, at(200)).unwrap_err(),
        ] {
            assert_eq!(err.code(), "stale_daemon_epoch");
        }
        assert!(ownership.is_held_at(at(200)), "nothing changed");
    }

    #[test]
    fn a_stale_daemon_epoch_cannot_take_over_an_expired_lease() {
        let mut rng = StreamRng { next: 1 };
        let granted = ResourceOwnership::unowned(resource("/canonical/session/a"))
            .acquire(
                &request(DaemonEpoch::new(4)),
                lease_id(1),
                &mut rng,
                at(100),
            )
            .unwrap();
        let mut rng = StreamRng { next: 90 };
        let err = granted
            .ownership
            .acquire(
                &request(DaemonEpoch::new(3)),
                lease_id(2),
                &mut rng,
                at(999_999),
            )
            .expect_err("a superseded daemon lifetime never wins the resource");
        assert_eq!(err.code(), "stale_daemon_epoch");
    }

    #[test]
    fn the_exact_holder_renews_and_releases() {
        let (ownership, token) = held();
        let holder = proof(&token, 4242, "start-a", DaemonEpoch::FIRST);
        let renewed = ownership
            .renew(&holder, DurationMs::from_millis(10_000), at(200))
            .expect("the exact holder may renew")
            .advanced()
            .expect("renew advances");
        assert_eq!(renewed.current().unwrap().expires_at(), at(10_200));
        assert_eq!(renewed.current().unwrap().renewed_at(), at(200));

        let released = renewed
            .release(&holder, at(300))
            .expect("the exact holder may release")
            .advanced()
            .unwrap();
        assert_eq!(released.current().unwrap().state(), LeaseState::Released);
        assert!(!released.is_held_at(at(300)));
        assert!(
            released.release(&holder, at(400)).unwrap().is_duplicate(),
            "a repeated release is idempotent"
        );
    }

    #[test]
    fn an_expired_lease_is_still_renewable_by_its_own_holder() {
        let (ownership, token) = held();
        let holder = proof(&token, 4242, "start-a", DaemonEpoch::FIRST);
        assert!(!ownership.is_held_at(at(30_100)));
        let renewed = ownership
            .renew(&holder, DurationMs::from_millis(30_000), at(30_100))
            .expect("nobody took over, so nothing was lost")
            .advanced()
            .unwrap();
        assert!(renewed.is_held_at(at(30_101)));
    }

    #[test]
    fn a_released_lease_cannot_be_renewed() {
        let (ownership, token) = held();
        let holder = proof(&token, 4242, "start-a", DaemonEpoch::FIRST);
        let released = ownership
            .release(&holder, at(300))
            .unwrap()
            .advanced()
            .unwrap();
        let err = released
            .renew(&holder, DurationMs::from_millis(1_000), at(400))
            .expect_err("a released lease is not renewable");
        assert_eq!(err.code(), "illegal_lease_transition");
    }

    #[test]
    fn an_unowned_resource_has_nothing_to_renew_or_release() {
        let ownership = ResourceOwnership::unowned(resource("/canonical/session/a"));
        let token = LeaseToken::from_persisted_bytes([7u8; LEASE_TOKEN_BYTES]);
        let holder = proof(&token, 4242, "start-a", DaemonEpoch::FIRST);
        assert_eq!(
            ownership
                .renew(&holder, DurationMs::from_millis(1_000), at(1))
                .unwrap_err()
                .code(),
            "no_current_lease"
        );
        assert_eq!(
            ownership.release(&holder, at(1)).unwrap_err().code(),
            "no_current_lease"
        );
    }

    #[test]
    fn the_csprng_port_is_the_only_way_to_mint_a_token() {
        let mut rng = StreamRng { next: 0 };
        let a = LeaseToken::generate(&mut rng);
        let b = LeaseToken::generate(&mut rng);
        assert_ne!(a, b);
        // No constructor takes a resource, a slot, an epoch, or an actor, so no
        // deterministic function of lease metadata can produce a token.
        assert_eq!(format!("{a:?}"), "LeaseToken(<redacted>)");
    }
}
