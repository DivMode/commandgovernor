//! Startup, in the order `docs/architecture.md` fixes, and the serve loop.
//!
//! # The order, and what implements each step
//!
//! `docs/architecture.md` "Startup recovery order" lists thirteen steps. Phase 1
//! has no hook inbox, no worker-host, no runtime adapter, no watchdog schedule
//! and no live browser or MCP connector, so steps 6, 7, 9, 10 and 11–13 have
//! nothing to run yet. The applicable subset runs in order, and no later step
//! begins until the earlier ones have passed:
//!
//! | Step | Architecture | Here |
//! | --- | --- | --- |
//! | 1 | acquire single-daemon state-root lock | [`crate::lock::InstanceLock::acquire`] |
//! | 2 | validate filesystem ownership/permissions | [`crate::layout`] |
//! | 3 | open SQLite, verify schema/integrity/migrations | [`OpenStore::start`] |
//! | 4 | quarantine orphaned external effects | `OpenStore::start`, `RecoverStartup` |
//! | 5 | replay/validate projections | `OpenStore::start`, `verify_projections` |
//! | 8 | verify artifacts required by open obligations | [`verify_pinned_artifacts`] |
//! | — | quarantine unreferenced artifacts | [`governor_artifacts::ArtifactStore::scan_orphans`] |
//! | ready | bind the owner-local socket | [`crate::ipc::IpcServer::bind`] |
//!
//! Steps 1 and 2 are this crate's; 3 to 5 are one atomic unit inside
//! [`OpenStore::start`], which cannot be entered part-way — there is no
//! constructor for a `Store` that skips them.
//!
//! **Deviation, recorded rather than hidden.** The architecture lists quarantine
//! (step 4) before projection replay (step 5); the store runs replay first, on
//! the stated ground that a store which cannot prove its own projections must
//! not go on to make external decisions from them. Both orders satisfy the
//! binding requirement — quarantine completes before *any* new external I/O is
//! scheduled — and the daemon inherits the store's order because both happen
//! inside one call it cannot interleave with.
//!
//! # Missing evidence never becomes success
//!
//! Every step returns a typed refusal rather than a warning, and a refusal
//! means the daemon did not become ready and scheduled nothing.
//!
//! # What is scoped, and what is fatal
//!
//! A hard startup refusal is reserved for damage the *state root* has, where
//! nothing the daemon could serve would be trustworthy: the instance lock, the
//! schema epoch, a drifted migration, a projection that disagrees with its
//! ledger, an unusable artifact root, filesystem ownership, and the control
//! socket.
//!
//! An artifact an open obligation pins that cannot be verified is not that. It
//! is damage to *one obligation*, and refusing the whole daemon over it takes
//! every unrelated obligation down with it. So step 8 scopes the failure to
//! where it happened: [`verify_pinned_artifacts`] raises the durable
//! `result_artifact_missing` condition for that obligation, records a safe
//! diagnostic, and the daemon goes on to serve. `status` and `doctor` both
//! report the condition, and the affected obligation stays open and
//! unprocessable — not by a flag anyone has to remember to check, but because
//! `ArtifactStore::read` verifies the digest and length on every read and hands
//! back **no bytes at all** on a mismatch (`docs/testing.md` ART-003, DB-008).
//! Nothing can hand that result to a foreman for review.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use governor_artifacts::{ArtifactConfig, ArtifactStore, OpenArtifactStore, StorageKey};
use governor_core::fence::DaemonEpoch;
use governor_core::lease::ProcessIncarnation;
use governor_core::time::Timestamp;
use governor_store_sqlite::{
    OpenCondition, OpenObligation, OpenStore, ResultArtifactMissingRequest, StartupRecovery, Store,
    StoreConfig, VerifiedProjections,
};

use crate::error::{DaemonError, ReclaimedLock};
use crate::ipc::{self, IpcServer, Request};
use crate::layout::{self, PathClass, StateRoot};
use crate::lock::InstanceLock;
use crate::logging::{Fields, Level, SafeLog};
use crate::ports::{self, SystemClock, Uuidv7Keys};
use crate::report;
use governor_store_sqlite::Clock as _;

/// How to bring a daemon up against one state root.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DaemonConfig {
    /// Where the durable state lives.
    pub state_root: StateRoot,
    /// Result-artifact policy.
    pub artifacts: ArtifactConfig,
    /// Bounded SQLite busy timeout, in milliseconds.
    pub busy_timeout_ms: u32,
}

impl DaemonConfig {
    /// The default policy for a state root.
    #[must_use]
    pub fn new(state_root: StateRoot) -> Self {
        Self {
            state_root,
            artifacts: ArtifactConfig::default(),
            busy_timeout_ms: governor_store_sqlite::DEFAULT_BUSY_TIMEOUT_MS,
        }
    }
}

/// What startup proved before the daemon agreed to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadyReport {
    /// The epoch this process advanced the database to.
    pub daemon_epoch: DaemonEpoch,
    /// This process's incarnation, as recorded in the instance lock.
    pub incarnation: ProcessIncarnation,
    /// The stale lock this start reclaimed, if any.
    pub reclaimed_lock: Option<ReclaimedLock>,
    /// Schema epoch after migration.
    pub schema_epoch: u32,
    /// Migrations applied during this open.
    pub migrations_applied: usize,
    /// Projection replay equivalence, proven before anything was scheduled.
    pub projections: VerifiedProjections,
    /// What startup quarantine found.
    pub recovery: StartupRecovery,
    /// Artifacts pinned by an open obligation that verified.
    pub artifacts_verified: usize,
    /// Artifacts pinned by an open obligation that did **not** verify.
    ///
    /// Each one left an open `result_artifact_missing` condition naming its
    /// obligation. The daemon still serves: the damage is scoped to those
    /// obligations, which cannot deliver a result because the artifact store
    /// refuses to return bytes that fail their digest.
    pub artifacts_unverified: usize,
    /// Unreferenced artifact files moved aside by the orphan sweep.
    pub artifacts_quarantined: usize,
}

/// A running Command Governor daemon.
///
/// Holding one is holding the state root's authority: the instance lock lives
/// inside it, and dropping it releases the lock and removes the socket.
#[derive(Debug)]
pub struct Daemon {
    config: DaemonConfig,
    _lock: InstanceLock,
    log: SafeLog,
    store: Store,
    artifacts: ArtifactStore,
    server: IpcServer,
    ready: ReadyReport,
}

impl Daemon {
    /// Runs the applicable startup order and binds the control socket.
    ///
    /// # Errors
    ///
    /// Any [`DaemonError`]. Every one of them means the daemon did not become
    /// ready and scheduled nothing.
    pub fn start(config: DaemonConfig) -> Result<Self, DaemonError> {
        let root = config.state_root.clone();

        // Step 1. The lock comes first, so nothing below can run twice against
        // one state root. Creating the root directory is its only precondition;
        // everything else about the filesystem is checked in step 2, under the
        // lock.
        layout::ensure_private_dir(root.path()).map_err(|defect| DaemonError::Filesystem {
            class: PathClass::StateRoot,
            defect,
        })?;
        let incarnation = crate::incarnation::current();
        let lock = InstanceLock::acquire(&root.lock_path(), incarnation.clone())?;

        // Step 2. Ownership and permissions, on every directory and on the lock
        // file itself.
        let owner_uid = validate_filesystem(&root)?;

        // The control socket's address must fit the platform's socket address
        // buffer. Binding happens last, so without this preflight an
        // impossible pathname would advance the daemon epoch and run recovery
        // before failing on a condition that was knowable up front.
        ipc::check_socket_path(&root.socket_path())?;

        let log = SafeLog::open(&root.log_root()).map_err(|_| DaemonError::Logging)?;
        let clock = SystemClock;
        log.record(
            clock.now(),
            Level::Info,
            "daemon.starting",
            &Fields::new()
                .int("slot", i64::from(incarnation.slot().get()))
                .token("start", incarnation.start().as_token())
                .flag("reclaimed_lock", lock.reclaimed().is_some()),
        );

        // Steps 3, 4 and 5: schema epoch gate, migrations, daemon-epoch
        // advance, projection replay equivalence, startup quarantine. One call,
        // no way in part-way.
        let store = OpenStore {
            config: StoreConfig::new(root.database_path())
                .with_busy_timeout_ms(config.busy_timeout_ms),
            ports: ports::production_ports(),
            failpoints: None,
            instance_id: uuid::Uuid::now_v7(),
        }
        .start()?;
        let startup = store.startup().clone();

        // SQLite creates its file and sidecars under the host umask, which on a
        // default macOS or Linux account is `0644`. The owner-only state root
        // already keeps other principals out, but the durable authority should
        // not be the one file in the layout that relies on its parent for that,
        // so the mode is forced and then checked like everything else. It can
        // only happen here: the files do not exist until the store opens them,
        // and `governor-store-sqlite` performs no filesystem operations at all
        // by design.
        harden_database_files(&root, owner_uid)?;

        log.record(
            clock.now(),
            Level::Info,
            "store.opened",
            &Fields::new()
                .int("daemon_epoch", to_i64(startup.daemon_epoch.get()))
                .int("schema_epoch", i64::from(startup.migrations.epoch))
                .count("migrations_applied", startup.migrations.applied.len())
                .count("projections.obligations", startup.projections.obligations)
                .count("projections.deliveries", startup.projections.deliveries)
                .count(
                    "quarantined_deliveries",
                    startup.recovery.quarantined_deliveries,
                )
                .count("uncertain_mutations", startup.recovery.uncertain_mutations)
                .count("ambiguous_attempts", startup.recovery.ambiguous_attempts),
        );

        let artifacts = OpenArtifactStore {
            root: root.artifact_root(),
            config: config.artifacts,
            keys: Box::new(Uuidv7Keys),
            failpoints: None,
        }
        .start()?;

        // Step 8. Every artifact an open obligation pins must actually be
        // there, and be the bytes the ledger recorded. One that is not scopes
        // its own obligation out of service; it does not stop the daemon.
        let pinned = verify_pinned_artifacts(&store, &artifacts, clock.now(), &log)?;
        if pinned.unverified > 0 {
            log.record(
                clock.now(),
                Level::Warn,
                "artifacts.serving_with_unverified_pins",
                &Fields::new()
                    .count("unverified", pinned.unverified)
                    .count("verified", pinned.verified),
            );
        }

        // Unreferenced files are set aside, never deleted: a publication that
        // crashed and one that is merely slow look identical. The reference
        // set must be complete before any file is reclassified: a committed
        // row whose storage_ref does not parse would silently drop out of the
        // set and let the sweep quarantine bytes the ledger still references,
        // so an unparseable reference is a corrupt-value refusal instead.
        let mut committed = BTreeSet::new();
        for artifact in store.list_committed_artifacts()? {
            let key = StorageKey::new(artifact.storage_ref().clone()).map_err(|_| {
                governor_store_sqlite::StoreError::from(governor_store_sqlite::CorruptValue::new(
                    "result_artifacts",
                    "storage_ref",
                    governor_store_sqlite::CorruptReason::MalformedIdentity,
                ))
            })?;
            committed.insert(key);
        }
        let scan = artifacts.scan_orphans(&committed, clock.now())?;
        if !scan.quarantined.is_empty() {
            log.record(
                clock.now(),
                Level::Warn,
                "artifacts.orphans_quarantined",
                &Fields::new()
                    .count("quarantined", scan.quarantined.len())
                    .count("within_grace", scan.within_grace.len())
                    .count("referenced", scan.referenced),
            );
        }

        let server = IpcServer::bind(&root)?;
        // The socket is created by `bind`, so its ownership can only be checked
        // now. A socket another principal could reach is a refusal, not a note.
        if let Some((what, defect)) = ipc::audit(&root, owner_uid).into_iter().next() {
            log.record(
                clock.now(),
                Level::Error,
                "ipc.audit_failed",
                &Fields::new().class("path", what),
            );
            return Err(DaemonError::Filesystem {
                class: PathClass::Ipc,
                defect,
            });
        }

        let ready = ReadyReport {
            daemon_epoch: startup.daemon_epoch,
            incarnation,
            reclaimed_lock: lock.reclaimed().copied(),
            schema_epoch: startup.migrations.epoch,
            migrations_applied: startup.migrations.applied.len(),
            projections: startup.projections,
            recovery: startup.recovery,
            artifacts_verified: pinned.verified,
            artifacts_unverified: pinned.unverified,
            artifacts_quarantined: scan.quarantined.len(),
        };

        log.record(
            clock.now(),
            Level::Info,
            "daemon.ready",
            &Fields::new()
                .count("artifacts_verified", ready.artifacts_verified)
                .count("artifacts_unverified", ready.artifacts_unverified)
                .count("artifacts_quarantined", ready.artifacts_quarantined),
        );

        Ok(Self {
            config,
            _lock: lock,
            log,
            store,
            artifacts,
            server,
            ready,
        })
    }

    /// What startup proved.
    #[must_use]
    pub const fn ready(&self) -> &ReadyReport {
        &self.ready
    }

    /// The state root this daemon owns.
    #[must_use]
    pub const fn state_root(&self) -> &StateRoot {
        &self.config.state_root
    }

    /// The durable authority.
    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    /// The result-artifact root.
    #[must_use]
    pub const fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// Serves the control socket until a shutdown signal arrives.
    ///
    /// `SIGINT` and `SIGTERM` set the flag; the accept loop notices within one
    /// poll interval, returns, and the drop order then releases the socket and
    /// the instance lock.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Logging`] when the signal handlers cannot be
    /// installed, which would leave the daemon unstoppable except by `SIGKILL`.
    pub fn run(&self) -> Result<(), DaemonError> {
        let stop = Arc::new(AtomicBool::new(false));
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            signal_hook::flag::register(signal, Arc::clone(&stop))
                .map_err(|_| DaemonError::SignalHandler)?;
        }
        self.serve_until(&stop);
        Ok(())
    }

    /// Serves until `stop` is set, without installing signal handlers.
    ///
    /// The seam the acceptance suite drives: a test needs the serve loop, not a
    /// process-wide handler.
    pub fn serve_until(&self, stop: &AtomicBool) {
        let clock = SystemClock;
        self.log
            .record(clock.now(), Level::Info, "daemon.serving", &Fields::new());
        self.server.serve(stop, &|request| self.answer(request));
        self.log
            .record(clock.now(), Level::Info, "daemon.stopping", &Fields::new());
    }

    /// Answers one control request.
    fn answer(&self, request: Request) -> Vec<String> {
        match request {
            Request::Ping => Vec::new(),
            Request::Status => self.status_lines(),
            Request::Obligations => match self.store.list_open_obligations() {
                Ok(open) => report::obligation_lines(&open, SystemClock.now()),
                Err(error) => vec![format!("error class={}", store_error_class(&error))],
            },
            Request::Doctor => self.daemon_doctor_lines(),
        }
    }

    fn status_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "daemon.state=running".to_owned(),
            format!("daemon.slot={}", self.ready.incarnation.slot()),
            format!("daemon.epoch={}", self.ready.daemon_epoch.get()),
            format!("store.schema_epoch={}", self.ready.schema_epoch),
            format!(
                "store.projections.obligations={}",
                self.ready.projections.obligations
            ),
            format!(
                "store.projections.deliveries={}",
                self.ready.projections.deliveries
            ),
            format!(
                "store.projections.verified_through={}",
                self.ready
                    .projections
                    .verified_through
                    .map_or(0, |seq| seq.get())
            ),
            format!(
                "recovery.quarantined_deliveries={}",
                self.ready.recovery.quarantined_deliveries
            ),
            format!(
                "recovery.uncertain_mutations={}",
                self.ready.recovery.uncertain_mutations
            ),
            format!(
                "recovery.ambiguous_attempts={}",
                self.ready.recovery.ambiguous_attempts
            ),
            format!("artifacts.verified={}", self.ready.artifacts_verified),
            format!("artifacts.unverified={}", self.ready.artifacts_unverified),
            format!("artifacts.quarantined={}", self.ready.artifacts_quarantined),
        ];

        match self.store.list_open_obligations() {
            Ok(open) => lines.extend(report::obligation_summary_lines(&open)),
            Err(error) => lines.push(format!("error class={}", store_error_class(&error))),
        }
        match self.store.open_health_conditions() {
            Ok(conditions) => lines.extend(report::health_lines(&conditions)),
            Err(error) => lines.push(format!("error class={}", store_error_class(&error))),
        }
        lines
    }

    fn daemon_doctor_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("daemon.slot={}", self.ready.incarnation.slot()),
            format!("daemon.epoch={}", self.ready.daemon_epoch.get()),
            format!(
                "daemon.reclaimed_stale_lock={}",
                self.ready.reclaimed_lock.is_some()
            ),
            format!("store.schema_epoch={}", self.ready.schema_epoch),
            format!("store.migrations_applied={}", self.ready.migrations_applied),
            format!(
                "store.projections.verified_through={}",
                self.ready
                    .projections
                    .verified_through
                    .map_or(0, |seq| seq.get())
            ),
            format!("artifacts.verified={}", self.ready.artifacts_verified),
            format!("artifacts.unverified={}", self.ready.artifacts_unverified),
        ];
        // Attention the *running* authority holds, next to the offline
        // diagnosis the same command already renders from the database. An
        // obligation whose pinned artifact did not verify is visible in both.
        match self.store.open_health_conditions() {
            Ok(conditions) => lines.extend(report::health_lines(&conditions)),
            Err(error) => lines.push(format!("error class={}", store_error_class(&error))),
        }
        lines
    }
}

/// Step 2 of the startup order, over every path the daemon owns.
fn validate_filesystem(root: &StateRoot) -> Result<u32, DaemonError> {
    let owner_uid =
        layout::effective_uid(root.path()).map_err(|defect| DaemonError::Filesystem {
            class: PathClass::StateRoot,
            defect,
        })?;

    for &class in PathClass::ALL {
        let path = root.directory(class);
        layout::ensure_private_dir(&path)
            .map_err(|defect| DaemonError::Filesystem { class, defect })?;
        layout::audit_dir(&path, owner_uid)
            .map_err(|defect| DaemonError::Filesystem { class, defect })?;
    }

    // The lock this process is holding must itself be a private file this user
    // owns; otherwise the election it decided was between the wrong parties.
    layout::audit_file(&root.lock_path(), owner_uid).map_err(|defect| DaemonError::Filesystem {
        class: PathClass::StateRoot,
        defect,
    })?;
    Ok(owner_uid)
}

/// Forces the database and its sidecars owner-only, then audits them.
fn harden_database_files(root: &StateRoot, owner_uid: u32) -> Result<(), DaemonError> {
    for path in root.database_files() {
        if !path.exists() {
            continue;
        }
        let refuse = |defect| DaemonError::Filesystem {
            class: PathClass::StateRoot,
            defect,
        };
        layout::make_file_private(&path).map_err(refuse)?;
        layout::audit_file(&path, owner_uid).map_err(refuse)?;
    }
    Ok(())
}

/// What step 8 could and could not prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PinnedArtifacts {
    verified: usize,
    unverified: usize,
}

/// Step 8: prove the bytes behind every artifact an open obligation pins.
///
/// A verified artifact resolves any open `result_artifact_missing` condition
/// for its obligation; an unverifiable one raises it. Both are durable, so the
/// finding survives the process whether or not anyone is watching.
///
/// A failure here is **scoped to its obligation** and does not stop the daemon:
/// see the module docs. The obligation it names cannot deliver its result
/// regardless, because the artifact store refuses to return bytes that fail
/// their recorded digest and length.
///
/// # Errors
///
/// A [`DaemonError::Store`] when the durable condition cannot be recorded —
/// that *is* structural, because an unrecorded finding is an invisible one.
fn verify_pinned_artifacts(
    store: &Store,
    artifacts: &ArtifactStore,
    now: Timestamp,
    log: &SafeLog,
) -> Result<PinnedArtifacts, DaemonError> {
    let open = store.list_open_obligations()?;
    let mut verified = 0_usize;
    let mut missing = 0_usize;

    for obligation in &open {
        let Some(artifact) = &obligation.result_artifact else {
            continue;
        };
        let request = ResultArtifactMissingRequest {
            obligation: obligation.id,
            artifact: artifact.id(),
        };
        match artifacts.read(artifact) {
            Ok(_) => {
                verified += 1;
                // Idempotent: resolving a condition that is not open is a
                // no-op, so this needs no prior read.
                store.resolve_result_artifact_missing(request)?;
            }
            Err(error) => {
                missing += 1;
                log.record(
                    now,
                    Level::Error,
                    "artifacts.verification_failed",
                    &Fields::new()
                        .id("obligation", obligation.id)
                        .id("artifact", artifact.id())
                        .class("reason", artifact_failure_class(&error)),
                );
                store.raise_result_artifact_missing(request)?;
            }
        }
    }

    Ok(PinnedArtifacts {
        verified,
        unverified: missing,
    })
}

/// A stable class for an artifact-layer failure, for logs and status lines.
const fn artifact_failure_class(error: &governor_artifacts::ArtifactError) -> &'static str {
    use governor_artifacts::ArtifactError;
    match error {
        ArtifactError::Missing { .. } => "missing",
        ArtifactError::Integrity { .. } => "integrity_mismatch",
        ArtifactError::TooLarge { .. } => "too_large",
        ArtifactError::UnsafePath { .. } => "unsafe_path",
        ArtifactError::InvalidKey(_) => "invalid_key",
        _ => "io",
    }
}

/// A stable class for a store failure, for a reply line.
const fn store_error_class(error: &governor_store_sqlite::StoreError) -> &'static str {
    use governor_store_sqlite::StoreError;
    match error {
        StoreError::Conflict(_) => "conflict",
        StoreError::SchemaEpochTooNew { .. } => "schema_epoch_too_new",
        StoreError::MigrationChecksumMismatch { .. } => "migration_checksum_mismatch",
        StoreError::UnknownAppliedMigration { .. } => "unknown_applied_migration",
        StoreError::ConnectionPolicy(_) => "connection_policy",
        StoreError::Corrupt(_) => "corrupt_value",
        StoreError::RepairNeeded(_) => "repair_needed",
        StoreError::QuarantineIncomplete { .. } => "quarantine_incomplete",
        StoreError::WriterGone => "writer_gone",
        StoreError::Sqlite(_) => "sqlite",
        _ => "unclassified",
    }
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Counts open obligations and conditions without a running daemon.
///
/// Used by `doctor`; see [`crate::doctor`].
pub(crate) fn summarise(
    obligations: &[OpenObligation],
    conditions: &[OpenCondition],
) -> Vec<String> {
    let mut lines = report::obligation_summary_lines(obligations);
    lines.extend(report::health_lines(conditions));
    lines
}
