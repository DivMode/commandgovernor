//! The mutation-command receipt journal.
//!
//! One authority: this is the Prime-Agent-style command journal from
//! `docs/research/2026-08-31-durable-orchestration-pattern-review.md`,
//! implemented as SQLite transactions rather than a second log file.
//!
//! ```text
//! BEGIN IMMEDIATE
//!   insert unique (actor_id, command_id, received)
//! COMMIT
//! -> only now may the caller dispatch consequential I/O
//! -> commit the completed safe result before replying
//! ```
//!
//! The three retry rules, and where each one is enforced:
//!
//! | Rule | Enforced by |
//! | --- | --- |
//! | exact retry of `completed` returns the recorded result, zero dispatch | [`MutationJournal::resolve`], surfaced as [`MutationAdmission::Replayed`] |
//! | retry of `received`/`uncertain` is typed uncertainty, zero dispatch | [`Conflict::MutationResultUncertain`]; [`MutationAdmission`] has no dispatch-anyway variant |
//! | a different identity is a genuinely new operation | [`MutationAdmission::Dispatch`], after the normal fencing path |
//!
//! A *fourth* rule is this store's, beyond the research doc's conceptual table:
//! an identity retried with a **different fingerprint** is
//! [`Conflict::MutationCommandMismatch`], never a replayed result. Without it,
//! reusing a command id for a different operation would silently return the
//! first operation's answer.
//!
//! # ACK here is layer 1 and only layer 1
//!
//! [`AckMutationReceipt`] moves a row to `acked`, which makes it eligible for
//! retention policy. It reaches no obligation: it takes an actor and a command
//! identity, [`governor_core::mutation::ReceiptAck`] carries nothing else, and
//! there is no code path from this module to
//! [`crate::ops::claim::AcknowledgeObligation`].

use governor_core::error::Conflict;
use governor_core::fence::DaemonEpoch;
use governor_core::id::{ActorId, MutationCommandId};
use governor_core::mutation::{
    MutationCommand, MutationCommandEvent, MutationCommandKind, MutationCommandStatus,
    MutationDisposition, MutationFingerprint, MutationJournal, ReceiptAck, SafeMutationResult,
};
use governor_core::time::Timestamp;
use rusqlite::{OptionalExtension as _, params};

use crate::codec::{
    decode_mutation_result, decode_mutation_status, encode_mutation_result, encode_mutation_status,
    hex32, id_text, parse_hex32, parse_token, parse_u64, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::ports::StorePorts;
use crate::tx::{Failpoint, Tx, WriteOp};

const TABLE: &str = "mutation_commands";

/// Opening one logical mutation before anything consequential is dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginMutationRequest {
    /// Semantic principal issuing the mutation. Survives transport reconnect.
    pub actor: ActorId,
    /// Stable identity of this logical mutation.
    pub command: MutationCommandId,
    /// What kind of mutation is being asked for.
    pub kind: MutationCommandKind,
    /// Digest binding the identity to the operation it was minted for.
    pub fingerprint: MutationFingerprint,
    /// Daemon epoch the command is received under.
    pub daemon_epoch: DaemonEpoch,
}

/// What the journal permits for one incoming mutation identity.
///
/// # There is no dispatch-anyway variant
///
/// A caller can be told to dispatch a genuinely new operation, or handed a
/// recorded result. It can never be told to redispatch a command whose fate is
/// unknown: that answer leaves as [`Conflict::MutationResultUncertain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationAdmission {
    /// A new identity. The `received` row is committed; dispatch may follow.
    Dispatch,
    /// An exact retry of a completed identity. No dispatch may follow.
    Replayed(SafeMutationResult),
}

/// Commits the `received` row that must precede any consequential dispatch.
pub(crate) struct BeginMutation {
    request: BeginMutationRequest,
    now: Timestamp,
}

impl WriteOp for BeginMutation {
    type Request = BeginMutationRequest;
    type Committed = MutationAdmission;
    type Output = MutationAdmission;

    const NAME: &'static str = "begin_mutation";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let mut journal = MutationJournal::new();
        if let Some(existing) = read(tx, self.request.actor, self.request.command)? {
            journal.record(existing);
        }
        match journal.resolve(
            self.request.actor,
            self.request.command,
            self.request.fingerprint,
        )? {
            MutationDisposition::RecordedResult(result) => {
                // Zero dispatch, zero write: the answer already exists.
                Ok(MutationAdmission::Replayed(result))
            }
            MutationDisposition::NewOperation => {
                let row = MutationCommand::received(
                    self.request.actor,
                    self.request.command,
                    self.request.kind.clone(),
                    self.request.fingerprint,
                    self.request.daemon_epoch,
                    self.now,
                );
                insert(tx, &row)?;
                tx.reach(Failpoint::AfterMutationReceived)?;
                tx.reach(Failpoint::BeforeCommit)?;
                Ok(MutationAdmission::Dispatch)
            }
        }
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Committing the bounded safe result, before the reply is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteMutationRequest {
    /// Actor that issued the command.
    pub actor: ActorId,
    /// Command identity being completed.
    pub command: MutationCommandId,
    /// Fingerprint of the operation, re-checked against the recorded one.
    pub fingerprint: MutationFingerprint,
    /// The bounded result to record.
    pub result: SafeMutationResult,
}

/// Commits a mutation's safe result.
///
/// A `received` row completes normally. An `uncertain` row may *also* complete,
/// and only this way: late proven evidence that the mutation did commit is
/// exactly what resolves uncertainty, and resolving it dispatches nothing —
/// there is no redispatch anywhere in this module.
pub(crate) struct CompleteMutation {
    request: CompleteMutationRequest,
    now: Timestamp,
}

impl WriteOp for CompleteMutation {
    type Request = CompleteMutationRequest;
    type Committed = MutationCommandStatus;
    type Output = MutationCommandStatus;

    const NAME: &'static str = "complete_mutation";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let row = require(tx, self.request.actor, self.request.command)?;
        if row.fingerprint() != self.request.fingerprint {
            return Err(Conflict::MutationCommandMismatch {
                actor: self.request.actor,
                command: self.request.command,
            }
            .into());
        }
        let transition = row.apply(&MutationCommandEvent::ResultCommitted {
            result: self.request.result.clone(),
            at: self.now,
        })?;
        let Some(next) = transition.advanced() else {
            return Ok(row.status());
        };
        write_result(tx, &next)?;
        tx.reach(Failpoint::AfterMutationResult)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(next.status())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// ACK layer 1: the client confirms receipt of a committed result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckMutationReceiptRequest {
    /// Actor acknowledging.
    pub actor: ActorId,
    /// Command whose committed result was received.
    pub command: MutationCommandId,
}

/// Marks a completed command acked. Retention eligibility, nothing else.
pub(crate) struct AckMutationReceipt {
    request: AckMutationReceiptRequest,
    now: Timestamp,
}

impl WriteOp for AckMutationReceipt {
    type Request = AckMutationReceiptRequest;
    type Committed = MutationCommandStatus;
    type Output = MutationCommandStatus;

    const NAME: &'static str = "ack_mutation_receipt";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let row = require(tx, self.request.actor, self.request.command)?;
        let transition = row.apply(&MutationCommandEvent::ReceiptAcknowledged(ReceiptAck::new(
            self.request.actor,
            self.request.command,
            self.now,
        )))?;
        let Some(next) = transition.advanced() else {
            return Ok(row.status());
        };
        tx.conn().execute(
            "UPDATE mutation_commands SET status = ?3, acked_at_ms = ?4
              WHERE actor_id = ?1 AND command_id = ?2",
            params![
                id_text(self.request.actor),
                id_text(self.request.command),
                encode_mutation_status(next.status()),
                next.acked_at().map(store_time),
            ],
        )?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(next.status())
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

// --- Row access -------------------------------------------------------------

fn insert(tx: &Tx<'_>, row: &MutationCommand) -> StoreResult<()> {
    tx.conn().execute(
        "INSERT INTO mutation_commands (actor_id, command_id, fingerprint, command_kind,
                status, daemon_epoch, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id_text(row.actor()),
            id_text(row.id()),
            hex32(row.fingerprint().as_bytes()),
            row.kind().as_token().as_str(),
            encode_mutation_status(row.status()),
            store_u64(row.daemon_epoch().get(), TABLE, "daemon_epoch")?,
            store_time(row.received_at()),
        ],
    )?;
    Ok(())
}

fn write_result(tx: &Tx<'_>, row: &MutationCommand) -> StoreResult<()> {
    let result = row.result().ok_or_else(|| {
        CorruptValue::new(TABLE, "safe_result_kind", CorruptReason::MalformedMetadata)
    })?;
    let stored = encode_mutation_result(result)?;
    tx.conn().execute(
        "UPDATE mutation_commands
            SET status = ?3, safe_result_kind = ?4, safe_result_ref = ?5,
                safe_result_conflict = ?6, completed_at_ms = ?7, uncertain_at_ms = ?8
          WHERE actor_id = ?1 AND command_id = ?2",
        params![
            id_text(row.actor()),
            id_text(row.id()),
            encode_mutation_status(row.status()),
            stored.kind,
            stored.reference,
            stored.conflict,
            row.completed_at().map(store_time),
            row.uncertain_at().map(store_time),
        ],
    )?;
    Ok(())
}

/// Marks a `received` row uncertain. Used only by startup recovery.
pub(crate) fn mark_uncertain(tx: &Tx<'_>, row: &MutationCommand, at: Timestamp) -> StoreResult<()> {
    let next = row
        .apply(&MutationCommandEvent::MarkedUncertain { at })?
        .or_unchanged(row.clone());
    tx.conn().execute(
        "UPDATE mutation_commands SET status = ?3, uncertain_at_ms = ?4
          WHERE actor_id = ?1 AND command_id = ?2",
        params![
            id_text(next.actor()),
            id_text(next.id()),
            encode_mutation_status(next.status()),
            next.uncertain_at().map(store_time),
        ],
    )?;
    Ok(())
}

fn require(
    tx: &Tx<'_>,
    actor: ActorId,
    command: MutationCommandId,
) -> StoreResult<MutationCommand> {
    read(tx, actor, command)?.ok_or_else(|| {
        // Nothing to complete or acknowledge. Reported as uncertainty rather
        // than as a missing row: from the caller's side the two are the same
        // situation, and neither permits a dispatch.
        Conflict::MutationResultUncertain { actor, command }.into()
    })
}

/// Reads and re-proves one journal row.
///
/// The row is rebuilt by folding its own recorded history through
/// [`MutationCommand`] — `received`, then any uncertainty, then any result,
/// then any ACK — and the folded status must equal the stored one. A row whose
/// columns cannot be reached by a legal sequence of transitions is corrupt and
/// fails closed rather than being trusted.
pub(crate) fn read(
    tx: &Tx<'_>,
    actor: ActorId,
    command: MutationCommandId,
) -> StoreResult<Option<MutationCommand>> {
    type Row = (
        String,
        String,
        String,
        i64,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let row: Option<Row> = tx
        .conn()
        .query_row(
            "SELECT fingerprint, command_kind, status, daemon_epoch, created_at_ms,
                    safe_result_kind, safe_result_ref, safe_result_conflict,
                    completed_at_ms, uncertain_at_ms, acked_at_ms
               FROM mutation_commands WHERE actor_id = ?1 AND command_id = ?2",
            params![id_text(actor), id_text(command)],
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
    let Some((
        fingerprint,
        kind,
        status,
        epoch,
        created,
        result_kind,
        result_ref,
        result_conflict,
        completed,
        uncertain,
        acked,
    )) = row
    else {
        return Ok(None);
    };

    let stored_status = decode_mutation_status(&status, TABLE)?;
    let mut folded = MutationCommand::received(
        actor,
        command,
        MutationCommandKind::new(parse_token(&kind, TABLE, "command_kind")?),
        MutationFingerprint::from_bytes(parse_hex32(&fingerprint, TABLE, "fingerprint")?),
        DaemonEpoch::new(parse_u64(epoch, TABLE, "daemon_epoch")?),
        Timestamp::from_unix_millis(created),
    );
    if let Some(at) = uncertain {
        folded = folded
            .apply(&MutationCommandEvent::MarkedUncertain {
                at: Timestamp::from_unix_millis(at),
            })?
            .or_unchanged(folded);
    }
    if let Some(at) = completed {
        let result = decode_mutation_result(
            result_kind.as_deref().unwrap_or_default(),
            result_ref.as_deref(),
            result_conflict.as_deref(),
        )?;
        folded = folded
            .apply(&MutationCommandEvent::ResultCommitted {
                result,
                at: Timestamp::from_unix_millis(at),
            })?
            .or_unchanged(folded);
    }
    if let Some(at) = acked {
        folded = folded
            .apply(&MutationCommandEvent::ReceiptAcknowledged(ReceiptAck::new(
                actor,
                command,
                Timestamp::from_unix_millis(at),
            )))?
            .or_unchanged(folded);
    }
    if folded.status() != stored_status {
        return Err(CorruptValue::new(TABLE, "status", CorruptReason::UnprovableEvidence).into());
    }
    Ok(Some(folded))
}

/// Every row from an older daemon epoch that is still `received`.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an unfoldable row.
pub(crate) fn unresolved_before(
    tx: &Tx<'_>,
    epoch: DaemonEpoch,
) -> StoreResult<Vec<MutationCommand>> {
    let mut statement = tx.conn().prepare(
        "SELECT actor_id, command_id FROM mutation_commands
          WHERE status = 'received' AND daemon_epoch < ?1",
    )?;
    let rows = statement.query_map(
        params![store_u64(epoch.get(), TABLE, "daemon_epoch")?],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (actor, command) = row?;
        let actor = crate::codec::parse_id(&actor, TABLE, "actor_id")?;
        let command = crate::codec::parse_id(&command, TABLE, "command_id")?;
        if let Some(found) = read(tx, actor, command)? {
            out.push(found);
        }
    }
    Ok(out)
}
