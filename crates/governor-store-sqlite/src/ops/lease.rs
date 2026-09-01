//! Durable exclusive ownership of a local resource.
//!
//! # Deliberately small
//!
//! `docs/research/2026-08-31-durable-orchestration-pattern-review.md` is
//! explicit: *"For V1, the global daemon/state-root lock remains simpler than a
//! distributed lease. Use the richer lease pattern for session/runtime/browser
//! resources only where a second process legitimately participates."*
//!
//! So this is not the daemon lock — that stays a file lock owned by
//! `governor-daemon`. What lives here is the store side of the narrower case,
//! and it is exactly one table holding one row per resource: the most recent
//! lease, kept after release so a superseded holder can still be told precisely
//! why it lost. That mirrors [`ResourceOwnership`], which holds exactly the
//! same thing, so replay is a direct read rather than a reconstruction.
//!
//! The daemon epoch itself lives in `meta`, not here: it is a property of the
//! process lifetime, and mutation commands and external attempts fence against
//! it too.
//!
//! # What survives a restart, and why it fails closed
//!
//! The row records the possession token, the process incarnation and the daemon
//! epoch. [`ResourceOwnership::renew`] and [`ResourceOwnership::release`] check
//! all three, so after a restart:
//!
//! - a recycled process number with a different start reference is a different
//!   incarnation and cannot renew or release;
//! - a superseded daemon epoch cannot mutate or take over current ownership;
//! - a token from a lease that was taken over no longer matches.

use governor_core::fence::DaemonEpoch;
use governor_core::id::{ActorId, ResourceLeaseId};
use governor_core::lease::{
    LEASE_TOKEN_BYTES, LeaseHolderProof, LeaseRequest, LeaseState, LeaseToken, ProcessIncarnation,
    ProcessSlot, ProcessStartRef, ResourceIdentity, ResourceNamespace, ResourceOwnership,
};
use governor_core::time::{DurationMs, Timestamp};
use rusqlite::{OptionalExtension as _, params};

use crate::codec::{
    encode_lease_state, hex32, id_text, parse_id, parse_token, parse_token_bytes, parse_u32,
    parse_u64, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::ports::StorePorts;
use crate::tx::{Failpoint, Tx, WriteOp};

const TABLE: &str = "resource_leases";

/// The canonical resource a lease is about, as the store addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    /// Resource-class namespace.
    pub namespace: ResourceNamespace,
    /// Digest of the canonical name. The name itself is never stored.
    pub digest: [u8; 32],
}

impl ResourceRef {
    /// Builds the store's reference from a domain identity.
    #[must_use]
    pub fn of(identity: &ResourceIdentity) -> Self {
        Self {
            namespace: identity.namespace().clone(),
            digest: *identity.digest(),
        }
    }

    fn identity(&self) -> ResourceIdentity {
        ResourceIdentity::from_persisted(self.namespace.clone(), self.digest)
    }
}

/// Taking exclusive ownership of a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireLeaseRequest {
    /// Which resource.
    pub resource: ResourceRef,
    /// Semantic holder asking for it.
    pub holder: ActorId,
    /// Process instance making the request.
    pub incarnation: ProcessIncarnation,
    /// Daemon epoch the request is made under.
    pub daemon_epoch: DaemonEpoch,
    /// How long the lease asserts liveness before it may be taken over.
    pub ttl: DurationMs,
}

/// A granted lease and the token its holder must keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantedLease {
    /// Identity of the new lease.
    pub lease: ResourceLeaseId,
    /// The possession fence. Present it to renew or release.
    pub token: LeaseToken,
    /// Instant the lease stops asserting liveness.
    pub expires_at: Timestamp,
}

/// Acquires a resource, minting a fresh possession token.
pub(crate) struct AcquireLease {
    request: AcquireLeaseRequest,
    lease: ResourceLeaseId,
    token: LeaseToken,
    now: Timestamp,
}

impl WriteOp for AcquireLease {
    type Request = AcquireLeaseRequest;
    type Committed = GrantedLease;
    type Output = GrantedLease;

    const NAME: &'static str = "acquire_lease";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            lease: ports.next_id(),
            token: ports.draw_lease_token(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let ownership = read(tx, &self.request.resource)?;
        // `acquire` mints its own token from a CSPRNG, which a transaction body
        // cannot reach. The token drawn in `prepare` is fed in through a
        // one-shot source so the domain rules still decide everything else.
        let mut rng = OneShot::new(self.token.clone());
        let granted = ownership.acquire(
            &LeaseRequest {
                holder: self.request.holder,
                incarnation: self.request.incarnation.clone(),
                daemon_epoch: self.request.daemon_epoch,
                ttl: self.request.ttl,
            },
            self.lease,
            &mut rng,
            self.now,
        )?;
        let lease = granted
            .ownership
            .current()
            .expect("a granted acquire always leaves a current lease");
        write(tx, &self.request.resource, lease)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(GrantedLease {
            lease: self.lease,
            token: granted.token,
            expires_at: lease.expires_at(),
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Extending or giving up a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseHolderRequest {
    /// Which resource.
    pub resource: ResourceRef,
    /// The three fences a holder must present. All three are checked.
    pub proof: LeaseHolderProof,
    /// New liveness window, for a renewal.
    pub ttl: DurationMs,
}

/// Extends the current lease's liveness.
pub(crate) struct RenewLease {
    request: LeaseHolderRequest,
    now: Timestamp,
}

impl WriteOp for RenewLease {
    type Request = LeaseHolderRequest;
    type Committed = Timestamp;
    type Output = Timestamp;

    const NAME: &'static str = "renew_lease";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let ownership = read(tx, &self.request.resource)?;
        let renewed = ownership
            .renew(&self.request.proof, self.request.ttl, self.now)?
            .or_unchanged(ownership);
        let lease = renewed
            .current()
            .expect("a renewed ownership always has a current lease");
        write(tx, &self.request.resource, lease)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(lease.expires_at())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Gives the resource back.
pub(crate) struct ReleaseLease {
    request: LeaseHolderRequest,
    now: Timestamp,
}

impl WriteOp for ReleaseLease {
    type Request = LeaseHolderRequest;
    type Committed = LeaseState;
    type Output = LeaseState;

    const NAME: &'static str = "release_lease";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let ownership = read(tx, &self.request.resource)?;
        let released = ownership
            .release(&self.request.proof, self.now)?
            .or_unchanged(ownership);
        let lease = released
            .current()
            .expect("a released ownership still records its last lease");
        write(tx, &self.request.resource, lease)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(lease.state())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

// --- Row access -------------------------------------------------------------

fn write(
    tx: &Tx<'_>,
    resource: &ResourceRef,
    lease: &governor_core::lease::ResourceLease,
) -> StoreResult<()> {
    tx.conn().execute(
        "INSERT INTO resource_leases (resource_namespace, resource_digest, resource_lease_id,
                lease_token, holder_actor_id, process_slot, process_start_ref, daemon_epoch,
                state, acquired_at_ms, renewed_at_ms, expires_at_ms, released_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(resource_namespace, resource_digest) DO UPDATE SET
                resource_lease_id = excluded.resource_lease_id,
                lease_token = excluded.lease_token,
                holder_actor_id = excluded.holder_actor_id,
                process_slot = excluded.process_slot,
                process_start_ref = excluded.process_start_ref,
                daemon_epoch = excluded.daemon_epoch,
                state = excluded.state,
                acquired_at_ms = excluded.acquired_at_ms,
                renewed_at_ms = excluded.renewed_at_ms,
                expires_at_ms = excluded.expires_at_ms,
                released_at_ms = excluded.released_at_ms",
        params![
            resource.namespace.as_token().as_str(),
            hex32(&resource.digest),
            id_text(lease.id()),
            lease.expose_token().expose_bytes().as_slice(),
            id_text(lease.holder()),
            i64::from(lease.incarnation().slot().get()),
            lease.incarnation().start().as_token().as_str(),
            store_u64(lease.daemon_epoch().get(), TABLE, "daemon_epoch")?,
            encode_lease_state(lease.state()),
            store_time(lease.acquired_at()),
            store_time(lease.renewed_at()),
            store_time(lease.expires_at()),
            lease.released_at().map(store_time),
        ],
    )?;
    Ok(())
}

/// Reads the ownership record for a resource, unowned when there is no row.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an undecodable row.
pub(crate) fn read(tx: &Tx<'_>, resource: &ResourceRef) -> StoreResult<ResourceOwnership> {
    type Row = (
        String,
        Vec<u8>,
        String,
        i64,
        String,
        i64,
        String,
        i64,
        i64,
        i64,
        Option<i64>,
    );
    let row: Option<Row> = tx
        .conn()
        .query_row(
            "SELECT resource_lease_id, lease_token, holder_actor_id, process_slot,
                    process_start_ref, daemon_epoch, state, acquired_at_ms, renewed_at_ms,
                    expires_at_ms, released_at_ms
               FROM resource_leases
              WHERE resource_namespace = ?1 AND resource_digest = ?2",
            params![
                resource.namespace.as_token().as_str(),
                hex32(&resource.digest)
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .optional()?;
    let identity = resource.identity();
    let Some((
        lease_id,
        token,
        holder,
        slot,
        start,
        epoch,
        state,
        acquired,
        renewed,
        expires,
        released,
    )) = row
    else {
        return Ok(ResourceOwnership::unowned(identity));
    };

    // Rebuilt by replaying the domain transitions the row records: acquire,
    // then release when the row says it was released. A row that no legal
    // sequence reaches fails closed rather than being trusted.
    let lease_id: ResourceLeaseId = parse_id(&lease_id, TABLE, "resource_lease_id")?;
    let mut rng = OneShot::new(LeaseToken::from_persisted_bytes(parse_token_bytes::<
        LEASE_TOKEN_BYTES,
    >(
        &token,
        TABLE,
        "lease_token",
    )?));
    let granted = ResourceOwnership::unowned(identity)
        .acquire(
            &LeaseRequest {
                holder: parse_id(&holder, TABLE, "holder_actor_id")?,
                incarnation: ProcessIncarnation::new(
                    ProcessSlot::new(parse_u32(slot, TABLE, "process_slot")?),
                    ProcessStartRef::new(parse_token(&start, TABLE, "process_start_ref")?),
                ),
                daemon_epoch: DaemonEpoch::new(parse_u64(epoch, TABLE, "daemon_epoch")?),
                ttl: DurationMs::from_millis(0),
            },
            lease_id,
            &mut rng,
            Timestamp::from_unix_millis(acquired),
        )
        .map_err(|_| CorruptValue::new(TABLE, "state", CorruptReason::UnprovableEvidence))?;

    let proof = LeaseHolderProof {
        token: granted.token.clone(),
        incarnation: granted
            .ownership
            .current()
            .expect("a granted acquire has a current lease")
            .incarnation()
            .clone(),
        daemon_epoch: granted
            .ownership
            .current()
            .expect("a granted acquire has a current lease")
            .daemon_epoch(),
    };
    let mut ownership = granted.ownership;
    // `renew` sets `expires_at = renewed_at + ttl`, so the recorded window is
    // measured from the *renewal*, not from the acquisition. Measuring it from
    // the wrong end would rebuild a lease with a different expiry and the
    // consistency check below would reject a perfectly good row.
    ownership = ownership
        .renew(
            &proof,
            DurationMs::from_millis(elapsed(renewed, expires)?),
            Timestamp::from_unix_millis(renewed),
        )?
        .or_unchanged(ownership);
    let stored_state = crate::codec::decode_lease_state(&state, TABLE)?;
    if stored_state == LeaseState::Released {
        let at = released.ok_or_else(|| {
            CorruptValue::new(TABLE, "released_at_ms", CorruptReason::MalformedMetadata)
        })?;
        ownership = ownership
            .release(&proof, Timestamp::from_unix_millis(at))?
            .or_unchanged(ownership);
    }
    let current = ownership
        .current()
        .expect("the rebuilt ownership has a current lease");
    if current.state() != stored_state || current.expires_at().as_unix_millis() != expires {
        return Err(CorruptValue::new(TABLE, "state", CorruptReason::UnprovableEvidence).into());
    }
    Ok(ownership)
}

fn elapsed(from: i64, to: i64) -> StoreResult<u64> {
    u64::try_from(to.saturating_sub(from)).map_err(|_| {
        CorruptValue::new(TABLE, "expires_at_ms", CorruptReason::IntegerOutOfRange).into()
    })
}

/// A [`governor_core::random::SecureRandom`] that replays one recorded value.
///
/// The domain API mints tokens from a CSPRNG, and a transaction body has no
/// port to reach one through. This feeds it the token drawn in `prepare` — or,
/// on the read path, the token already persisted — so the *rules* still come
/// from `governor-core` while the entropy stays outside the transaction.
struct OneShot {
    bytes: [u8; LEASE_TOKEN_BYTES],
}

impl OneShot {
    fn new(token: LeaseToken) -> Self {
        Self {
            bytes: *token.expose_bytes(),
        }
    }
}

impl governor_core::random::SecureRandom for OneShot {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for (slot, byte) in dest.iter_mut().zip(self.bytes.iter().cycle()) {
            *slot = *byte;
        }
    }
}
