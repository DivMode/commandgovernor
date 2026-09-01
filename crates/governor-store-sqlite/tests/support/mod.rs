//! Deterministic harness for the store suites.
//!
//! Real SQLite files in throwaway directories: `:memory:` cannot express WAL,
//! `synchronous=FULL`, or reopening the same bytes, and all three are under
//! test. Every port is deterministic, so a scenario replays identically.

// This module is compiled once per integration binary, and each binary uses a
// different subset of it. Both lints below are artefacts of that: the unused
// helpers are used by a *sibling* binary, and nothing here is meant to be
// reachable outside the test tree in the first place.
#![allow(dead_code, unreachable_pub)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use governor_core::binding::{ConversationRef, WriteCapabilityState};
use governor_core::fence::{BindingGeneration, ObligationVersion, SafeToken, SourceRef};
use governor_core::id::{Id, IdKind, IdSource};
use governor_core::random::SecureRandom;
use governor_core::time::Timestamp;
use governor_core::worker_evidence::{ChildExitStatus, ManagedRunOutcome};
use governor_store_sqlite::{
    ArmDeliverySendRequest, BindForemanRequest, ClaimedDelivery, Clock, CompletionReceipts,
    CreateOrClaimDeliveryRequest, DeliveryOutcome, DurableArtifact, Failpoint, FailpointHook,
    OpenStore, OpenWorkerTurnRequest, OpenedWorkerTurn, ProjectSpec, PublishWorkerResultRequest,
    RecordDeliveryOutcomeRequest, RecordWorkerStartedRequest, SessionSpec, Store, StoreConfig,
    StoreError, StorePorts, StoreResult,
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use uuid::Uuid;

/// A clock that advances one millisecond per reading.
///
/// Monotonic and reproducible: a scenario's timestamps are a function of how
/// many instants it asked for, not of how fast the machine ran.
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
///
/// Never acceptable in a daemon, exactly right for proving the port is the only
/// way in and that a correlation ID is drawn once and persisted.
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

/// A failpoint that fires once, at one named point of one operation.
///
/// This is the seam the kill-window suites attach to: it aborts the
/// transaction body exactly where a process would have died, and the
/// transaction rolls back — which is what a crash before `COMMIT` looks like
/// from the next process's point of view.
pub struct FireOnce {
    target: (&'static str, Failpoint),
    fired: Mutex<bool>,
}

impl FireOnce {
    pub fn new(op: &'static str, point: Failpoint) -> Self {
        Self {
            target: (op, point),
            fired: Mutex::new(false),
        }
    }
}

impl FailpointHook for FireOnce {
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

/// The instant every simulated process starts its clock at, unless it says
/// otherwise.
pub const DEFAULT_CLOCK_START: i64 = 1_000;

/// A throwaway state root that can be opened and reopened.
pub struct Harness {
    dir: TempDir,
    /// Advances once per open, so each simulated process draws a distinct byte
    /// stream. A real CSPRNG never repeats itself across restarts, and a
    /// harness that did would hide exactly the bugs these suites look for —
    /// two "different" correlation IDs or lease tokens that are in fact equal.
    opens: AtomicU64,
}

impl Harness {
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temp state root"),
            opens: AtomicU64::new(0),
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.dir.path().join("governor.sqlite3")
    }

    /// Opens the store, running the full startup sequence.
    pub fn open(&self) -> StoreResult<Store> {
        self.open_at(DEFAULT_CLOCK_START, None)
    }

    /// Opens the store with a failpoint armed.
    pub fn open_with(&self, hook: Option<Box<dyn FailpointHook>>) -> StoreResult<Store> {
        self.open_at(DEFAULT_CLOCK_START, hook)
    }

    /// Opens the store as a process whose clock starts at `start_ms`.
    ///
    /// Every open otherwise starts from the same instant, which is what keeps
    /// scenarios reproducible — but "a much later process" is a real case, and
    /// a lease liveness window cannot be crossed without one.
    pub fn open_at(
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

    /// A second, read-only connection for assertions about raw rows.
    ///
    /// WAL permits concurrent readers, so this observes exactly what a crashed
    /// process would leave behind without disturbing the writer.
    pub fn inspect(&self) -> Connection {
        Connection::open_with_flags(self.database_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only inspection connection")
    }

    /// Every byte the state root currently holds, database and sidecars.
    pub fn raw_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.database_path().into_os_string();
            path.push(suffix);
            if let Ok(bytes) = std::fs::read(Path::new(&path)) {
                out.extend_from_slice(&bytes);
            }
        }
        out
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a redaction-safe token, panicking on a value the domain refuses.
pub fn token(value: &str) -> SafeToken {
    SafeToken::new(value).expect("fixture tokens are safe")
}

/// Builds a source identity from three short labels.
pub fn source(namespace: &str, event: &str, fence: &str) -> SourceRef {
    SourceRef::new(token(namespace), token(event), token(fence))
}

/// Mints an opaque identity for a fixture.
pub fn id<K: IdKind>(value: u128) -> Id<K> {
    Id::from_uuid(Uuid::from_u128(value))
}

/// Opens one worker turn with plausible provenance.
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
                display_name: Some(token("phase1-store")),
                runtime_instance_ref: Some(token("pane-3")),
                worker_session_ref: Some(token("sess-9")),
            },
            worker_turn_ref: Some(token("turn-1")),
            priority: 10,
        })
        .expect("opening a worker turn")
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

/// Drives an obligation to `running`.
pub fn start_worker(store: &Store, obligation: governor_core::id::ObligationId, run: &str) {
    store
        .record_worker_started(RecordWorkerStartedRequest {
            obligation,
            source: source("claude.init", run, "start"),
            incarnation: governor_core::fence::IncarnationGeneration::FIRST,
        })
        .expect("recording a verified worker start");
}

/// The receipts a complete successful managed run produces.
pub fn completion_receipts(run: &str) -> CompletionReceipts {
    CompletionReceipts {
        run_ref: token(run),
        final_result_complete: true,
        outcome: ManagedRunOutcome::Success,
        child_exit: ChildExitStatus::Success,
    }
}

/// Artifact metadata for bytes a test pretends are already durable.
///
/// The assertion [`DurableArtifact::assert_durable_from_parts`] demands is
/// *stood in for* here, not performed: these suites drive the database half
/// alone and have no artifact root. The file half — and the real bridge through
/// `governor_artifacts::PublishedArtifact::durable` — is proven by the
/// `governor-artifacts` suites, which publish real bytes and then commit.
pub fn durable_artifact(storage_ref: &str) -> DurableArtifact {
    DurableArtifact::assert_durable_from_parts(
        token(storage_ref),
        governor_core::artifact::ArtifactDigest::from_bytes([7u8; 32]),
        128,
        token("text.markdown"),
    )
}

/// Publishes a confirmed result for an obligation already `running`.
pub fn publish_result(
    store: &Store,
    obligation: governor_core::id::ObligationId,
    run: &str,
) -> StoreResult<governor_store_sqlite::PublishedResult> {
    store.publish_worker_result(PublishWorkerResultRequest {
        obligation,
        source: source("claude.result", run, "final"),
        incarnation: governor_core::fence::IncarnationGeneration::FIRST,
        receipts: completion_receipts(run),
        artifact: durable_artifact(run),
    })
}

/// Schedules the first wake revision for an obligation and claims an attempt.
pub fn schedule_wake(
    store: &Store,
    obligation: governor_core::id::ObligationId,
    generation: BindingGeneration,
    version: ObligationVersion,
    fenced_source: SourceRef,
) -> StoreResult<ClaimedDelivery> {
    store.create_or_claim_delivery(CreateOrClaimDeliveryRequest {
        obligation,
        binding_generation: generation,
        expected_version: version,
        expected_source: fenced_source,
        revision: governor_core::fence::DeliveryRevision::FIRST,
        attempt_budget: 3,
        wake_protocol: token("composer.v1"),
    })
}

/// Arms and accepts a wake, leaving the revision frozen at `accepted`.
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
                message: governor_core::foreman_turn::ProviderMessageRef::new(token(message)),
            },
        })
        .expect("recording exact acceptance evidence");
}

/// Counts rows in a table, for "zero rows changed" assertions.
pub fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("counting rows")
}

/// Reads one scalar text column.
pub fn scalar(conn: &Connection, sql: &str) -> Option<String> {
    conn.query_row(sql, [], |row| row.get::<_, Option<String>>(0))
        .unwrap_or(None)
}
