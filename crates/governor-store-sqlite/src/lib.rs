//! SQLite persistence authority for Command Governor.
//!
//! # Phase 1 role
//!
//! `governor-store-sqlite` is the single source of durable truth: the append-only
//! source/domain event log, the replayable projections built from it, and the
//! immutable result-artifact metadata that pins those artifacts while an
//! obligation is open. Event order is the daemon-assigned SQLite sequence, and a
//! projection mismatch on startup fails closed.
//!
//! # Boundary
//!
//! All writes go through one daemon-owned writer actor; there is never a second
//! independent writer. The database runs with WAL, foreign keys, a bounded busy
//! timeout, and `synchronous=FULL`, under explicit deterministic migrations with
//! a schema epoch/version check. There is no ORM, and no external I/O is
//! performed while a transaction is held.
//!
//! Result artifacts are published file-before-database: write an owner-private
//! temp file, sync it, atomically rename to its immutable key, sync the
//! containing directory, and only then commit the metadata, terminal event, and
//! `completed_unprocessed` obligation in a single transaction. This crate owns
//! the database half; the file half is the artifact layer's, and it hands this
//! crate [`ops::publish::DurableArtifact`] once the bytes are durable.
//!
//! # What is derived from the ledger, and what is not
//!
//! Obligations, browser deliveries and their attempts are **projections**: they
//! are rebuilt by folding `events` through the `governor-core` state machines,
//! and [`Store::verify_projections`] proves the stored rows agree with that
//! replay (`docs/testing.md` DB-001).
//!
//! The mutation-command journal, external attempts and resource leases are
//! **not** ledger-derived. Each is a self-contained row whose own transaction
//! protocol is the durability contract — an intent row must commit *alone and
//! first*, before any consequential I/O, so coupling it to an event append
//! would be coupling it to a fact the ledger may not hold. This follows
//! `docs/research/2026-08-31-durable-orchestration-pattern-review.md`, which
//! places the command journal and external attempts in their own tables under
//! one SQLite authority.
//!
//! # No I/O inside a transaction, structurally
//!
//! Everything ambient this crate can reach — the clock, the CSPRNG, identity
//! minting — lives behind [`ports::StorePorts`], which is lent only to the
//! pre-transaction phase of a write. Inside a transaction there is nothing to
//! call. See [`ports`] and [`tx`].

mod codec;
pub mod error;
mod event;
pub mod inspect;
mod load;
mod meta;
mod migrate;
mod open;
mod ops;
pub mod ports;
mod replay;
mod safe_metadata;
mod store;
mod tx;
mod writer;

pub use error::{
    CorruptReason, CorruptValue, PolicyViolation, ProjectionMismatch, RepairNeeded, StoreError,
    StoreResult,
};
pub use event::EventKind;
pub use inspect::ReadOnlyDiagnosis;
pub use load::{
    LineageEdge, ManagedConfigRecord, OpenCondition, OpenObligation, SessionLoadoutRecord,
};
pub use migrate::{MigrationReport, SUPPORTED_SCHEMA_EPOCH};
pub use open::{DEFAULT_BUSY_TIMEOUT_MS, PolicyReport, StoreConfig};
pub use ops::AttemptEvidence;
pub use ops::bootstrap::{
    BindForemanRequest, BoundForeman, OpenWorkerTurnRequest, OpenedWorkerTurn, ProjectSpec,
    SessionSpec,
};
pub use ops::claim::{
    AcknowledgeRequest, Acknowledged, DeliverHandoffRequest, ExpireClaimRequest, ExpiredClaim,
    MintClaimRequest, MintedClaim,
};
pub use ops::delivery::{
    ArmDeliverySendRequest, ClaimedDelivery, CreateOrClaimDeliveryRequest, DeliveryOutcome,
    ReconcileAmbiguousDeliveryRequest, RecordDeliveryOutcomeRequest,
};
pub use ops::effect::{
    ExternalOutcome, GrantedPermit, MarkExternalDispatchedRequest, RecordExternalIntentRequest,
    RecordExternalOutcomeRequest,
};
pub use ops::health::{
    HealthConditionRecorded, RaiseForemanUnreachableRequest, ResultArtifactMissingRequest,
    SessionHealthRequest, TerminalEvidenceConflictRequest,
};
pub use ops::lease::{AcquireLeaseRequest, GrantedLease, LeaseHolderRequest, ResourceRef};
pub use ops::mutation::{
    AckMutationReceiptRequest, BeginMutationRequest, CompleteMutationRequest, MutationAdmission,
};
pub use ops::recovery::StartupRecovery;
pub use ops::session::{
    AuthorizeWorkerSpawnRequest, BindSessionLoadoutRequest, BoundSessionLoadout,
    RecordManagedConfigRequest, RecordSessionLineageRequest, RecordedLineage,
    RecordedManagedConfig, ResolveWorkerLoadoutRequest, ResolvedLoadout,
};
pub use ops::worker::{
    CancelObligationRequest, CompletionReceipts, DurableArtifact, ObligationAdvanced,
    PublishWorkerResultRequest, PublishedResult, RecordWorkerFailureRequest,
    RecordWorkerStartedRequest,
};
pub use ports::{Clock, StorePorts};
pub use replay::VerifiedProjections;
pub use store::{OpenStore, StartupReport, Store};
pub use tx::{Failpoint, FailpointHook};
pub use writer::ObligationSnapshot;
