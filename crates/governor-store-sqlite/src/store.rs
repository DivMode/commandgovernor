//! The public face of the store: open, recover, then operate.
//!
//! # Startup order
//!
//! `docs/architecture.md` "Startup recovery order" and `docs/testing.md` DB-003
//! / DB-006 fix the sequence, and [`OpenStore::start`] is that sequence:
//!
//! 1. open the connection and **prove** the pragma policy is in force;
//! 2. gate on the schema epoch — a newer database is refused before anything
//!    else is read;
//! 3. apply outstanding migrations, each in its own transaction;
//! 4. advance the daemon epoch once;
//! 5. verify projection replay equivalence, failing closed on a mismatch;
//! 6. quarantine every effect whose fate a previous process lost;
//! 7. only now hand back a [`Store`].
//!
//! Steps 5 and 6 are the two that must not be skipped for convenience. A
//! caller cannot skip them: [`Store`] is only reachable through
//! [`OpenStore::start`], and there is no constructor that takes a connection.

use governor_core::artifact::ResultArtifact;
use governor_core::effect::EffectDecision;
use governor_core::fence::DaemonEpoch;
use governor_core::id::{ExternalAttemptId, ObligationId};
use rusqlite::TransactionBehavior;
use uuid::Uuid;

use crate::error::StoreResult;
use crate::load::{OpenCondition, OpenObligation};
use crate::migrate::{self, MigrationReport};
use crate::open::{self, PolicyReport, StoreConfig};
use crate::ops::AttemptEvidence;
use crate::ops::bootstrap::{
    BindForeman, BindForemanRequest, BoundForeman, OpenWorkerTurn, OpenWorkerTurnRequest,
    OpenedWorkerTurn,
};
use crate::ops::claim::{
    AcknowledgeObligation, AcknowledgeRequest, Acknowledged, DeliverHandoff, DeliverHandoffRequest,
    ExpireClaimRequest, ExpireForemanClaim, ExpiredClaim, MintClaimRequest, MintForemanClaim,
    MintedClaim,
};
use crate::ops::delivery::{
    ArmDeliverySend, ArmDeliverySendRequest, ClaimedDelivery, CreateOrClaimDelivery,
    CreateOrClaimDeliveryRequest, ReconcileAmbiguousDelivery, ReconcileAmbiguousDeliveryRequest,
    RecordDeliveryOutcome, RecordDeliveryOutcomeRequest,
};
use crate::ops::effect::{
    GrantedPermit, MarkExternalDispatched, MarkExternalDispatchedRequest, RecordExternalIntent,
    RecordExternalIntentRequest, RecordExternalOutcome, RecordExternalOutcomeRequest,
};
use crate::ops::health::{
    HealthConditionRecorded, RaiseForemanUnreachable, RaiseForemanUnreachableRequest,
    RaiseResultArtifactMissing, RecordTerminalEvidenceConflict, ResolveResultArtifactMissing,
    ResultArtifactMissingRequest, TerminalEvidenceConflictRequest,
};
use crate::ops::lease::{
    AcquireLease, AcquireLeaseRequest, GrantedLease, LeaseHolderRequest, ReleaseLease, RenewLease,
};
use crate::ops::mutation::{
    AckMutationReceipt, AckMutationReceiptRequest, BeginMutation, BeginMutationRequest,
    CompleteMutation, CompleteMutationRequest, MutationAdmission,
};
use crate::ops::recovery::{RecoverStartup, RecoverStartupRequest, StartupRecovery};
use crate::ops::worker::{
    CancelObligation, CancelObligationRequest, ObligationAdvanced, PublishWorkerResult,
    PublishWorkerResultRequest, PublishedResult, RecordWorkerFailure, RecordWorkerFailureRequest,
    RecordWorkerStarted, RecordWorkerStartedRequest,
};
use crate::ports::StorePorts;
use crate::replay::VerifiedProjections;
use crate::tx::FailpointHook;
use crate::writer::{Command, ObligationSnapshot, WriterHandle};

/// Everything a store needs to be opened.
///
/// The ports are required, not optional: this crate ships no real clock and no
/// real CSPRNG, so the composition root has to supply them. That is what keeps
/// "nothing ambient inside a transaction" true rather than aspirational.
pub struct OpenStore {
    /// Where the database lives and how long it waits for the write lock.
    pub config: StoreConfig,
    /// Clock, CSPRNG and identity minting.
    pub ports: StorePorts,
    /// Optional interruption seam for the deterministic crash suites.
    pub failpoints: Option<Box<dyn FailpointHook>>,
    /// Opaque identity to stamp on a database file being created.
    pub instance_id: Uuid,
}

impl std::fmt::Debug for OpenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenStore")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// What opening the store did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupReport {
    /// The pragma policy, as the engine reports it.
    pub policy: PolicyReport,
    /// Migrations applied or verified.
    pub migrations: MigrationReport,
    /// The epoch this process is running under.
    pub daemon_epoch: DaemonEpoch,
    /// How far the *previous* process had verified, read before this one ran.
    ///
    /// Diagnostics: a watermark far behind the ledger head says the last
    /// process stopped before it could finish verifying.
    pub previously_verified_through: Option<governor_core::fence::EventSeq>,
    /// Projection replay equivalence, proven before anything was scheduled.
    pub projections: VerifiedProjections,
    /// What startup quarantine found.
    pub recovery: StartupRecovery,
}

/// The SQLite authority.
///
/// Every method sends one typed message to the single writer actor and waits
/// for its typed answer. Nothing here holds a connection, so nothing here can
/// hold a transaction across a call into an adapter.
#[derive(Debug)]
pub struct Store {
    writer: WriterHandle,
    report: StartupReport,
}

impl OpenStore {
    /// Opens, migrates, recovers, and returns a ready store.
    ///
    /// # Errors
    ///
    /// - [`crate::StoreError::ConnectionPolicy`] when a required pragma is not
    ///   in force;
    /// - [`crate::StoreError::SchemaEpochTooNew`] when the database is newer
    ///   than this binary (nothing else is read);
    /// - [`crate::StoreError::MigrationChecksumMismatch`] on a drifted
    ///   migration;
    /// - [`crate::StoreError::RepairNeeded`] when a projection disagrees with
    ///   its replay;
    /// - a SQLite error.
    pub fn start(self) -> StoreResult<Store> {
        let Self {
            config,
            ports,
            failpoints,
            instance_id,
        } = self;

        let (mut conn, policy) = open::open(&config)?;
        let migrations =
            migrate::migrate(&mut conn, ports.now(), instance_id, failpoints.as_deref())?;

        // The epoch advances exactly once per process, in its own transaction,
        // before recovery reads it. Everything recovery quarantines is defined
        // relative to it.
        // The quarantine workload is counted here too, under the same write
        // lock that fixed the epoch it is relative to: `RecoverStartup::prepare`
        // must mint one identity per orphan, and `prepare` cannot read the
        // database. A count that turns out to be short fails the recovery
        // transaction closed rather than truncating it.
        let (daemon_epoch, previously_verified_through, quarantine_capacity) = {
            let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let watermark = crate::meta::last_verified_projection_seq(&transaction)?;
            let epoch = crate::meta::advance_daemon_epoch(&transaction)?;
            let workload = crate::ops::recovery::quarantine_workload(&transaction, epoch)?;
            transaction.commit()?;
            (epoch, watermark, workload)
        };

        let writer = WriterHandle::spawn(conn, ports, failpoints);

        // Replay equivalence before quarantine, and quarantine before the
        // caller can schedule anything: a store that cannot prove its own
        // projections must not go on to make external decisions from them.
        let projections = writer.query(Command::VerifyProjections)?;
        let recovery = writer.call::<RecoverStartup, _>(
            RecoverStartupRequest {
                daemon_epoch,
                quarantine_capacity,
            },
            Command::RecoverStartup,
        )?;

        Ok(Store {
            writer,
            report: StartupReport {
                policy,
                migrations,
                daemon_epoch,
                previously_verified_through,
                projections,
                recovery,
            },
        })
    }
}

impl Store {
    /// What opening this store did.
    #[must_use]
    pub const fn startup(&self) -> &StartupReport {
        &self.report
    }

    /// The epoch this process is running under.
    #[must_use]
    pub const fn daemon_epoch(&self) -> DaemonEpoch {
        self.report.daemon_epoch
    }

    /// Registers a project, task, session, incarnation, turn and obligation.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn open_worker_turn(
        &self,
        request: OpenWorkerTurnRequest,
    ) -> StoreResult<OpenedWorkerTurn> {
        self.writer
            .call::<OpenWorkerTurn, _>(request, Command::OpenWorkerTurn)
    }

    /// Commits a verified foreman binding, superseding older generations.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn bind_foreman(&self, request: BindForemanRequest) -> StoreResult<BoundForeman> {
        self.writer
            .call::<BindForeman, _>(request, Command::BindForeman)
    }

    /// Records a verified worker start.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn record_worker_started(
        &self,
        request: RecordWorkerStartedRequest,
    ) -> StoreResult<ObligationAdvanced> {
        self.writer
            .call::<RecordWorkerStarted, _>(request, Command::RecordWorkerStarted)
    }

    /// Records a verified terminal worker failure. Unprocessed work.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn record_worker_failure(
        &self,
        request: RecordWorkerFailureRequest,
    ) -> StoreResult<ObligationAdvanced> {
        self.writer
            .call::<RecordWorkerFailure, _>(request, Command::RecordWorkerFailure)
    }

    /// Publishes a confirmed final result whose artifact is already durable.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn publish_worker_result(
        &self,
        request: PublishWorkerResultRequest,
    ) -> StoreResult<PublishedResult> {
        self.writer
            .call::<PublishWorkerResult, _>(request, Command::PublishWorkerResult)
    }

    /// Creates or finds a wake revision and claims an attempt, before any I/O.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn create_or_claim_delivery(
        &self,
        request: CreateOrClaimDeliveryRequest,
    ) -> StoreResult<ClaimedDelivery> {
        self.writer
            .call::<CreateOrClaimDelivery, _>(request, Command::CreateOrClaimDelivery)
    }

    /// Arms the Send ambiguity fence immediately before the exact Send.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn arm_delivery_send(
        &self,
        request: ArmDeliverySendRequest,
    ) -> StoreResult<governor_core::fence::AttemptNo> {
        self.writer
            .call::<ArmDeliverySend, _>(request, Command::ArmDeliverySend)
    }

    /// Records what a delivery attempt actually did.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn record_delivery_outcome(
        &self,
        request: RecordDeliveryOutcomeRequest,
    ) -> StoreResult<governor_core::outbound::DeliveryState> {
        self.writer
            .call::<RecordDeliveryOutcome, _>(request, Command::RecordDeliveryOutcome)
    }

    /// Promotes an ambiguous wake revision to accepted on exact evidence.
    ///
    /// No Send happens and none becomes possible: the revision was already
    /// frozen and stays frozen (`docs/state-machines.md` "Ambiguous
    /// reconciliation", `docs/testing.md` DEL-015).
    ///
    /// # Errors
    ///
    /// - [`governor_core::error::Conflict::UnknownDeliveryId`] when the
    ///   correlation ID or the conversation does not name a current delivery —
    ///   deliberately undifferentiated;
    /// - [`governor_core::error::Conflict::StaleBindingGeneration`] when the
    ///   revision is not from the current generation;
    /// - [`governor_core::error::Conflict::IllegalDeliveryTransition`] when the
    ///   revision is not ambiguous, and
    ///   [`governor_core::error::Conflict::DeliveryRevisionFrozen`] when it is
    ///   already accepted on *different* evidence.
    pub fn reconcile_ambiguous_delivery(
        &self,
        request: ReconcileAmbiguousDeliveryRequest,
    ) -> StoreResult<governor_core::outbound::DeliveryState> {
        self.writer
            .call::<ReconcileAmbiguousDelivery, _>(request, Command::ReconcileAmbiguousDelivery)
    }

    /// Mints one claim from an accepted current-generation wake.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn mint_foreman_claim(&self, request: MintClaimRequest) -> StoreResult<MintedClaim> {
        self.writer
            .call::<MintForemanClaim, _>(request, Command::MintForemanClaim)
    }

    /// Records that the result or input request reached the claiming foreman.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn deliver_handoff(
        &self,
        request: DeliverHandoffRequest,
    ) -> StoreResult<ObligationAdvanced> {
        self.writer
            .call::<DeliverHandoff, _>(request, Command::DeliverHandoff)
    }

    /// Closes an obligation with a fully fenced disposition. ACK layer 3.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn acknowledge_obligation(&self, request: AcknowledgeRequest) -> StoreResult<Acknowledged> {
        self.writer
            .call::<AcknowledgeObligation, _>(request, Command::AcknowledgeObligation)
    }

    /// Returns a lapsed claim's obligation to the attention state it came from.
    ///
    /// Internal coordination, never a decision about the work: this closes
    /// nothing and releases no artifact (`docs/state-machines.md` "Claim/ACK
    /// fencing").
    ///
    /// # Errors
    ///
    /// - [`governor_core::error::Conflict::ObligationAlreadyClaimed`] when the
    ///   claim is still live, so there is nothing to expire;
    /// - [`governor_core::error::Conflict::StaleClaim`] when a different claim
    ///   holds the obligation;
    /// - a [`crate::StoreError`] from the transaction.
    pub fn expire_foreman_claim(&self, request: ExpireClaimRequest) -> StoreResult<ExpiredClaim> {
        self.writer
            .call::<ExpireForemanClaim, _>(request, Command::ExpireForemanClaim)
    }

    /// Commits the `received` row that must precede a consequential dispatch.
    ///
    /// # Errors
    ///
    /// - [`governor_core::error::Conflict::MutationResultUncertain`] for a
    ///   retry of an identity with no committed result — never a redispatch;
    /// - [`governor_core::error::Conflict::MutationCommandMismatch`] when the
    ///   identity was minted for a different operation.
    pub fn begin_mutation(&self, request: BeginMutationRequest) -> StoreResult<MutationAdmission> {
        self.writer
            .call::<BeginMutation, _>(request, Command::BeginMutation)
    }

    /// Commits a mutation's bounded safe result, before the reply is sent.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn complete_mutation(
        &self,
        request: CompleteMutationRequest,
    ) -> StoreResult<governor_core::mutation::MutationCommandStatus> {
        self.writer
            .call::<CompleteMutation, _>(request, Command::CompleteMutation)
    }

    /// ACK layer 1: retention eligibility, and nothing else.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn ack_mutation_receipt(
        &self,
        request: AckMutationReceiptRequest,
    ) -> StoreResult<governor_core::mutation::MutationCommandStatus> {
        self.writer
            .call::<AckMutationReceipt, _>(request, Command::AckMutationReceipt)
    }

    /// Commits one external-effect intent, then surrenders one permit.
    ///
    /// The permit is produced strictly after the intent transaction commits.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn record_external_intent(
        &self,
        request: RecordExternalIntentRequest,
    ) -> StoreResult<GrantedPermit> {
        self.writer
            .call::<RecordExternalIntent, _>(request, Command::RecordExternalIntent)
    }

    /// Commits the dispatch fence immediately before the adapter issues a call.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn mark_external_dispatched(
        &self,
        request: MarkExternalDispatchedRequest,
    ) -> StoreResult<governor_core::effect::ExternalAttemptState> {
        self.writer
            .call::<MarkExternalDispatched, _>(request, Command::MarkExternalDispatched)
    }

    /// Records what became of a dispatched external call.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn record_external_outcome(
        &self,
        request: RecordExternalOutcomeRequest,
    ) -> StoreResult<governor_core::effect::ExternalAttemptState> {
        self.writer
            .call::<RecordExternalOutcome, _>(request, Command::RecordExternalOutcome)
    }

    /// Acquires exclusive ownership of a resource.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn acquire_lease(&self, request: AcquireLeaseRequest) -> StoreResult<GrantedLease> {
        self.writer
            .call::<AcquireLease, _>(request, Command::AcquireLease)
    }

    /// Extends the current lease's liveness.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn renew_lease(
        &self,
        request: LeaseHolderRequest,
    ) -> StoreResult<governor_core::time::Timestamp> {
        self.writer
            .call::<RenewLease, _>(request, Command::RenewLease)
    }

    /// Gives a resource back.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn release_lease(
        &self,
        request: LeaseHolderRequest,
    ) -> StoreResult<governor_core::lease::LeaseState> {
        self.writer
            .call::<ReleaseLease, _>(request, Command::ReleaseLease)
    }

    /// Rebuilds every projection from the ledger and compares it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::StoreError::RepairNeeded`] on any disagreement.
    pub fn verify_projections(&self) -> StoreResult<VerifiedProjections> {
        self.writer.query(Command::VerifyProjections)
    }

    /// Reads one obligation's fenced state.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row error when the obligation does not exist.
    pub fn read_obligation(&self, obligation: ObligationId) -> StoreResult<ObligationSnapshot> {
        self.writer.ask(obligation, Command::ReadObligation)
    }

    /// Cancels delegated work on the local user's authority.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::StoreError`] from the transaction.
    pub fn cancel_obligation(
        &self,
        request: CancelObligationRequest,
    ) -> StoreResult<ObligationAdvanced> {
        self.writer
            .call::<CancelObligation, _>(request, Command::CancelObligation)
    }

    /// Reads every open health condition.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row error for an undecodable condition.
    pub fn open_health_conditions(&self) -> StoreResult<Vec<OpenCondition>> {
        self.writer.query(Command::OpenHealthConditions)
    }

    /// Reads every obligation that still owes somebody something.
    ///
    /// Each one's state is folded from its ledger slice, not read out of the
    /// materialised row, so this is the same fact
    /// [`Self::read_obligation`] reports, for the whole open set.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row error, or whatever folding an obligation refused
    /// on.
    pub fn list_open_obligations(&self) -> StoreResult<Vec<OpenObligation>> {
        self.writer.query(Command::ListOpenObligations)
    }

    /// Reads every committed result-artifact record.
    ///
    /// The artifact root's orphan sweep needs the set of keys the durable
    /// authority knows about; anything else in the root is unreferenced.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row error for an undecodable row.
    pub fn list_committed_artifacts(&self) -> StoreResult<Vec<ResultArtifact>> {
        self.writer.query(Command::ListCommittedArtifacts)
    }

    /// Opens `foreman_unreachable` for an obligation whose wake budget is spent.
    ///
    /// Attention only: it closes nothing, schedules nothing, and a repeat while
    /// the condition is open changes no row (`docs/testing.md` GPT-006).
    ///
    /// # Errors
    ///
    /// [`governor_core::error::Conflict::ObligationClosed`] for work nobody is
    /// owed any more.
    pub fn raise_foreman_unreachable(
        &self,
        request: RaiseForemanUnreachableRequest,
    ) -> StoreResult<HealthConditionRecorded> {
        self.writer
            .call::<RaiseForemanUnreachable, _>(request, Command::RaiseForemanUnreachable)
    }

    /// Opens `result_artifact_missing` for an artifact an open obligation pins.
    ///
    /// The explicit repair state `docs/testing.md` DB-008 requires. It does not
    /// make the obligation processable, and it does not close it.
    ///
    /// # Errors
    ///
    /// - [`governor_core::error::Conflict::ObligationClosed`] for closed work;
    /// - [`governor_core::error::Conflict::IllegalObligationTransition`] when
    ///   the obligation does not require that artifact.
    pub fn raise_result_artifact_missing(
        &self,
        request: ResultArtifactMissingRequest,
    ) -> StoreResult<HealthConditionRecorded> {
        self.writer
            .call::<RaiseResultArtifactMissing, _>(request, Command::RaiseResultArtifactMissing)
    }

    /// Closes `result_artifact_missing` after the bytes verified again.
    ///
    /// # Errors
    ///
    /// [`governor_core::error::Conflict::IllegalObligationTransition`] when the
    /// obligation does not require that artifact.
    pub fn resolve_result_artifact_missing(
        &self,
        request: ResultArtifactMissingRequest,
    ) -> StoreResult<HealthConditionRecorded> {
        self.writer
            .call::<ResolveResultArtifactMissing, _>(request, Command::ResolveResultArtifactMissing)
    }

    /// Records contradictory terminal evidence for a turn that already has one.
    ///
    /// `docs/testing.md` OBL-006. The arbitration is `governor-core`'s: only
    /// [`governor_core::worker_evidence::WorkerOutcome::NeedsReconciliation`]
    /// opens a condition, and what is stored is that class and the turn — never
    /// a receipt, a payload, or a second obligation.
    ///
    /// # Errors
    ///
    /// [`governor_core::error::Conflict::IllegalObligationTransition`] when the
    /// turn holds no confirmed terminal result to contradict, or when the
    /// presented receipts are not contradictory at all.
    pub fn record_terminal_evidence_conflict(
        &self,
        request: TerminalEvidenceConflictRequest,
    ) -> StoreResult<HealthConditionRecorded> {
        self.writer.call::<RecordTerminalEvidenceConflict, _>(
            request,
            Command::RecordTerminalEvidenceConflict,
        )
    }

    /// Resolves an external attempt without offering a permit.
    ///
    /// # Errors
    ///
    /// Returns the conflict [`governor_core::effect::ExternalAttempt::decide`]
    /// raises for an attempt that can neither replay nor reconcile.
    pub fn resolve_external_attempt(
        &self,
        attempt: ExternalAttemptId,
    ) -> StoreResult<EffectDecision<AttemptEvidence>> {
        self.writer.ask(attempt, Command::ResolveExternalAttempt)
    }
}
