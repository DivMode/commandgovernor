//! The single daemon-owned writer actor.
//!
//! # One writer, one thread, no pool
//!
//! `docs/adr/0002-rust-daemon-and-sqlite.md`: SQLite serialises writes anyway,
//! so a pool cannot make two write transactions commit concurrently and would
//! only make lock ordering less explicit. There is exactly one
//! [`rusqlite::Connection`], it lives on one dedicated OS thread, and it never
//! leaves. Every state change is a [`Command`] sent down a channel and answered
//! on a reply channel the caller owns.
//!
//! The thread is a plain `std::thread` with `std::sync::mpsc` channels. No
//! Tokio: the async boundary belongs to `governor-daemon`, which will wrap
//! these synchronous calls at its own edge. Keeping the runtime out of here is
//! what lets the crash and replay suites drive the store directly.
//!
//! # The transaction shape, in one place
//!
//! [`run`] is the only code that opens a transaction, and it always does the
//! same four things:
//!
//! 1. `prepare` the operation, with the ports — *outside* the transaction;
//! 2. `BEGIN IMMEDIATE`, taking the write lock before the first dependent read;
//! 3. `commit` the operation body, with no ports in scope;
//! 4. `COMMIT`, and only then `finish`.
//!
//! Step 4 is why [`crate::tx::WriteOp`] has a third phase at all: it is the
//! only place a durable-intent acceptance may be surrendered. See
//! [`crate::ops::effect`].
//!
//! A body that returns `Err` never reaches step 4 and the transaction is
//! dropped, which rolls it back. A rejected fence therefore changes zero rows,
//! and that is a property of this function rather than of every operation
//! remembering to undo itself.

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use governor_core::effect::EffectDecision;
use governor_core::id::{ExternalAttemptId, ObligationId};
use rusqlite::{Connection, TransactionBehavior};

use crate::error::{StoreError, StoreResult};
use crate::load::OpenCondition;
use crate::ops::AttemptEvidence;
use crate::ops::bootstrap::{BindForeman, OpenWorkerTurn};
use crate::ops::claim::{AcknowledgeObligation, DeliverHandoff, MintForemanClaim};
use crate::ops::delivery::{ArmDeliverySend, CreateOrClaimDelivery, RecordDeliveryOutcome};
use crate::ops::effect::{MarkExternalDispatched, RecordExternalIntent, RecordExternalOutcome};
use crate::ops::lease::{AcquireLease, ReleaseLease, RenewLease};
use crate::ops::mutation::{AckMutationReceipt, BeginMutation, CompleteMutation};
use crate::ops::recovery::RecoverStartup;
use crate::ops::worker::{
    CancelObligation, PublishWorkerResult, RecordWorkerFailure, RecordWorkerStarted,
};
use crate::ports::StorePorts;
use crate::replay::{self, VerifiedProjections};
use crate::tx::{FailpointHook, Tx, WriteOp};

/// One request and the channel its answer goes back on.
///
/// Typed on both ends: the caller knows the request type and the reply type,
/// and the actor cannot mix two operations' replies up because each carries its
/// own sender.
pub(crate) struct Job<O: WriteOp> {
    pub(crate) request: O::Request,
    pub(crate) reply: SyncSender<StoreResult<O::Output>>,
}

/// A read that needs the connection but writes nothing.
pub(crate) struct Query<T> {
    pub(crate) reply: SyncSender<StoreResult<T>>,
}

/// A read that needs one argument.
pub(crate) struct Ask<A, T> {
    pub(crate) argument: A,
    pub(crate) reply: SyncSender<StoreResult<T>>,
}

/// The complete set of messages the writer actor accepts.
///
/// Deliberately an explicit enum rather than a boxed closure: this list *is*
/// the store's mutating surface, and a reviewer can see all of it at once.
pub(crate) enum Command {
    OpenWorkerTurn(Job<OpenWorkerTurn>),
    BindForeman(Job<BindForeman>),
    RecordWorkerStarted(Job<RecordWorkerStarted>),
    RecordWorkerFailure(Job<RecordWorkerFailure>),
    CancelObligation(Job<CancelObligation>),
    PublishWorkerResult(Job<PublishWorkerResult>),
    CreateOrClaimDelivery(Job<CreateOrClaimDelivery>),
    ArmDeliverySend(Job<ArmDeliverySend>),
    RecordDeliveryOutcome(Job<RecordDeliveryOutcome>),
    MintForemanClaim(Job<MintForemanClaim>),
    DeliverHandoff(Job<DeliverHandoff>),
    AcknowledgeObligation(Job<AcknowledgeObligation>),
    BeginMutation(Job<BeginMutation>),
    CompleteMutation(Job<CompleteMutation>),
    AckMutationReceipt(Job<AckMutationReceipt>),
    RecordExternalIntent(Job<RecordExternalIntent>),
    MarkExternalDispatched(Job<MarkExternalDispatched>),
    RecordExternalOutcome(Job<RecordExternalOutcome>),
    AcquireLease(Job<AcquireLease>),
    RenewLease(Job<RenewLease>),
    ReleaseLease(Job<ReleaseLease>),
    RecoverStartup(Job<RecoverStartup>),
    VerifyProjections(Query<VerifiedProjections>),
    OpenHealthConditions(Query<Vec<OpenCondition>>),
    ReadObligation(Ask<ObligationId, ObligationSnapshot>),
    ResolveExternalAttempt(Ask<ExternalAttemptId, EffectDecision<AttemptEvidence>>),
    Shutdown,
}

/// A read-only view of one obligation's fenced state.
///
/// Everything a caller needs to present the next fenced mutation, and nothing
/// it could mutate through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObligationSnapshot {
    /// Current lifecycle state.
    pub state: governor_core::obligation::ObligationState,
    /// Compare-and-swap version to present.
    pub version: governor_core::fence::ObligationVersion,
    /// Source fact the obligation currently stands on.
    pub source: governor_core::fence::SourceRef,
    /// Current claim, if one is held.
    pub claim: Option<governor_core::id::ClaimId>,
    /// Artifact required for review, if any.
    pub result_artifact: Option<governor_core::id::ResultArtifactId>,
    /// Whether the obligation still owes somebody something.
    pub open: bool,
}

/// Runs the actor loop until the channel closes or a shutdown arrives.
pub(crate) fn serve(
    mut conn: Connection,
    mut ports: StorePorts,
    hook: Option<Box<dyn FailpointHook>>,
    commands: &Receiver<Command>,
) {
    let hook = hook.as_deref();
    while let Ok(command) = commands.recv() {
        if dispatch(&mut conn, &mut ports, hook, command) {
            break;
        }
    }
}

/// Handles one message. Returns `true` when the actor must stop.
fn dispatch(
    conn: &mut Connection,
    ports: &mut StorePorts,
    hook: Option<&dyn FailpointHook>,
    command: Command,
) -> bool {
    match command {
        Command::OpenWorkerTurn(job) => run(conn, ports, hook, job),
        Command::BindForeman(job) => run(conn, ports, hook, job),
        Command::RecordWorkerStarted(job) => run(conn, ports, hook, job),
        Command::RecordWorkerFailure(job) => run(conn, ports, hook, job),
        Command::CancelObligation(job) => run(conn, ports, hook, job),
        Command::PublishWorkerResult(job) => run(conn, ports, hook, job),
        Command::CreateOrClaimDelivery(job) => run(conn, ports, hook, job),
        Command::ArmDeliverySend(job) => run(conn, ports, hook, job),
        Command::RecordDeliveryOutcome(job) => run(conn, ports, hook, job),
        Command::MintForemanClaim(job) => run(conn, ports, hook, job),
        Command::DeliverHandoff(job) => run(conn, ports, hook, job),
        Command::AcknowledgeObligation(job) => run(conn, ports, hook, job),
        Command::BeginMutation(job) => run(conn, ports, hook, job),
        Command::CompleteMutation(job) => run(conn, ports, hook, job),
        Command::AckMutationReceipt(job) => run(conn, ports, hook, job),
        Command::RecordExternalIntent(job) => run(conn, ports, hook, job),
        Command::MarkExternalDispatched(job) => run(conn, ports, hook, job),
        Command::RecordExternalOutcome(job) => run(conn, ports, hook, job),
        Command::AcquireLease(job) => run(conn, ports, hook, job),
        Command::RenewLease(job) => run(conn, ports, hook, job),
        Command::ReleaseLease(job) => run(conn, ports, hook, job),
        Command::RecoverStartup(job) => run(conn, ports, hook, job),
        Command::VerifyProjections(query) => {
            // Writes the watermark, so it takes the write lock like any other
            // mutation rather than pretending to be a pure read.
            let answer = in_write_transaction(conn, hook, "verify_projections", replay::verify);
            let _ = query.reply.send(answer);
        }
        Command::OpenHealthConditions(query) => {
            let answer = in_read_transaction(
                conn,
                hook,
                "open_health_conditions",
                crate::load::open_conditions,
            );
            let _ = query.reply.send(answer);
        }
        Command::ReadObligation(ask) => {
            let answer = in_read_transaction(conn, hook, "read_obligation", |tx| {
                let loaded = crate::load::obligation(tx, ask.argument)?;
                let projection = loaded.projection;
                Ok(ObligationSnapshot {
                    state: projection.state(),
                    version: projection.version(),
                    source: projection.source().clone(),
                    claim: projection.claim(),
                    result_artifact: projection.result_artifact(),
                    open: projection.is_open(),
                })
            });
            let _ = ask.reply.send(answer);
        }
        Command::ResolveExternalAttempt(ask) => {
            let answer = in_read_transaction(conn, hook, "resolve_external_attempt", |tx| {
                crate::ops::effect::resolve(tx, ask.argument)
            });
            let _ = ask.reply.send(answer);
        }
        Command::Shutdown => return true,
    }
    false
}

/// The one place a write transaction is opened.
fn run<O: WriteOp>(
    conn: &mut Connection,
    ports: &mut StorePorts,
    hook: Option<&dyn FailpointHook>,
    job: Job<O>,
) {
    // Phase 1: gather ports. Outside the transaction, on purpose.
    let prepared = match O::prepare(job.request, ports) {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = job.reply.send(Err(error));
            return;
        }
    };

    let answer = (|| {
        // Phase 2: take the write lock before the first read it depends on.
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let committed = {
            let tx = Tx::new(&transaction, hook, O::NAME);
            // Phase 3: compare then mutate, with no ports in scope.
            prepared.commit(&tx)?
        };
        transaction.commit()?;
        // Phase 4: only now. `finish` consumes the operation by value.
        Ok(prepared.finish(committed))
    })();
    let _ = job.reply.send(answer);
}

fn in_write_transaction<T>(
    conn: &mut Connection,
    hook: Option<&dyn FailpointHook>,
    name: &'static str,
    body: impl FnOnce(&Tx<'_>) -> StoreResult<T>,
) -> StoreResult<T> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let value = {
        let tx = Tx::new(&transaction, hook, name);
        body(&tx)?
    };
    transaction.commit()?;
    Ok(value)
}

fn in_read_transaction<T>(
    conn: &mut Connection,
    hook: Option<&dyn FailpointHook>,
    name: &'static str,
    body: impl FnOnce(&Tx<'_>) -> StoreResult<T>,
) -> StoreResult<T> {
    // Deferred: a read takes a shared lock, and taking the write lock for it
    // would let a query block a mutation for no reason.
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let tx = Tx::new(&transaction, hook, name);
    body(&tx)
}

/// The caller's side of the actor.
pub(crate) struct WriterHandle {
    sender: SyncSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl WriterHandle {
    /// Spawns the actor on its own OS thread.
    pub(crate) fn spawn(
        conn: Connection,
        ports: StorePorts,
        hook: Option<Box<dyn FailpointHook>>,
    ) -> Self {
        // A bounded channel: an unbounded one would let a runaway producer
        // queue work the single writer can never drain, and the backpressure
        // is more useful than the queue.
        let (sender, receiver) = sync_channel(64);
        let thread = std::thread::Builder::new()
            .name("cg-store-writer".to_owned())
            .spawn(move || serve(conn, ports, hook, &receiver))
            .expect("spawning the store writer thread");
        Self {
            sender,
            thread: Some(thread),
        }
    }

    /// Sends one operation and waits for its typed answer.
    pub(crate) fn call<O, T>(
        &self,
        request: O::Request,
        wrap: impl FnOnce(Job<O>) -> Command,
    ) -> StoreResult<T>
    where
        O: WriteOp<Output = T>,
    {
        let (reply, answers) = sync_channel(1);
        self.sender
            .send(wrap(Job { request, reply }))
            .map_err(|_| StoreError::WriterGone)?;
        answers.recv().map_err(|_| StoreError::WriterGone)?
    }

    /// Sends one argument-free read and waits for its answer.
    pub(crate) fn query<T>(&self, wrap: impl FnOnce(Query<T>) -> Command) -> StoreResult<T> {
        let (reply, answers) = sync_channel(1);
        self.sender
            .send(wrap(Query { reply }))
            .map_err(|_| StoreError::WriterGone)?;
        answers.recv().map_err(|_| StoreError::WriterGone)?
    }

    /// Sends one read with an argument and waits for its answer.
    pub(crate) fn ask<A, T>(
        &self,
        argument: A,
        wrap: impl FnOnce(Ask<A, T>) -> Command,
    ) -> StoreResult<T> {
        let (reply, answers) = sync_channel(1);
        self.sender
            .send(wrap(Ask { argument, reply }))
            .map_err(|_| StoreError::WriterGone)?;
        answers.recv().map_err(|_| StoreError::WriterGone)?
    }
}

impl Drop for WriterHandle {
    fn drop(&mut self) {
        // Best effort: if the actor already died the send fails and the join
        // returns immediately. Either way the connection is closed on the
        // thread that owns it, never from here.
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for WriterHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriterHandle { .. }")
    }
}
