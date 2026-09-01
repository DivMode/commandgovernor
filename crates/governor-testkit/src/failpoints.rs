//! The two crash seams, and the hooks a matrix drives them with.
//!
//! `governor-store-sqlite` exposes [`Failpoint`] inside every write
//! transaction, and `governor-artifacts` exposes [`ArtifactFailpoint`] at every
//! step of crash-safe publication. Both traits are shaped the same way on
//! purpose, so one matrix can walk both halves of a publication.
//!
//! A hook here is shared rather than owned: the store and the artifact store
//! each take a `Box<dyn …Hook>`, and the test keeps a clone so it can ask
//! afterwards whether the point was actually reached. A cell whose failpoint
//! never fired is not a failed injection, it is a real answer — that operation
//! does not pass through that point — and the suites assert on the outcome
//! either way.
//!
//! # Enumerating the points
//!
//! [`ArtifactFailpoint::ALL`] is supplied by the artifact crate. [`Failpoint`]
//! is `#[non_exhaustive]` and carries no such list, so [`STORE_FAILPOINTS`]
//! is maintained here. A variant added upstream will not appear in it
//! automatically; [`store_failpoint_label`] fails closed on an unknown variant
//! so the omission surfaces as a panic in the first matrix that reaches it
//! rather than as a silently smaller matrix.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use governor_artifacts::{ArtifactError, ArtifactFailpoint, ArtifactFailpointHook, ArtifactResult};
use governor_store_sqlite::{Failpoint, FailpointHook, StoreError, StoreResult};

/// Every store failpoint reachable inside a write transaction.
///
/// `BeforeMigrationRecorded` is deliberately absent: it is only reachable while
/// a migration runs, which is not a write operation a caller can invoke. See
/// [`MIGRATION_FAILPOINTS`].
pub const TRANSACTION_FAILPOINTS: &[Failpoint] = &[
    Failpoint::AfterEventAppend,
    Failpoint::AfterProjectionUpdate,
    Failpoint::BeforeCommit,
    Failpoint::AfterIntentInsert,
    Failpoint::AfterMutationReceived,
    Failpoint::AfterMutationResult,
];

/// Store failpoints reachable only while a migration runs.
pub const MIGRATION_FAILPOINTS: &[Failpoint] =
    &[Failpoint::BeforeMigrationRecorded, Failpoint::BeforeCommit];

/// Every store failpoint this testkit knows about.
pub const STORE_FAILPOINTS: &[Failpoint] = &[
    Failpoint::AfterEventAppend,
    Failpoint::AfterProjectionUpdate,
    Failpoint::BeforeCommit,
    Failpoint::BeforeMigrationRecorded,
    Failpoint::AfterIntentInsert,
    Failpoint::AfterMutationReceived,
    Failpoint::AfterMutationResult,
];

/// A stable label for one store failpoint.
///
/// # Panics
///
/// Panics on a variant this testkit does not know, which is the signal to add
/// it to [`STORE_FAILPOINTS`]. Failing loudly is the point: a matrix that
/// silently skipped a new crash window would claim coverage it does not have.
#[must_use]
pub fn store_failpoint_label(point: Failpoint) -> &'static str {
    match point {
        Failpoint::AfterEventAppend => "after_event_append",
        Failpoint::AfterProjectionUpdate => "after_projection_update",
        Failpoint::BeforeCommit => "before_commit",
        Failpoint::BeforeMigrationRecorded => "before_migration_recorded",
        Failpoint::AfterIntentInsert => "after_intent_insert",
        Failpoint::AfterMutationReceived => "after_mutation_received",
        Failpoint::AfterMutationResult => "after_mutation_result",
        unknown => panic!("unknown store failpoint {unknown:?}: add it to STORE_FAILPOINTS"),
    }
}

/// Every named write operation the store exposes.
///
/// The names are [`WriteOp::NAME`](governor_store_sqlite) values, which is what
/// a failpoint hook is told. `docs/testing.md` DB-002 asks for a crash around
/// *each* multi-row transition, so the matrix iterates this list rather than a
/// hand-picked subset.
pub const WRITE_OPERATIONS: &[&str] = &[
    "open_worker_turn",
    "bind_foreman",
    "record_worker_started",
    "record_worker_failure",
    "publish_worker_result",
    "cancel_obligation",
    "create_or_claim_delivery",
    "arm_delivery_send",
    "record_delivery_outcome",
    "mint_foreman_claim",
    "deliver_handoff",
    "acknowledge_obligation",
    "expire_foreman_claim",
    "begin_mutation",
    "complete_mutation",
    "ack_mutation_receipt",
    "record_external_intent",
    "mark_external_dispatched",
    "record_external_outcome",
    "acquire_lease",
    "renew_lease",
    "release_lease",
    "recover_startup",
    "record_managed_config",
    "resolve_worker_loadout",
    "bind_session_loadout",
    "record_session_lineage",
    "authorize_worker_spawn",
    "raise_loadout_unverifiable",
    "resolve_loadout_unverifiable",
    "raise_managed_config_missing",
    "resolve_managed_config_missing",
    "raise_lineage_broken",
    "resolve_lineage_broken",
];

/// A store failpoint that aborts one transaction, once.
///
/// Aborting inside the body rolls the transaction back, which is exactly what a
/// process death before `COMMIT` looks like to the next process.
#[derive(Debug, Clone)]
pub struct StoreCrash {
    target: (&'static str, Failpoint),
    fired: Arc<AtomicBool>,
    announced: Arc<AtomicUsize>,
}

impl StoreCrash {
    /// Arms a crash at one point of one operation.
    #[must_use]
    pub fn at(op: &'static str, point: Failpoint) -> Self {
        Self {
            target: (op, point),
            fired: Arc::new(AtomicBool::new(false)),
            announced: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Whether the injected failure actually fired.
    #[must_use]
    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::Relaxed)
    }

    /// How many times any point of any operation was announced.
    #[must_use]
    pub fn announcements(&self) -> usize {
        self.announced.load(Ordering::Relaxed)
    }

    /// The boxed form the store takes at open time.
    #[must_use]
    pub fn boxed(&self) -> Box<dyn FailpointHook> {
        Box::new(self.clone())
    }
}

impl FailpointHook for StoreCrash {
    fn reached(&self, op: &'static str, point: Failpoint) -> StoreResult<()> {
        self.announced.fetch_add(1, Ordering::Relaxed);
        if (op, point) != self.target || self.fired.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ABORT),
            Some(format!(
                "injected crash at {op}/{}",
                store_failpoint_label(point)
            )),
        )))
    }
}

/// An artifact failpoint that aborts one publication, once.
#[derive(Debug, Clone)]
pub struct ArtifactCrash {
    target: ArtifactFailpoint,
    fired: Arc<AtomicBool>,
}

impl ArtifactCrash {
    /// Arms a crash at one publication step.
    #[must_use]
    pub fn at(point: ArtifactFailpoint) -> Self {
        Self {
            target: point,
            fired: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the injected failure actually fired.
    #[must_use]
    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::Relaxed)
    }

    /// The boxed form the artifact store takes at open time.
    #[must_use]
    pub fn boxed(&self) -> Box<dyn ArtifactFailpointHook> {
        Box::new(self.clone())
    }
}

impl ArtifactFailpointHook for ArtifactCrash {
    fn reached(&self, op: &'static str, point: ArtifactFailpoint) -> ArtifactResult<()> {
        if point != self.target || self.fired.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        Err(ArtifactError::Injected { op, point })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_crash_fires_once_at_its_exact_target() {
        let crash = StoreCrash::at("publish_worker_result", Failpoint::BeforeCommit);
        let announce = |op, point| FailpointHook::reached(&crash, op, point);

        assert!(
            announce("publish_worker_result", Failpoint::AfterEventAppend).is_ok(),
            "a different point is inert"
        );
        assert!(
            announce("bind_foreman", Failpoint::BeforeCommit).is_ok(),
            "a different operation is inert"
        );
        assert!(announce("publish_worker_result", Failpoint::BeforeCommit).is_err());
        assert!(crash.fired());
        assert!(
            announce("publish_worker_result", Failpoint::BeforeCommit).is_ok(),
            "and it never fires twice"
        );
        assert_eq!(crash.announcements(), 4);
    }

    #[test]
    fn every_known_point_has_a_label() {
        for point in STORE_FAILPOINTS {
            assert!(!store_failpoint_label(*point).is_empty());
        }
        for point in ArtifactFailpoint::ALL {
            assert!(!point.as_str().is_empty());
        }
    }
}
