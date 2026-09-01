//! Domain fixtures: the lifecycle steps every suite builds scenarios out of.
//!
//! These are thin. Each one drives the *real* store operation with plausible
//! provenance and returns what it committed, so a suite reads as a sequence of
//! domain facts rather than as request construction. Nothing here decides
//! anything: the fences, the arbitration and the transitions all stay where
//! they belong.
//!
//! [`publish_result`] is the one composition that matters: it publishes real
//! bytes through `governor-artifacts` first and only then commits the SQLite
//! transaction, which is the file-before-database ordering itself rather than a
//! stand-in for it.

use std::collections::BTreeSet;

use governor_artifacts::{
    ArtifactError, ArtifactStore, InvalidStorageKey, PublishRequest, PublishedArtifact,
    RetentionInput, StorageKey,
};
use governor_core::artifact::{ArtifactDigest, RetentionState};
use governor_core::binding::{ConversationRef, WriteCapabilityState};
use governor_core::fence::{
    AttemptNo, BindingGeneration, DeliveryRevision, IncarnationGeneration, ObligationVersion,
    SafeToken, SourceRef,
};
use governor_core::foreman_turn::ProviderMessageRef;
use governor_core::id::{ClaimId, Id, IdKind, ObligationId};
use governor_core::obligation::Disposition;
use governor_core::time::{DurationMs, Timestamp};
use governor_core::worker_evidence::{ChildExitStatus, ManagedRunOutcome, WorkerFailureClass};
use governor_store_sqlite::{
    AcknowledgeRequest, Acknowledged, ArmDeliverySendRequest, BindForemanRequest, ClaimedDelivery,
    CompletionReceipts, CreateOrClaimDeliveryRequest, DeliverHandoffRequest, DeliveryOutcome,
    ExpireClaimRequest, ExpiredClaim, MintClaimRequest, MintedClaim, ObligationAdvanced,
    ObligationSnapshot, OpenWorkerTurnRequest, OpenedWorkerTurn, ProjectSpec,
    PublishWorkerResultRequest, PublishedResult, RecordDeliveryOutcomeRequest,
    RecordWorkerFailureRequest, RecordWorkerStartedRequest, SessionSpec, Store, StoreError,
    StoreResult,
};
use rusqlite::Connection;
use uuid::Uuid;

use crate::harness::Harness;

/// Retention delay the fixtures apply when closing an obligation.
///
/// A day: long enough that nothing in these suites becomes deletable by
/// accident, so a released artifact is *released* and not gone.
pub const RETENTION_GRACE: DurationMs = DurationMs::from_millis(86_400_000);

/// A claim minted with no lifetime is past its expiry the moment the next
/// operation reads the clock, which is what makes expiry deterministic here.
///
/// A lapsed claim authorises no mutation at all — the store refuses a handoff
/// under one just as it refuses an ACK — so a scenario that needs an
/// obligation *in `processing`* under a lapsed claim mints with
/// [`LIVE_CLAIM`], delivers the handoff, and then calls [`lapse_claim`].
pub const ALREADY_LAPSED: DurationMs = DurationMs::ZERO;

/// A claim lifetime long enough to stay live for a whole scenario.
pub const LIVE_CLAIM: DurationMs = DurationMs::from_millis(60_000);

/// Moves a shared clock far enough that a [`LIVE_CLAIM`] claim has lapsed.
pub fn lapse_claim(clock: &crate::clock::FakeClock) {
    clock.advance(DurationMs::from_millis(LIVE_CLAIM.as_millis() + 1_000));
}

/// Builds a redaction-safe token.
///
/// # Panics
///
/// Panics on a value the domain refuses, which in a fixture is a bug in the
/// fixture.
#[must_use]
pub fn token(value: &str) -> SafeToken {
    SafeToken::new(value).expect("fixture tokens are safe")
}

/// Builds a source identity from three short labels.
#[must_use]
pub fn source(namespace: &str, event: &str, fence: &str) -> SourceRef {
    SourceRef::new(token(namespace), token(event), token(fence))
}

/// Mints an opaque identity for a fixture.
#[must_use]
pub fn id<K: IdKind>(value: u128) -> Id<K> {
    Id::from_uuid(Uuid::from_u128(value))
}

/// Opens one worker turn with plausible provenance.
///
/// # Panics
///
/// Panics when the store refuses, which no fence permits here.
pub fn open_turn(store: &Store) -> OpenedWorkerTurn {
    open_named_turn(store, "turn-1")
}

/// Opens one worker turn with an explicit worker-native turn reference.
///
/// # Panics
///
/// Panics when the store refuses.
pub fn open_named_turn(store: &Store, worker_turn_ref: &str) -> OpenedWorkerTurn {
    store
        .open_worker_turn(worker_turn_request(worker_turn_ref))
        .expect("opening a worker turn")
}

/// The request [`open_turn`] sends, for a caller that must handle the refusal.
#[must_use]
pub fn worker_turn_request(worker_turn_ref: &str) -> OpenWorkerTurnRequest {
    OpenWorkerTurnRequest {
        project: ProjectSpec {
            source_host: token("github.com"),
            source_repo_id: Some(token("R_kgDO")),
            source_repo_display: Some(token("DivMode.commandgovernor")),
        },
        source_issue_ref: Some(token("issue-2")),
        session: SessionSpec {
            runtime_kind: token("herdr"),
            worker_kind: token("claude"),
            display_name: Some(token("phase1-testkit")),
            runtime_instance_ref: Some(token("pane-3")),
            worker_session_ref: Some(token("sess-9")),
        },
        worker_turn_ref: Some(token(worker_turn_ref)),
        priority: 10,
    }
}

/// A worker-turn request whose token-shaped fields carry SEC-001 sentinels.
///
/// Three of the four representable sentinels go in here, through the ordinary
/// public request; the fourth is the acceptance evidence a wake records. See
/// [`crate::sentinels::INJECTED`] for the pairing and the column each is
/// allowed to reach. Everything else about the request is the usual fixture,
/// so a lifecycle driven from it is the same lifecycle every other suite runs.
///
/// # Panics
///
/// Panics when a sentinel the corpus calls token-shaped is not, which would
/// mean [`crate::sentinels::FORBIDDEN`] disagrees with the charset.
#[must_use]
pub fn sentinel_turn_request() -> OpenWorkerTurnRequest {
    let mut request = worker_turn_request(crate::sentinels::value_of("provider api token"));
    request.source_issue_ref = Some(token(crate::sentinels::value_of("github credential")));
    request.session.display_name = Some(token(crate::sentinels::value_of("environment secret")));
    request
}

/// The acceptance evidence [`sentinel_turn_request`]'s lifecycle records.
#[must_use]
pub fn sentinel_message_ref() -> &'static str {
    crate::sentinels::value_of("browser cookie")
}

/// Binds a foreman conversation and returns its generation.
///
/// # Panics
///
/// Panics when the store refuses.
pub fn bind(store: &Store, conversation: &str) -> BindingGeneration {
    store
        .bind_foreman(bind_request(conversation))
        .expect("binding a verified conversation")
        .generation
}

/// The request [`bind`] sends, for a caller that must handle the refusal.
#[must_use]
pub fn bind_request(conversation: &str) -> BindForemanRequest {
    BindForemanRequest {
        provider: token("chatgpt"),
        conversation: ConversationRef::new(token(conversation)),
        conversation_url_ref: token(conversation),
        profile: token("cg-profile"),
        connector_abi: token("command-governor-foreman.v1"),
        capability_epoch: 1,
        write_capability: WriteCapabilityState::Proven,
    }
}

/// Drives an obligation to `running`.
///
/// # Panics
///
/// Panics when the store refuses.
pub fn start_worker(store: &Store, obligation: ObligationId, run: &str) {
    store
        .record_worker_started(RecordWorkerStartedRequest {
            obligation,
            source: source("claude.init", run, "start"),
            incarnation: IncarnationGeneration::FIRST,
        })
        .expect("recording a verified worker start");
}

/// The receipts a complete successful managed run produces.
#[must_use]
pub fn completion_receipts(run: &str) -> CompletionReceipts {
    CompletionReceipts {
        run_ref: token(run),
        final_result_complete: true,
        outcome: ManagedRunOutcome::Success,
        child_exit: ChildExitStatus::Success,
    }
}

/// The bounded final result a fixture worker produced.
pub const FINAL_RESULT: &[u8] =
    b"# Review\n\nThe change is ready: 3 files, 1 test added. CGSENTINELFINALRESULT\n";

/// Which half of a publication refused.
#[derive(Debug)]
#[non_exhaustive]
pub enum PublicationFailure {
    /// The bytes were never made durable.
    Artifact(ArtifactError),
    /// The bytes are durable; the transaction did not commit.
    Store(StoreError),
}

/// Publishes bytes through the artifact layer alone.
///
/// # Errors
///
/// Returns whatever the artifact layer refused on.
pub fn publish_bytes(
    artifacts: &mut ArtifactStore,
    bytes: &[u8],
) -> Result<PublishedArtifact, ArtifactError> {
    artifacts.publish(PublishRequest {
        bytes,
        media_type: token("text.markdown"),
    })
}

/// The whole durable publication: artifact first, then the one transaction.
///
/// This is the composition the daemon performs, and the reason the suites can
/// assert on both halves at once.
///
/// # Errors
///
/// Returns [`PublicationFailure`] naming which half refused.
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

/// Records a verified terminal worker failure. Unprocessed work, not a closure.
///
/// # Errors
///
/// Returns whatever fence the store refused on.
pub fn record_failure(
    store: &Store,
    obligation: ObligationId,
    run: &str,
) -> StoreResult<ObligationAdvanced> {
    store.record_worker_failure(RecordWorkerFailureRequest {
        obligation,
        source: source("claude.result", run, "error"),
        incarnation: IncarnationGeneration::FIRST,
        failure: WorkerFailureClass::StructuredError,
    })
}

/// Reads one obligation's fenced state.
///
/// # Panics
///
/// Panics when the obligation does not exist.
pub fn snapshot(store: &Store, obligation: ObligationId) -> ObligationSnapshot {
    store
        .read_obligation(obligation)
        .expect("obligation snapshot")
}

/// Schedules a wake revision for an obligation and claims an attempt on it.
///
/// # Errors
///
/// Returns whatever fence the store refused on.
pub fn schedule_wake(
    store: &Store,
    obligation: ObligationId,
    generation: BindingGeneration,
    revision: DeliveryRevision,
) -> StoreResult<ClaimedDelivery> {
    let current = snapshot(store, obligation);
    store.create_or_claim_delivery(CreateOrClaimDeliveryRequest {
        obligation,
        binding_generation: generation,
        expected_version: current.version,
        expected_source: current.source,
        revision,
        attempt_budget: 3,
        wake_protocol: token("composer.v1"),
    })
}

/// Schedules the first wake revision against explicit fences.
///
/// # Errors
///
/// Returns whatever fence the store refused on.
pub fn schedule_wake_fenced(
    store: &Store,
    obligation: ObligationId,
    generation: BindingGeneration,
    version: ObligationVersion,
    fenced_source: SourceRef,
) -> StoreResult<ClaimedDelivery> {
    store.create_or_claim_delivery(CreateOrClaimDeliveryRequest {
        obligation,
        binding_generation: generation,
        expected_version: version,
        expected_source: fenced_source,
        revision: DeliveryRevision::FIRST,
        attempt_budget: 3,
        wake_protocol: token("composer.v1"),
    })
}

/// Arms the Send ambiguity fence for one attempt.
///
/// # Errors
///
/// Returns whatever fence the store refused on.
pub fn arm_send(
    store: &Store,
    claimed: &ClaimedDelivery,
    generation: BindingGeneration,
) -> StoreResult<AttemptNo> {
    store.arm_delivery_send(ArmDeliverySendRequest {
        delivery_id: claimed.delivery_id.clone(),
        binding_generation: generation,
        attempt: claimed.attempt,
    })
}

/// Records what one attempt actually did.
///
/// # Errors
///
/// Returns whatever the delivery machine refused on.
pub fn record_outcome(
    store: &Store,
    claimed: &ClaimedDelivery,
    attempt: AttemptNo,
    outcome: DeliveryOutcome,
) -> StoreResult<governor_core::outbound::DeliveryState> {
    store.record_delivery_outcome(RecordDeliveryOutcomeRequest {
        delivery_id: claimed.delivery_id.clone(),
        attempt,
        outcome,
    })
}

/// Arms and accepts a wake, leaving the revision frozen at `accepted`.
///
/// # Panics
///
/// Panics when either half refuses.
pub fn accept_wake(
    store: &Store,
    claimed: &ClaimedDelivery,
    generation: BindingGeneration,
    message: &str,
) {
    arm_send(store, claimed, generation).expect("arming the Send fence");
    record_outcome(
        store,
        claimed,
        claimed.attempt,
        DeliveryOutcome::Accepted {
            message: ProviderMessageRef::new(token(message)),
        },
    )
    .expect("recording exact acceptance evidence");
}

/// Mints one claim from an accepted wake.
///
/// # Errors
///
/// Returns whatever fence the store refused on.
pub fn mint_claim(
    store: &Store,
    obligation: ObligationId,
    wake: &ClaimedDelivery,
    generation: BindingGeneration,
    lifetime: DurationMs,
) -> StoreResult<MintedClaim> {
    let current = snapshot(store, obligation);
    store.mint_foreman_claim(MintClaimRequest {
        obligation,
        presented_delivery_id: wake.delivery_id.clone(),
        binding_generation: generation,
        expected_version: current.version,
        expected_source: current.source,
        lifetime,
    })
}

/// Records that the result reached the claiming foreman.
///
/// # Errors
///
/// Returns whatever the obligation machine refused on.
pub fn handoff(
    store: &Store,
    obligation: ObligationId,
    claim: ClaimId,
) -> StoreResult<ObligationAdvanced> {
    store.deliver_handoff(DeliverHandoffRequest { obligation, claim })
}

/// Closes an obligation with a fully fenced disposition.
///
/// # Errors
///
/// Returns whatever fence the store refused on.
pub fn acknowledge(
    store: &Store,
    obligation: ObligationId,
    generation: BindingGeneration,
    claim: ClaimId,
    disposition: Disposition,
) -> StoreResult<Acknowledged> {
    let current = snapshot(store, obligation);
    store.acknowledge_obligation(AcknowledgeRequest {
        obligation,
        expected_version: current.version,
        expected_source: current.source,
        binding_generation: generation,
        claim,
        disposition,
        retention_grace: RETENTION_GRACE,
    })
}

/// Returns a lapsed claim's obligation to the attention it came from.
///
/// # Errors
///
/// Returns whatever the claim machine refused on.
pub fn expire_claim(
    store: &Store,
    obligation: ObligationId,
    claim: ClaimId,
) -> StoreResult<ExpiredClaim> {
    store.expire_foreman_claim(ExpireClaimRequest { obligation, claim })
}

/// One obligation driven to an accepted wake, with a real durable artifact.
#[derive(Debug)]
pub struct AcceptedWork {
    /// The obligation, open and awaiting review.
    pub obligation: ObligationId,
    /// The active binding generation.
    pub generation: BindingGeneration,
    /// The accepted wake revision.
    pub wake: ClaimedDelivery,
    /// The artifact whose bytes are durable on disk.
    pub artifact: PublishedArtifact,
}

/// Drives one obligation to `completed_unprocessed` with an accepted wake.
///
/// The whole prefix every browser, MCP and retention scenario starts from, in
/// one call: project, task, session, turn, binding, worker start, a real
/// artifact publication, a scheduled wake, and exact acceptance evidence.
///
/// # Panics
///
/// Panics when any step refuses, which no fence in this sequence permits.
pub fn accepted_work(
    store: &Store,
    artifacts: &mut ArtifactStore,
    conversation: &str,
) -> AcceptedWork {
    let turn = open_turn(store);
    let generation = bind(store, conversation);
    start_worker(store, turn.obligation, "run-1");
    let (artifact, _) = publish_result(store, artifacts, turn.obligation, "run-1", FINAL_RESULT)
        .expect("publishing a confirmed result");
    let wake = schedule_wake(store, turn.obligation, generation, DeliveryRevision::FIRST)
        .expect("scheduling a wake");
    accept_wake(store, &wake, generation, "msg-1");
    AcceptedWork {
        obligation: turn.obligation,
        generation,
        wake,
        artifact,
    }
}

/// Drives one obligation all the way to `processing` under a live claim.
///
/// # Panics
///
/// Panics when any step refuses.
pub fn handed_over(
    store: &Store,
    artifacts: &mut ArtifactStore,
    conversation: &str,
    lifetime: DurationMs,
) -> (AcceptedWork, ClaimId) {
    let work = accepted_work(store, artifacts, conversation);
    let minted = mint_claim(
        store,
        work.obligation,
        &work.wake,
        work.generation,
        lifetime,
    )
    .expect("minting a claim from the accepted wake");
    handoff(store, work.obligation, minted.claim).expect("handing the result over");
    (work, minted.claim)
}

// --- Reading back what actually committed ------------------------------------

/// One committed `result_artifacts` row, as the durable authority holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRow {
    /// Opaque daemon-allocated key the bytes live under.
    pub storage_ref: String,
    /// Recorded digest, lowercase hex.
    pub sha256_hex: String,
    /// Recorded length.
    pub byte_len: u64,
    /// Whether an open obligation still pins it.
    pub retention_state: String,
    /// Earliest permitted deletion instant, stamped by a closing ACK.
    pub deletable_at: Option<Timestamp>,
}

impl ArtifactRow {
    /// The key, re-validated. A tampered row must not become a path.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStorageKey`] when the recorded value is not a legal
    /// single path component.
    pub fn key(&self) -> Result<StorageKey, InvalidStorageKey> {
        StorageKey::parse(&self.storage_ref)
    }

    /// The recorded digest.
    ///
    /// # Panics
    ///
    /// Panics when the column is not 32 bytes of hex, which is a corrupt row
    /// rather than a test fixture problem.
    #[must_use]
    pub fn digest(&self) -> ArtifactDigest {
        let mut bytes = [0u8; 32];
        assert_eq!(self.sha256_hex.len(), 64, "a digest column is 32 hex bytes");
        for (slot, pair) in bytes.iter_mut().zip(self.sha256_hex.as_bytes().chunks(2)) {
            let text = std::str::from_utf8(pair).expect("hex is ascii");
            *slot = u8::from_str_radix(text, 16).expect("hex digit pair");
        }
        ArtifactDigest::from_bytes(bytes)
    }

    /// Retention as the domain models it.
    ///
    /// # Panics
    ///
    /// Panics on a label outside the closed set.
    #[must_use]
    pub fn retention(&self) -> RetentionState {
        match self.retention_state.as_str() {
            "pinned" => RetentionState::Pinned,
            "eligible" => RetentionState::Eligible,
            other => panic!("unknown retention label {other}"),
        }
    }

    /// The input one retention sweep decides on.
    ///
    /// # Panics
    ///
    /// Panics when the recorded key is not a legal storage key.
    #[must_use]
    pub fn as_retention_input(&self) -> RetentionInput {
        RetentionInput {
            key: self.key().expect("committed keys are valid"),
            state: self.retention(),
            deletable_at: self.deletable_at,
        }
    }
}

/// Every committed artifact row, ordered by key.
///
/// # Panics
///
/// Panics when the table cannot be read.
#[must_use]
pub fn artifact_rows(conn: &Connection) -> Vec<ArtifactRow> {
    let mut statement = conn
        .prepare(
            "SELECT storage_ref, sha256_hex, byte_len, retention_state,
                    eligible_for_delete_at_ms
               FROM result_artifacts ORDER BY storage_ref",
        )
        .expect("preparing the artifact query");
    let rows = statement
        .query_map([], |row| {
            Ok(ArtifactRow {
                storage_ref: row.get(0)?,
                sha256_hex: row.get(1)?,
                byte_len: row.get::<_, i64>(2)?.try_into().expect("non-negative"),
                retention_state: row.get(3)?,
                deletable_at: row
                    .get::<_, Option<i64>>(4)?
                    .map(Timestamp::from_unix_millis),
            })
        })
        .expect("querying artifacts");
    rows.map(|row| row.expect("artifact row")).collect()
}

/// Every `storage_ref` a committed row references.
///
/// # Panics
///
/// As [`artifact_rows`].
#[must_use]
pub fn committed_keys(conn: &Connection) -> BTreeSet<StorageKey> {
    artifact_rows(conn)
        .into_iter()
        .filter_map(|row| row.key().ok())
        .collect()
}

/// Storage refs referenced by an obligation in `completed_unprocessed`.
///
/// # Panics
///
/// Panics when the join cannot be read.
#[must_use]
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
///
/// # Panics
///
/// Panics naming the storage ref whose bytes are not durable and verifiable.
pub fn assert_no_completion_without_durable_bytes(harness: &Harness, context: &str) {
    let conn = harness.inspect();
    let artifacts = harness.open_artifacts();
    let rows = artifact_rows(&conn);
    for storage_ref in completed_unprocessed_refs(&conn) {
        let row = rows
            .iter()
            .find(|row| row.storage_ref == storage_ref)
            .unwrap_or_else(|| {
                panic!("{context}: a completion references an unknown artifact row")
            });
        let key = row
            .key()
            .unwrap_or_else(|_| panic!("{context}: a completion references an illegal key"));
        let bytes = artifacts
            .read_verified(&key, row.digest(), row.byte_len)
            .unwrap_or_else(|error| {
                panic!(
                    "{context}: completed_unprocessed references {storage_ref}, \
                     which is not durable and verifiable: {error}"
                )
            });
        assert_eq!(
            u64::try_from(bytes.len()).expect("length fits"),
            row.byte_len,
            "{context}: the stored length disagrees with the committed row"
        );
    }
}
