//! Authorising a worker process, in the one order that is safe.
//!
//! # Eleven steps, and the line the transaction is drawn on
//!
//! `governor-store-sqlite` cannot open a file — the source scan in its own test
//! suite enforces that — so the artifact read has to happen here, in the
//! composition root. What makes reading *outside* a transaction sound is that
//! the store re-checks the same facts *inside* one before it hands out
//! anything.
//!
//! ```text
//! OUTSIDE ANY TRANSACTION
//!  1. store.read_session_loadout(incarnation)   the persisted parts
//!  2. CommittedLoadout::rehydrate(parts)        pure; refusal => loadout_unverifiable
//!  3. artifacts.read_verified(key, ..)          the only filesystem read
//!     + SHA-256 over the bytes just read        refusal => managed_config_missing
//!  4. ManagedConfigVerified::verify(..)         the witness, from that observation
//!  5. committed.admit_resume(presented, ..)     pure; yields a ResumePermit
//!
//! INSIDE ONE BEGIN IMMEDIATE  (store::authorize_worker_spawn)
//!  6. re-read session_loadouts + worker_loadouts, require the (id, digest)
//!     pair to be identical to the one step 2 verified
//!  7. re-read managed_config_artifacts, require digest and length identical to
//!     what step 3 observed
//!  8. INSERT the external_attempts intent row
//! COMMIT
//!
//! STRICTLY AFTER COMMIT  (WriteOp::finish)
//!  9. RecordedIntent::accept_committed
//! 10. ExternalAttempt::decide -> ExternalExecutionPermit
//! 11. hand the adapter (ExternalExecutionPermit, ResumePermit), by value
//! ```
//!
//! Step 3 is deliberately a byte read and a fresh hash rather than a metadata
//! comparison. [`ManagedConfigVerified::verify`] demands a digest and a length
//! *observed now*, and passing the recorded digest straight back in would prove
//! nothing: the row's `sha256_hex` is unchanged when the file on disk has been
//! rewritten, so a metadata-only check passes for exactly the case the check
//! exists to catch.
//!
//! # Why the permits are not fields
//!
//! [`WorkerSpawnAuthorization`] holds both permits and exposes neither.
//! [`WorkerSpawnAuthorization::spawn_with`] consumes the authorization and
//! hands an adapter `(ExternalExecutionPermit, ResumePermit)` by value. Neither
//! permit is `Clone` or `Copy`, neither has a public constructor,
//! [`ExternalExecutionPermit`] is reachable only from `WriteOp::finish` after
//! `COMMIT`, and [`ResumePermit`] only from
//! [`CommittedLoadout::admit_resume`]. A spawn without a durable intent and a
//! re-proved launch snapshot is therefore not expressible, rather than
//! discouraged.

use governor_artifacts::{ArtifactError, ArtifactStore, StorageKey};
use governor_core::artifact::ArtifactDigest;
use governor_core::effect::{DestinationRef, ExternalExecutionPermit};
use governor_core::error::Conflict;
use governor_core::fence::{DaemonEpoch, SourceRef};
use governor_core::id::{ExternalAttemptId, SessionId, SessionIncarnationId};
use governor_core::session::{
    CommittedLoadout, LoadoutIntegrityError, ManagedConfigDigest, ManagedConfigVerified,
    ResumePermit, WorkerLoadoutFence,
};
use governor_store_sqlite::{
    AuthorizeWorkerSpawnRequest, SessionHealthRequest, SessionLoadoutRecord, Store, StoreError,
};
use sha2::{Digest as _, Sha256};

/// What the daemon is being asked to launch or resume.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResumeWorkerRequest {
    /// Logical session being resumed.
    pub session: SessionId,
    /// The incarnation whose bound loadout is being resumed.
    pub incarnation: SessionIncarnationId,
    /// The exact loadout fence the caller claims to be resuming under.
    pub presented: WorkerLoadoutFence,
    /// Opaque destination the spawn targets.
    pub destination: DestinationRef,
    /// The source fact that justifies the spawn.
    pub source: SourceRef,
    /// Daemon epoch the intent is recorded under.
    pub daemon_epoch: DaemonEpoch,
}

/// Why a resume was refused.
///
/// Every variant means **no process was started and no permit exists**. There
/// is no variant that means "probably fine": an unverifiable launch snapshot is
/// reconciliation work, never a resume under whatever configuration is on disk
/// now.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ResumeRefusal {
    /// The incarnation has no bound launch loadout.
    #[error("session incarnation has no launch loadout to resume")]
    NoLaunchLoadout,

    /// The persisted launch row does not re-derive its own digest.
    ///
    /// A durable `loadout_unverifiable` condition was raised for the session.
    #[error("the persisted launch loadout failed its own integrity check: {0}")]
    LoadoutUnverifiable(LoadoutIntegrityError),

    /// The managed configuration's bytes are missing, truncated or rewritten.
    ///
    /// A durable `managed_config_missing` condition was raised for the session.
    #[error("the managed configuration could not be re-proved: {0}")]
    ManagedConfigUnreadable(ArtifactError),

    /// A fence was rejected: the presented loadout, or the configuration proof.
    #[error("the resume fence was rejected: {0}")]
    Refused(Conflict),

    /// The durable authority refused.
    #[error("the durable store refused: {0}")]
    Store(StoreError),
}

impl From<StoreError> for ResumeRefusal {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Conflict(conflict) => Self::Refused(conflict),
            other => Self::Store(other),
        }
    }
}

/// Permission to create exactly one worker process.
///
/// Holding one means an intent row is committed *and* the launch snapshot was
/// re-proved against bytes the row does not control. Neither permit is
/// reachable except through [`Self::spawn_with`], which consumes both by value.
#[derive(Debug)]
pub struct WorkerSpawnAuthorization {
    attempt: ExternalAttemptId,
    permit: ExternalExecutionPermit,
    resume: ResumePermit,
}

impl WorkerSpawnAuthorization {
    /// The attempt whose intent is durable.
    ///
    /// Needed to record the dispatch fence and the outcome. It is an opaque
    /// identity, not a capability: holding it authorises nothing.
    #[must_use]
    pub const fn attempt(&self) -> ExternalAttemptId {
        self.attempt
    }

    /// The launch snapshot the resume was admitted against.
    #[must_use]
    pub const fn fence(&self) -> WorkerLoadoutFence {
        self.resume.fence()
    }

    /// Hands both permits to a runtime adapter, by value, exactly once.
    ///
    /// This is the adapter-facing spawn seam. There is no accessor for either
    /// permit and no way to clone one, so an adapter that wants to create a
    /// process has to be called from here — which means an intent row is
    /// already committed and a `ManagedConfigVerified` witness was already
    /// produced from freshly read bytes.
    pub fn spawn_with<T>(
        self,
        adapter: impl FnOnce(ExternalExecutionPermit, ResumePermit) -> T,
    ) -> T {
        adapter(self.permit, self.resume)
    }
}

/// Runs the eleven-step authorization for one worker resume.
///
/// # Errors
///
/// Returns a [`ResumeRefusal`]. Every one of them means no process was started
/// and no permit was produced. The two that indicate durable damage —
/// [`ResumeRefusal::LoadoutUnverifiable`] and
/// [`ResumeRefusal::ManagedConfigUnreadable`] — have already raised their
/// session-scoped health condition before returning, so the finding survives
/// the process whether or not anyone is watching.
pub fn authorize_worker_resume(
    store: &Store,
    artifacts: &ArtifactStore,
    request: &ResumeWorkerRequest,
) -> Result<WorkerSpawnAuthorization, ResumeRefusal> {
    let attention = SessionHealthRequest {
        session: request.session,
    };

    // Step 1. The persisted parts, never a re-proved value: proving them is
    // `rehydrate`'s job, and having the loader do it would launder a tampered
    // row into a loadout that agrees with itself.
    let record = store
        .read_session_loadout(request.incarnation)?
        .ok_or(ResumeRefusal::NoLaunchLoadout)?;

    // Step 2. Pure. A refusal here is durable damage to one session.
    let committed = match CommittedLoadout::rehydrate(record.persisted.clone()) {
        Ok(committed) => committed,
        Err(error) => {
            store.raise_loadout_unverifiable(attention)?;
            return Err(ResumeRefusal::LoadoutUnverifiable(error));
        }
    };

    // Steps 3 and 4. The only filesystem read, and the witness minted from it.
    let verified = match verify_config(artifacts, &record) {
        Ok(verified) => verified,
        Err(error) => {
            store.raise_managed_config_missing(attention)?;
            return Err(ResumeRefusal::ManagedConfigUnreadable(error));
        }
    };
    let config = verified.reference();

    // Step 5. Pure, and the only source of a `ResumePermit`.
    let resume = committed
        .admit_resume(request.presented, verified)
        .map_err(ResumeRefusal::Refused)?;

    // Steps 6 to 10, inside one transaction, permit strictly after COMMIT.
    let granted = store.authorize_worker_spawn(AuthorizeWorkerSpawnRequest {
        session: request.session,
        incarnation: request.incarnation,
        verified_loadout: request.presented,
        verified_config: config,
        destination: request.destination.clone(),
        source: request.source.clone(),
        daemon_epoch: request.daemon_epoch,
    })?;

    // Both proofs held this time, so any earlier finding about them is stale.
    // Resolving a condition that is not open is a no-op, so this needs no read.
    store.resolve_loadout_unverifiable(attention)?;
    store.resolve_managed_config_missing(attention)?;

    Ok(WorkerSpawnAuthorization {
        attempt: granted.attempt,
        permit: granted.permit,
        resume,
    })
}

/// Reads one managed configuration's bytes and mints the witness from them.
///
/// The digest is computed over the bytes this call just read. `read_verified`
/// has already refused a mismatch, so the recomputation cannot disagree — but
/// it is the *observation* [`ManagedConfigVerified::verify`] demands, and
/// handing back the recorded digest instead would satisfy the type while
/// proving nothing.
fn verify_config(
    artifacts: &ArtifactStore,
    record: &SessionLoadoutRecord,
) -> Result<ManagedConfigVerified, ArtifactError> {
    let key = StorageKey::new(record.config.storage_ref.clone())?;
    let bytes = artifacts.read_verified(
        &key,
        ArtifactDigest::from_bytes(*record.config.digest.as_bytes()),
        record.config.byte_len,
    )?;
    let observed_digest = ManagedConfigDigest::from_persisted(Sha256::digest(&bytes).into());
    let observed_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    ManagedConfigVerified::verify(record.config.reference(), observed_digest, observed_len).map_err(
        |source| ArtifactError::Integrity {
            key: key.to_string(),
            source,
        },
    )
}
