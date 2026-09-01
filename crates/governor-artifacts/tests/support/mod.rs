//! Deterministic harness for the artifact suites.
//!
//! Real directories on a real filesystem and a real SQLite database. Nothing
//! about modes, `O_NOFOLLOW`, `fsync`, atomic name publication, or the
//! file-before-database ordering can be expressed against a fake, and the
//! forbidden outcome these suites exist to rule out — a committed
//! `completed_unprocessed` obligation referencing an artifact that was never
//! made durable — is a statement about both halves at once.
//!
//! Every port is deterministic, so a scenario replays identically.

// Each integration binary compiles this module and uses a different subset of
// it, and none of it is meant to be reachable outside the test tree.
#![allow(dead_code, unreachable_pub)]

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use governor_artifacts::{
    ArtifactConfig, ArtifactError, ArtifactFailpoint, ArtifactFailpointHook, ArtifactResult,
    ArtifactStore, OpenArtifactStore, PublishRequest, PublishedArtifact, RetentionInput,
    StorageKey, StorageKeySource,
};
use governor_core::artifact::{ArtifactDigest, ResultArtifact, RetentionState};
use governor_core::binding::{ConversationRef, WriteCapabilityState};
use governor_core::fence::{
    BindingGeneration, DeliveryRevision, IncarnationGeneration, ObligationVersion, SafeToken,
    SourceRef,
};
use governor_core::foreman_turn::ProviderMessageRef;
use governor_core::id::{Id, IdKind, IdSource, ObligationId};
use governor_core::random::SecureRandom;
use governor_core::time::{DurationMs, Timestamp};
use governor_core::worker_evidence::{ChildExitStatus, ManagedRunOutcome};
use governor_store_sqlite::{
    ArmDeliverySendRequest, BindForemanRequest, ClaimedDelivery, Clock, CompletionReceipts,
    CreateOrClaimDeliveryRequest, DeliveryOutcome, Failpoint, FailpointHook, OpenStore,
    OpenWorkerTurnRequest, OpenedWorkerTurn, ProjectSpec, PublishWorkerResultRequest,
    PublishedResult, RecordDeliveryOutcomeRequest, RecordWorkerStartedRequest, SessionSpec, Store,
    StoreConfig, StoreError, StorePorts, StoreResult,
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use uuid::Uuid;

/// The instant every simulated process starts its clock at.
pub const DEFAULT_CLOCK_START: i64 = 1_000;

// --- Deterministic ports ----------------------------------------------------

/// A clock that advances one millisecond per reading.
pub struct StepClock(AtomicI64);

impl StepClock {
    pub fn new(start: i64) -> Self {
        Self(AtomicI64::new(start))
    }
}

impl Clock for StepClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

/// A counting identity source.
pub struct CountingIds(AtomicU64);

impl CountingIds {
    pub fn new(start: u64) -> Self {
        Self(AtomicU64::new(start))
    }
}

impl IdSource for CountingIds {
    fn next_uuid(&mut self) -> Uuid {
        Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed)))
    }
}

/// A deterministic byte stream standing in for a CSPRNG.
pub struct StreamRng(AtomicU64);

impl StreamRng {
    pub fn new(seed: u64) -> Self {
        Self(AtomicU64::new(seed))
    }
}

impl SecureRandom for StreamRng {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for slot in dest.iter_mut() {
            let next = self.0.fetch_add(1, Ordering::Relaxed);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a deterministic test stream, not entropy"
            )]
            let byte = next as u8;
            *slot = byte;
        }
    }
}

/// Opaque keys that are reproducible across a scenario.
///
/// A daemon draws these from entropy; a suite that did could not assert which
/// file it expected to find.
pub struct SequentialKeys(u64);

impl SequentialKeys {
    pub fn new(start: u64) -> Self {
        Self(start)
    }
}

impl StorageKeySource for SequentialKeys {
    fn next_key(&mut self) -> StorageKey {
        self.0 += 1;
        StorageKey::parse(&format!("ra-{:08}", self.0)).expect("generated keys are valid")
    }
}

/// A key source that hands out one fixed key, for immutability tests.
pub struct FixedKey(StorageKey);

impl FixedKey {
    pub fn new(key: &str) -> Self {
        Self(StorageKey::parse(key).expect("fixture key"))
    }
}

impl StorageKeySource for FixedKey {
    fn next_key(&mut self) -> StorageKey {
        self.0.clone()
    }
}

// --- Failpoints -------------------------------------------------------------

/// An artifact failpoint that fires once, at one named point.
pub struct ArtifactFireOnce {
    target: ArtifactFailpoint,
    fired: Mutex<bool>,
}

impl ArtifactFireOnce {
    pub fn new(point: ArtifactFailpoint) -> Self {
        Self {
            target: point,
            fired: Mutex::new(false),
        }
    }
}

impl ArtifactFailpointHook for ArtifactFireOnce {
    fn reached(&self, op: &'static str, point: ArtifactFailpoint) -> ArtifactResult<()> {
        if point != self.target {
            return Ok(());
        }
        let mut fired = self.fired.lock().expect("failpoint mutex");
        if *fired {
            return Ok(());
        }
        *fired = true;
        Err(ArtifactError::Injected { op, point })
    }
}

/// A store failpoint that fires once, at one named point of one operation.
pub struct StoreFireOnce {
    target: (&'static str, Failpoint),
    fired: Mutex<bool>,
}

impl StoreFireOnce {
    pub fn new(op: &'static str, point: Failpoint) -> Self {
        Self {
            target: (op, point),
            fired: Mutex::new(false),
        }
    }
}

impl FailpointHook for StoreFireOnce {
    fn reached(&self, op: &'static str, point: Failpoint) -> StoreResult<()> {
        if (op, point) != self.target {
            return Ok(());
        }
        let mut fired = self.fired.lock().expect("failpoint mutex");
        if *fired {
            return Ok(());
        }
        *fired = true;
        Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ABORT),
            Some(format!("injected crash at {op}/{point:?}")),
        )))
    }
}

// --- Harness ----------------------------------------------------------------

/// A throwaway state root holding both halves of the durability contract.
pub struct Harness {
    dir: TempDir,
    opens: AtomicU64,
}

impl Harness {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("temp state root");
        // The hostile-umask suite runs under `umask 0777`, which strips every
        // bit from the temp directory `tempfile` asks for and leaves a root the
        // test itself cannot traverse. The *state root's* mode is the daemon
        // installer's business, not this crate's, so the harness repairs its
        // own scaffolding here and lets the suites assert about the artifact
        // root, which is what `governor-artifacts` actually owns.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .expect("making the temp state root traversable");
        Self {
            dir,
            opens: AtomicU64::new(0),
        }
    }

    pub fn state_root(&self) -> &Path {
        self.dir.path()
    }

    pub fn database_path(&self) -> PathBuf {
        self.dir.path().join("governor.sqlite3")
    }

    pub fn artifact_root(&self) -> PathBuf {
        self.dir.path().join("artifacts")
    }

    /// Opens the SQLite authority, running the full startup sequence.
    pub fn open_store(&self) -> StoreResult<Store> {
        self.open_store_with(DEFAULT_CLOCK_START, None)
    }

    pub fn open_store_with(
        &self,
        start_ms: i64,
        hook: Option<Box<dyn FailpointHook>>,
    ) -> StoreResult<Store> {
        let generation = self.opens.fetch_add(1, Ordering::Relaxed);
        OpenStore {
            config: StoreConfig::new(self.database_path()),
            ports: StorePorts::new(
                Box::new(StepClock::new(start_ms)),
                Box::new(StreamRng::new(1 + generation * 1_000)),
                Box::new(CountingIds::new(1 + generation * 10_000)),
            ),
            failpoints: hook,
            instance_id: Uuid::from_u128(0xC0FFEE),
        }
        .start()
    }

    /// Opens the artifact store with default policy and sequential keys.
    pub fn open_artifacts(&self) -> ArtifactStore {
        self.open_artifacts_with(
            ArtifactConfig::default(),
            Box::new(SequentialKeys::new(0)),
            None,
        )
    }

    pub fn open_artifacts_with(
        &self,
        config: ArtifactConfig,
        keys: Box<dyn StorageKeySource>,
        failpoints: Option<Box<dyn ArtifactFailpointHook>>,
    ) -> ArtifactStore {
        OpenArtifactStore {
            root: self.artifact_root(),
            config,
            keys,
            failpoints,
        }
        .start()
        .expect("opening the artifact root")
    }

    /// A second, read-only connection for assertions about raw rows.
    pub fn inspect(&self) -> Connection {
        Connection::open_with_flags(self.database_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only inspection connection")
    }

    /// Every regular file currently under the artifact root, by directory.
    pub fn files_in(&self, dir: &str) -> Vec<String> {
        let path = self.artifact_root().join(dir);
        let Ok(entries) = std::fs::read_dir(&path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

// --- Domain fixtures --------------------------------------------------------

pub fn token(value: &str) -> SafeToken {
    SafeToken::new(value).expect("fixture tokens are safe")
}

pub fn source(namespace: &str, event: &str, fence: &str) -> SourceRef {
    SourceRef::new(token(namespace), token(event), token(fence))
}

pub fn id<K: IdKind>(value: u128) -> Id<K> {
    Id::from_uuid(Uuid::from_u128(value))
}

pub fn open_turn(store: &Store) -> OpenedWorkerTurn {
    store
        .open_worker_turn(OpenWorkerTurnRequest {
            project: ProjectSpec {
                source_host: token("github.com"),
                source_repo_id: Some(token("R_kgDO")),
                source_repo_display: Some(token("DivMode.commandgovernor")),
            },
            source_issue_ref: Some(token("issue-2")),
            session: SessionSpec {
                runtime_kind: token("herdr"),
                worker_kind: token("claude"),
                display_name: Some(token("phase1-artifacts")),
                runtime_instance_ref: Some(token("pane-3")),
                worker_session_ref: Some(token("sess-9")),
            },
            worker_turn_ref: Some(token("turn-1")),
            priority: 10,
        })
        .expect("opening a worker turn")
}

pub fn start_worker(store: &Store, obligation: ObligationId, run: &str) {
    store
        .record_worker_started(RecordWorkerStartedRequest {
            obligation,
            source: source("claude.init", run, "start"),
            incarnation: IncarnationGeneration::FIRST,
        })
        .expect("recording a verified worker start");
}

pub fn completion_receipts(run: &str) -> CompletionReceipts {
    CompletionReceipts {
        run_ref: token(run),
        final_result_complete: true,
        outcome: ManagedRunOutcome::Success,
        child_exit: ChildExitStatus::Success,
    }
}

/// Binds a foreman conversation and returns its generation.
pub fn bind(store: &Store, conversation: &str) -> BindingGeneration {
    store
        .bind_foreman(BindForemanRequest {
            provider: token("chatgpt"),
            conversation: ConversationRef::new(token(conversation)),
            conversation_url_ref: token(conversation),
            profile: token("cg-profile"),
            connector_abi: token("command-governor-foreman.v1"),
            capability_epoch: 1,
            write_capability: WriteCapabilityState::Proven,
        })
        .expect("binding a verified conversation")
        .generation
}

/// Schedules the first wake revision and claims an attempt.
pub fn schedule_wake(
    store: &Store,
    obligation: ObligationId,
    generation: BindingGeneration,
    version: ObligationVersion,
    fenced_source: SourceRef,
) -> ClaimedDelivery {
    store
        .create_or_claim_delivery(CreateOrClaimDeliveryRequest {
            obligation,
            binding_generation: generation,
            expected_version: version,
            expected_source: fenced_source,
            revision: DeliveryRevision::FIRST,
            attempt_budget: 3,
            wake_protocol: token("composer.v1"),
        })
        .expect("scheduling a wake")
}

/// Arms and accepts a wake: the physical ChatGPT settlement, which is *not*
/// an ACK (`docs/state-machines.md` invariant 14).
pub fn accept_wake(
    store: &Store,
    claimed: &ClaimedDelivery,
    generation: BindingGeneration,
    message: &str,
) {
    store
        .arm_delivery_send(ArmDeliverySendRequest {
            delivery_id: claimed.delivery_id.clone(),
            binding_generation: generation,
            attempt: claimed.attempt,
        })
        .expect("arming the Send fence");
    store
        .record_delivery_outcome(RecordDeliveryOutcomeRequest {
            delivery_id: claimed.delivery_id.clone(),
            attempt: claimed.attempt,
            outcome: DeliveryOutcome::Accepted {
                message: ProviderMessageRef::new(token(message)),
            },
        })
        .expect("recording exact acceptance evidence");
}

/// The bounded final result a fixture worker produced.
pub const FINAL_RESULT: &[u8] = b"# Review\n\nThe change is ready: 3 files, 1 test added.\n";

/// Publishes bytes through the artifact layer.
pub fn publish_bytes(
    artifacts: &mut ArtifactStore,
    bytes: &[u8],
) -> ArtifactResult<PublishedArtifact> {
    artifacts.publish(PublishRequest {
        bytes,
        media_type: token("text.markdown"),
    })
}

/// The whole durable publication: artifact first, then the one transaction.
///
/// This is the composition the daemon performs, and the reason the suites can
/// assert on both halves at once.
pub fn publish_result(
    store: &Store,
    artifacts: &mut ArtifactStore,
    obligation: ObligationId,
    run: &str,
    bytes: &[u8],
) -> Result<(PublishedArtifact, PublishedResult), PublicationFailure> {
    let published = publish_bytes(artifacts, bytes).map_err(PublicationFailure::Artifact)?;
    let committed = store
        .publish_worker_result(PublishWorkerResultRequest {
            obligation,
            source: source("claude.result", run, "final"),
            incarnation: IncarnationGeneration::FIRST,
            receipts: completion_receipts(run),
            artifact: published.durable(),
        })
        .map_err(PublicationFailure::Store)?;
    Ok((published, committed))
}

/// Which half of a publication refused.
#[derive(Debug)]
pub enum PublicationFailure {
    Artifact(ArtifactError),
    Store(StoreError),
}

// --- Durable-state assertions ----------------------------------------------

/// One committed `result_artifacts` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRow {
    pub artifact_id: String,
    pub storage_ref: String,
    pub sha256_hex: String,
    pub byte_len: u64,
    pub retention_state: String,
    /// Latest instant at which an obligation referencing it closed.
    pub released_at: Option<Timestamp>,
}

impl ArtifactRow {
    /// The metadata shape a read verifies against.
    pub fn as_metadata(&self) -> ResultArtifact {
        let mut digest = [0u8; 32];
        for (slot, pair) in digest.iter_mut().zip(self.sha256_hex.as_bytes().chunks(2)) {
            let text = std::str::from_utf8(pair).expect("hex is ascii");
            *slot = u8::from_str_radix(text, 16).expect("hex digit pair");
        }
        ResultArtifact::new(
            id(0),
            token(&self.storage_ref),
            ArtifactDigest::from_bytes(digest),
            self.byte_len,
            Timestamp::from_unix_millis(0),
        )
    }

    pub fn retention(&self) -> RetentionState {
        match self.retention_state.as_str() {
            "pinned" => RetentionState::Pinned,
            "eligible" => RetentionState::Eligible,
            other => panic!("unknown retention label {other}"),
        }
    }

    pub fn as_retention_input(&self) -> RetentionInput {
        RetentionInput {
            key: StorageKey::parse(&self.storage_ref).expect("committed keys are valid"),
            state: self.retention(),
            released_at: self.released_at,
        }
    }
}

/// Every committed artifact row, with the release instant a sweep needs.
///
/// The release instant is derived here rather than read from
/// `eligible_for_delete_at_ms`, which the store leaves `NULL` — see the
/// deviation note in `governor_artifacts::gc`.
pub fn artifact_rows(conn: &Connection) -> Vec<ArtifactRow> {
    let mut statement = conn
        .prepare(
            "SELECT ra.result_artifact_id, ra.storage_ref, ra.sha256_hex, ra.byte_len,
                    ra.retention_state,
                    (SELECT MAX(e.observed_at_ms)
                       FROM obligations o JOIN events e ON e.seq = o.closed_event_seq
                      WHERE o.result_artifact_id = ra.result_artifact_id)
               FROM result_artifacts ra
              ORDER BY ra.storage_ref",
        )
        .expect("preparing the artifact query");
    let rows = statement
        .query_map([], |row| {
            Ok(ArtifactRow {
                artifact_id: row.get(0)?,
                storage_ref: row.get(1)?,
                sha256_hex: row.get(2)?,
                byte_len: row.get::<_, i64>(3)?.try_into().expect("non-negative"),
                retention_state: row.get(4)?,
                released_at: row
                    .get::<_, Option<i64>>(5)?
                    .map(Timestamp::from_unix_millis),
            })
        })
        .expect("querying artifacts");
    rows.map(|row| row.expect("artifact row")).collect()
}

/// Every `storage_ref` the durable authority has committed.
pub fn committed_keys(conn: &Connection) -> BTreeSet<StorageKey> {
    artifact_rows(conn)
        .into_iter()
        .map(|row| StorageKey::parse(&row.storage_ref).expect("committed keys are valid"))
        .collect()
}

/// Obligation state labels, by obligation id.
pub fn obligation_states(conn: &Connection) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare("SELECT obligation_id, state FROM obligations ORDER BY obligation_id")
        .expect("preparing the obligation query");
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("querying obligations");
    rows.map(|row| row.expect("obligation row")).collect()
}

/// Storage refs referenced by an obligation in `completed_unprocessed`.
pub fn completed_unprocessed_refs(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare(
            "SELECT ra.storage_ref
               FROM obligations o JOIN result_artifacts ra
                 ON ra.result_artifact_id = o.result_artifact_id
              WHERE o.state = 'completed_unprocessed'
              ORDER BY ra.storage_ref",
        )
        .expect("preparing the completion query");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("querying completions");
    rows.map(|row| row.expect("completion row")).collect()
}

/// **The forbidden outcome.**
///
/// `docs/testing.md` ART-001: no committed `completed_unprocessed` obligation
/// may reference a missing or non-durable result artifact. Every crash-matrix
/// cell ends here.
pub fn assert_no_completion_without_durable_bytes(harness: &Harness, context: &str) {
    let conn = harness.inspect();
    let artifacts = harness.open_artifacts();
    let rows = artifact_rows(&conn);
    for storage_ref in completed_unprocessed_refs(&conn) {
        let row = rows
            .iter()
            .find(|row| row.storage_ref == storage_ref)
            .unwrap_or_else(|| panic!("{context}: completion references an unknown artifact row"));
        let bytes = artifacts.read(&row.as_metadata()).unwrap_or_else(|error| {
            panic!(
                "{context}: completed_unprocessed references {storage_ref}, \
                 which is not durable and verifiable: {error}"
            )
        });
        assert_eq!(
            u64::try_from(bytes.len()).expect("length fits"),
            row.byte_len,
            "{context}: stored length disagrees with the committed row"
        );
    }
}

/// Convenience for a grace-free policy in sweep tests.
pub fn config_with_grace(orphan: u64, retention: u64) -> ArtifactConfig {
    ArtifactConfig {
        orphan_grace: DurationMs::from_millis(orphan),
        retention_grace: DurationMs::from_millis(retention),
        ..ArtifactConfig::default()
    }
}
