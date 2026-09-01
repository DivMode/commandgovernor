//! Mutation-command receipts: stable identity, replayed results, and the ACK
//! that is *only* a retention permit.
//!
//! This is the pure domain layer for the SQLite command journal
//! ([`docs/research/2026-08-31-durable-orchestration-pattern-review.md`],
//! "`governor-store-sqlite`"). Every daemon, IPC or MCP write carries a stable
//! `(actor, command)` identity so that a transport reconnect cannot mint a new
//! logical mutation by accident.
//!
//! ```text
//! received --(result committed)--> completed --(receipt ack)--> acked
//!    |
//!    +--(startup or retry finds no committed result)--> uncertain
//! ```
//!
//! # The two rules that matter
//!
//! - **An exact retry of a completed identity returns the recorded result.** No
//!   dispatch, no second effect.
//! - **An exact retry of a `received` or `uncertain` identity is uncertain.** It
//!   is never redispatched. [`MutationJournal::resolve`] cannot express a
//!   redispatch decision: [`MutationDisposition`] has no such variant, and
//!   uncertainty leaves through [`Conflict::MutationResultUncertain`].
//!
//! # This is ACK layer 1 of 3
//!
//! [`ReceiptAck`] is the weakest of Command Governor's three acknowledgements
//! and must stay that way:
//!
//! | Layer | Type | What it permits |
//! | --- | --- | --- |
//! | 1. mutation receipt | [`ReceiptAck`] (here) | journal retention/compaction |
//! | 2. attention claim | [`crate::claim::mint_claim`] | responsibility for an obligation |
//! | 3. semantic disposition | [`crate::claim::acknowledge`] | *closing* the obligation |
//!
//! The separation is structural, not documentary. [`ReceiptAck`] holds an
//! [`ActorId`] and a [`MutationCommandId`] and nothing else — no
//! [`crate::obligation::Disposition`], no [`crate::id::ObligationId`], no
//! [`crate::id::ClaimId`], no binding generation. It implements no `From` or
//! `Into` toward any of them, no [`crate::obligation::ObligationEvent`] variant
//! accepts one, and [`crate::claim::acknowledge`] takes a
//! [`crate::obligation::AckRequest`] that a receipt ACK cannot be turned into.
//! A receipt ACK therefore cannot close a result obligation for the same reason
//! a claim ID cannot be used as an obligation ID: they are different types with
//! no bridge.
//!
//! # Where the external side effect lives
//!
//! A mutation command is a *request identity*. The consequential external write
//! it may cause is a separate fact recorded by [`crate::effect`], because
//! command delivery and side effect have different failure modes and must be
//! able to disagree.
//!
//! [`docs/research/2026-08-31-durable-orchestration-pattern-review.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/research/2026-08-31-durable-orchestration-pattern-review.md

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::digest::absorb;
use crate::error::{Conflict, ConflictKind, Outcome, Transition};
use crate::fence::{DaemonEpoch, SafeToken};
use crate::id::{ActorId, MutationCommandId};
use crate::time::{DurationMs, Timestamp};

/// Domain-separation label for [`MutationFingerprint`].
///
/// Changing this string changes every derived fingerprint and is a protocol
/// break for the command journal.
pub const MUTATION_FINGERPRINT_DOMAIN: &str = "command-governor/mutation-fingerprint/v1";

/// What kind of mutation a command asks for.
///
/// A [`SafeToken`] rather than a closed enum: `governor-core` must not have to
/// enumerate every daemon, IPC and MCP write before those surfaces exist, and
/// the charset already rules out routing a payload through it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationCommandKind(SafeToken);

impl MutationCommandKind {
    /// Wraps the opaque command-kind label.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the label, for persistence and diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

impl core::fmt::Display for MutationCommandKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.0, f)
    }
}

/// Digest binding a command identity to the operation it actually asked for.
///
/// # Why this exists
///
/// "Exact retry" has to mean exact. Without a fingerprint, a client that reuses
/// a [`MutationCommandId`] for a *different* operation would silently receive
/// the first operation's recorded result — deduplication turning into a wrong
/// answer. The fingerprint makes that mismatch a typed
/// [`Conflict::MutationCommandMismatch`] instead.
///
/// It is a digest, not the parameters themselves: the journal records that the
/// operation was the same one, never what it contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationFingerprint([u8; 32]);

impl MutationFingerprint {
    /// Derives the fingerprint from the command kind and its fenced parameters.
    ///
    /// The pre-image is domain-separated and every part is length-prefixed, so
    /// no two distinct parameter lists can encode to the same byte string.
    #[must_use]
    pub fn derive(kind: &MutationCommandKind, parameters: &[&SafeToken]) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, MUTATION_FINGERPRINT_DOMAIN.as_bytes());
        absorb(&mut hasher, kind.as_token().as_str().as_bytes());
        let count = u64::try_from(parameters.len()).expect("parameter count fits in u64");
        hasher.update(count.to_be_bytes());
        for parameter in parameters {
            absorb(&mut hasher, parameter.as_str().as_bytes());
        }
        Self(hasher.finalize().into())
    }

    /// Wraps a fingerprint the store already computed.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes, for persistence.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The bounded result a completed mutation command may record.
///
/// Deliberately shaped, not free-form: a durable journal that could hold an
/// arbitrary response body would be a place for prompts, tool output and
/// credentials to accumulate. A result is one of three shapes, and the only
/// variable part is a single opaque [`SafeToken`] reference.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SafeMutationResult {
    /// The mutation applied, optionally yielding one opaque reference the
    /// caller can use to address what it created or changed.
    Applied {
        /// Opaque reference to the affected object, when there is one.
        reference: Option<SafeToken>,
    },
    /// The mutation was a no-op: the target was already in the requested state.
    AlreadySatisfied,
    /// The mutation was refused by a typed domain conflict.
    Refused {
        /// The stable classification the caller was given.
        conflict: ConflictKind,
    },
}

impl SafeMutationResult {
    /// Stable `snake_case` result kind, for the journal's `safe_result_kind`.
    #[must_use]
    pub const fn kind_code(&self) -> &'static str {
        match self {
            Self::Applied { .. } => "applied",
            Self::AlreadySatisfied => "already_satisfied",
            Self::Refused { .. } => "refused",
        }
    }
}

/// Journal status of one mutation command identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MutationCommandStatus {
    /// Committed before dispatch. No result is durable yet.
    Received,
    /// A safe result is committed. An exact retry replays it.
    Completed,
    /// Recorded as having no committed result after a restart or a retry.
    /// It is never redispatched automatically.
    Uncertain,
    /// The client confirmed receipt of the committed result. Retention only.
    Acked,
}

impl MutationCommandStatus {
    /// Stable `snake_case` code for storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Completed => "completed",
            Self::Uncertain => "uncertain",
            Self::Acked => "acked",
        }
    }

    /// Reports whether a safe result is committed for this identity.
    #[must_use]
    pub const fn has_committed_result(self) -> bool {
        matches!(self, Self::Completed | Self::Acked)
    }
}

/// ACK layer 1: the client confirms it received a committed mutation result.
///
/// # What this type deliberately cannot do
///
/// It carries an actor and a command identity and nothing else. It has no
/// disposition, no obligation, no claim, and no binding generation, and there
/// is no conversion from it to any of those. Its single effect is
/// [`CompactionEligibility`]. Closing engineering work is
/// [`crate::claim::acknowledge`]'s job and stays there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptAck {
    actor: ActorId,
    command: MutationCommandId,
    at: Timestamp,
}

impl ReceiptAck {
    /// Records that the client confirmed receipt of a committed result.
    #[must_use]
    pub const fn new(actor: ActorId, command: MutationCommandId, at: Timestamp) -> Self {
        Self { actor, command, at }
    }

    /// Actor that acknowledged.
    #[must_use]
    pub const fn actor(&self) -> ActorId {
        self.actor
    }

    /// Command whose result was acknowledged.
    #[must_use]
    pub const fn command(&self) -> MutationCommandId {
        self.command
    }

    /// Instant of the acknowledgement.
    #[must_use]
    pub const fn at(&self) -> Timestamp {
        self.at
    }
}

/// Whether a journal row may be compacted away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompactionEligibility {
    /// The row must be kept: no receipt ACK, or the policy age has not elapsed.
    Retained,
    /// Acked and old enough. The journal may drop the row.
    Eligible,
}

/// An event applied to a mutation command journal row.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MutationCommandEvent {
    /// A safe result was committed. This must happen before the reply is sent.
    ResultCommitted {
        /// The bounded result to record.
        result: SafeMutationResult,
        /// Observation instant.
        at: Timestamp,
    },
    /// Startup recovery or a retry observed `received` with no committed
    /// result. This records the uncertainty; it never dispatches anything.
    MarkedUncertain {
        /// Observation instant.
        at: Timestamp,
    },
    /// The client confirmed receipt of the committed result.
    ReceiptAcknowledged(ReceiptAck),
}

impl MutationCommandEvent {
    const fn label(&self) -> &'static str {
        match self {
            Self::ResultCommitted { .. } => "result_committed",
            Self::MarkedUncertain { .. } => "marked_uncertain",
            Self::ReceiptAcknowledged(_) => "receipt_acknowledged",
        }
    }
}

/// One row of the durable mutation-command journal.
///
/// # Construction is replay
///
/// The only constructor is [`Self::received`], which always produces
/// [`MutationCommandStatus::Received`]. Every later status comes from folding
/// [`MutationCommandEvent`]s, so the store rebuilds a row exactly as the daemon
/// built it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationCommand {
    actor: ActorId,
    id: MutationCommandId,
    kind: MutationCommandKind,
    fingerprint: MutationFingerprint,
    daemon_epoch: DaemonEpoch,
    status: MutationCommandStatus,
    result: Option<SafeMutationResult>,
    received_at: Timestamp,
    completed_at: Option<Timestamp>,
    uncertain_at: Option<Timestamp>,
    acked_at: Option<Timestamp>,
}

impl MutationCommand {
    /// Records a command as `received`.
    ///
    /// The store commits this row in its own transaction **before** the
    /// mutation is dispatched. That ordering is what makes a crash detectable
    /// as uncertainty rather than invisible.
    #[must_use]
    pub const fn received(
        actor: ActorId,
        id: MutationCommandId,
        kind: MutationCommandKind,
        fingerprint: MutationFingerprint,
        daemon_epoch: DaemonEpoch,
        at: Timestamp,
    ) -> Self {
        Self {
            actor,
            id,
            kind,
            fingerprint,
            daemon_epoch,
            status: MutationCommandStatus::Received,
            result: None,
            received_at: at,
            completed_at: None,
            uncertain_at: None,
            acked_at: None,
        }
    }

    /// Actor that issued the command.
    #[must_use]
    pub const fn actor(&self) -> ActorId {
        self.actor
    }

    /// Stable command identity.
    #[must_use]
    pub const fn id(&self) -> MutationCommandId {
        self.id
    }

    /// What kind of mutation was asked for.
    #[must_use]
    pub const fn kind(&self) -> &MutationCommandKind {
        &self.kind
    }

    /// Fingerprint of the operation this identity was minted for.
    #[must_use]
    pub const fn fingerprint(&self) -> MutationFingerprint {
        self.fingerprint
    }

    /// Daemon epoch the command was received under.
    #[must_use]
    pub const fn daemon_epoch(&self) -> DaemonEpoch {
        self.daemon_epoch
    }

    /// Current journal status.
    #[must_use]
    pub const fn status(&self) -> MutationCommandStatus {
        self.status
    }

    /// The committed safe result, if one exists.
    #[must_use]
    pub const fn result(&self) -> Option<&SafeMutationResult> {
        self.result.as_ref()
    }

    /// Instant the command was recorded as received.
    #[must_use]
    pub const fn received_at(&self) -> Timestamp {
        self.received_at
    }

    /// Instant a safe result was committed, if one was.
    #[must_use]
    pub const fn completed_at(&self) -> Option<Timestamp> {
        self.completed_at
    }

    /// Instant the command was recorded as uncertain, if it was.
    #[must_use]
    pub const fn uncertain_at(&self) -> Option<Timestamp> {
        self.uncertain_at
    }

    /// Instant the receipt was acknowledged, if it was.
    #[must_use]
    pub const fn acked_at(&self) -> Option<Timestamp> {
        self.acked_at
    }

    /// Whether this row may be compacted away at `now`.
    ///
    /// Retention needs both halves: the receipt ACK *and* the policy age. An
    /// ACK alone is not a licence to forget.
    #[must_use]
    pub fn compaction_eligibility(
        &self,
        now: Timestamp,
        min_age: DurationMs,
    ) -> CompactionEligibility {
        match self.acked_at {
            Some(acked) if now.saturating_elapsed_since(acked) >= min_age => {
                CompactionEligibility::Eligible
            }
            _ => CompactionEligibility::Retained,
        }
    }

    /// Applies an event, returning a new row or a typed conflict.
    ///
    /// # Errors
    ///
    /// - [`Conflict::IllegalMutationTransition`] for an event the status does
    ///   not accept;
    /// - [`Conflict::MutationNotCompleted`] when a receipt ACK is presented for
    ///   a row with no committed result;
    /// - [`Conflict::MutationCommandMismatch`] when an ACK names a different
    ///   actor or command.
    ///
    /// Every one of these leaves the row untouched.
    pub fn apply(&self, event: &MutationCommandEvent) -> Outcome<Self> {
        match event {
            MutationCommandEvent::ResultCommitted { result, at } => {
                self.commit_result(result, *at, event.label())
            }
            MutationCommandEvent::MarkedUncertain { at } => self.mark_uncertain(*at, event.label()),
            MutationCommandEvent::ReceiptAcknowledged(ack) => self.acknowledge_receipt(ack),
        }
    }

    fn commit_result(
        &self,
        result: &SafeMutationResult,
        at: Timestamp,
        label: &'static str,
    ) -> Outcome<Self> {
        match self.status {
            // `uncertain` is a *finding*, not a verdict: late evidence that the
            // mutation did commit is exactly what resolves it, and it resolves
            // it without dispatching anything.
            MutationCommandStatus::Received | MutationCommandStatus::Uncertain => {
                let mut next = self.clone();
                next.status = MutationCommandStatus::Completed;
                next.result = Some(result.clone());
                next.completed_at = Some(at);
                Ok(Transition::Advanced(next))
            }
            MutationCommandStatus::Completed | MutationCommandStatus::Acked
                if self.result.as_ref() == Some(result) =>
            {
                Ok(Transition::Duplicate)
            }
            from => Err(Conflict::IllegalMutationTransition { from, event: label }),
        }
    }

    fn mark_uncertain(&self, at: Timestamp, label: &'static str) -> Outcome<Self> {
        match self.status {
            MutationCommandStatus::Received => {
                let mut next = self.clone();
                next.status = MutationCommandStatus::Uncertain;
                next.uncertain_at = Some(at);
                Ok(Transition::Advanced(next))
            }
            MutationCommandStatus::Uncertain => Ok(Transition::Duplicate),
            from => Err(Conflict::IllegalMutationTransition { from, event: label }),
        }
    }

    fn acknowledge_receipt(&self, ack: &ReceiptAck) -> Outcome<Self> {
        if ack.actor != self.actor || ack.command != self.id {
            return Err(Conflict::MutationCommandMismatch {
                actor: ack.actor,
                command: ack.command,
            });
        }
        match self.status {
            MutationCommandStatus::Completed => {
                let mut next = self.clone();
                next.status = MutationCommandStatus::Acked;
                next.acked_at = Some(ack.at);
                Ok(Transition::Advanced(next))
            }
            MutationCommandStatus::Acked => Ok(Transition::Duplicate),
            // There is nothing to have received: an ACK cannot conjure a result
            // and must never be mistaken for one.
            MutationCommandStatus::Received | MutationCommandStatus::Uncertain => {
                Err(Conflict::MutationNotCompleted {
                    actor: self.actor,
                    command: self.id,
                })
            }
        }
    }
}

/// What a retry of a mutation identity resolves to.
///
/// # There is no `Dispatch` variant
///
/// That absence is the point. A caller resolving an identity can be told the
/// identity is new — in which case it must go through the normal policy and
/// fencing path — or handed the recorded result. It can never be told to
/// redispatch a command whose fate is unknown; that answer leaves through
/// [`Conflict::MutationResultUncertain`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationDisposition {
    /// The journal has never seen this identity. It is a genuinely new
    /// operation and must pass normal policy and fencing; it is *not*
    /// deduplicated against anything.
    NewOperation,
    /// An exact retry of a completed identity. This is the recorded result and
    /// no dispatch may follow.
    RecordedResult(SafeMutationResult),
}

/// Pure index of mutation command identities, keyed `(actor, command)`.
///
/// The durable enforcement is the journal's
/// `PRIMARY KEY(actor_id, command_id)`. This type is the same rule at the pure
/// level, so retry and restart behaviour can be proven without a database, and
/// [`crate::fence::SourceLedger`] is its direct sibling for source events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationJournal {
    rows: BTreeMap<(ActorId, MutationCommandId), MutationCommand>,
}

impl MutationJournal {
    /// Creates an empty journal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of recorded identities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Reports whether the journal holds no identities.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row for one identity, if it has been recorded.
    #[must_use]
    pub fn get(&self, actor: ActorId, command: MutationCommandId) -> Option<&MutationCommand> {
        self.rows.get(&(actor, command))
    }

    /// Records or replaces the row for one identity.
    ///
    /// The store calls this after each committed journal write, and replay
    /// calls it while folding history.
    pub fn record(&mut self, command: MutationCommand) {
        self.rows.insert((command.actor, command.id), command);
    }

    /// Resolves an incoming mutation identity.
    ///
    /// # Errors
    ///
    /// - [`Conflict::MutationCommandMismatch`] when the identity was minted for
    ///   a different operation, so this is not an *exact* retry;
    /// - [`Conflict::MutationResultUncertain`] when the identity is `received`
    ///   or `uncertain`. The caller must surface the uncertainty; it must not
    ///   redispatch, and this function offers it no way to.
    pub fn resolve(
        &self,
        actor: ActorId,
        command: MutationCommandId,
        fingerprint: MutationFingerprint,
    ) -> Result<MutationDisposition, Conflict> {
        let Some(row) = self.get(actor, command) else {
            return Ok(MutationDisposition::NewOperation);
        };
        if row.fingerprint != fingerprint {
            return Err(Conflict::MutationCommandMismatch { actor, command });
        }
        match row.status {
            MutationCommandStatus::Completed | MutationCommandStatus::Acked => {
                let result = row
                    .result
                    .clone()
                    .expect("a completed row always carries its safe result");
                Ok(MutationDisposition::RecordedResult(result))
            }
            MutationCommandStatus::Received | MutationCommandStatus::Uncertain => {
                Err(Conflict::MutationResultUncertain { actor, command })
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{MutationCommandKind, MutationFingerprint};
    use crate::fence::SafeToken;
    use crate::id::{ActorId, MutationCommandId};
    use uuid::Uuid;

    pub(crate) fn actor(n: u128) -> ActorId {
        ActorId::from_uuid(Uuid::from_u128(n))
    }

    pub(crate) fn command(n: u128) -> MutationCommandId {
        MutationCommandId::from_uuid(Uuid::from_u128(n))
    }

    pub(crate) fn kind(value: &str) -> MutationCommandKind {
        MutationCommandKind::new(SafeToken::new(value).expect("fixture kinds are safe"))
    }

    pub(crate) fn fingerprint(kind_label: &str, parameter: &str) -> MutationFingerprint {
        let parameter = SafeToken::new(parameter).expect("fixture parameters are safe");
        MutationFingerprint::derive(&kind(kind_label), &[&parameter])
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{actor, command, fingerprint, kind};
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn received() -> MutationCommand {
        MutationCommand::received(
            actor(1),
            command(9),
            kind("worker.resume"),
            fingerprint("worker.resume", "turn-7"),
            DaemonEpoch::FIRST,
            at(10),
        )
    }

    fn completed() -> MutationCommand {
        received()
            .apply(&MutationCommandEvent::ResultCommitted {
                result: SafeMutationResult::Applied { reference: None },
                at: at(11),
            })
            .expect("committing a result is legal from received")
            .advanced()
            .expect("committing advances")
    }

    #[test]
    fn a_completed_identity_replays_its_recorded_result() {
        let mut journal = MutationJournal::new();
        journal.record(completed());
        let disposition = journal
            .resolve(actor(1), command(9), fingerprint("worker.resume", "turn-7"))
            .expect("an exact retry of a completed identity resolves");
        assert_eq!(
            disposition,
            MutationDisposition::RecordedResult(SafeMutationResult::Applied { reference: None })
        );
    }

    #[test]
    fn a_pending_identity_is_uncertain_and_offers_no_dispatch() {
        let mut journal = MutationJournal::new();
        journal.record(received());
        let err = journal
            .resolve(actor(1), command(9), fingerprint("worker.resume", "turn-7"))
            .expect_err("a received identity with no result is uncertain");
        assert_eq!(err.code(), "mutation_result_uncertain");

        // And after startup recovery marks it, the answer does not change.
        let mut journal = MutationJournal::new();
        journal.record(
            received()
                .apply(&MutationCommandEvent::MarkedUncertain { at: at(50) })
                .unwrap()
                .advanced()
                .unwrap(),
        );
        let err = journal
            .resolve(actor(1), command(9), fingerprint("worker.resume", "turn-7"))
            .unwrap_err();
        assert_eq!(err.code(), "mutation_result_uncertain");
    }

    #[test]
    fn a_different_command_id_is_a_new_operation() {
        let mut journal = MutationJournal::new();
        journal.record(completed());
        let disposition = journal
            .resolve(
                actor(1),
                command(10),
                fingerprint("worker.resume", "turn-7"),
            )
            .expect("an unseen identity resolves");
        assert_eq!(
            disposition,
            MutationDisposition::NewOperation,
            "a new identity is never deduplicated against an old one"
        );

        // The same command id under a different actor is also new.
        let disposition = journal
            .resolve(actor(2), command(9), fingerprint("worker.resume", "turn-7"))
            .unwrap();
        assert_eq!(disposition, MutationDisposition::NewOperation);
    }

    #[test]
    fn reusing_an_identity_for_another_operation_is_refused() {
        let mut journal = MutationJournal::new();
        journal.record(completed());
        let err = journal
            .resolve(actor(1), command(9), fingerprint("worker.resume", "turn-8"))
            .expect_err("this is not an exact retry");
        assert_eq!(err.code(), "mutation_command_mismatch");
    }

    #[test]
    fn a_receipt_ack_requires_a_committed_result() {
        let err = received()
            .apply(&MutationCommandEvent::ReceiptAcknowledged(ReceiptAck::new(
                actor(1),
                command(9),
                at(12),
            )))
            .expect_err("there is no result to have received");
        assert_eq!(err.code(), "mutation_not_completed");
    }

    #[test]
    fn a_receipt_ack_only_unlocks_retention() {
        let row = completed();
        assert_eq!(
            row.compaction_eligibility(at(1_000_000), DurationMs::from_millis(0)),
            CompactionEligibility::Retained,
            "an unacked row is kept regardless of age"
        );

        let acked = row
            .apply(&MutationCommandEvent::ReceiptAcknowledged(ReceiptAck::new(
                actor(1),
                command(9),
                at(12),
            )))
            .expect("acking a completed row is legal")
            .advanced()
            .unwrap();
        assert_eq!(acked.status(), MutationCommandStatus::Acked);
        assert_eq!(
            acked.compaction_eligibility(at(13), DurationMs::from_millis(1_000)),
            CompactionEligibility::Retained,
            "the policy age has not elapsed"
        );
        assert_eq!(
            acked.compaction_eligibility(at(2_000), DurationMs::from_millis(1_000)),
            CompactionEligibility::Eligible
        );

        // An acked identity still replays exactly the same recorded result.
        let mut journal = MutationJournal::new();
        journal.record(acked);
        assert_eq!(
            journal
                .resolve(actor(1), command(9), fingerprint("worker.resume", "turn-7"))
                .unwrap(),
            MutationDisposition::RecordedResult(SafeMutationResult::Applied { reference: None })
        );
    }

    #[test]
    fn a_receipt_ack_from_another_identity_is_refused() {
        let err = completed()
            .apply(&MutationCommandEvent::ReceiptAcknowledged(ReceiptAck::new(
                actor(2),
                command(9),
                at(12),
            )))
            .expect_err("an ACK is bound to its own identity");
        assert_eq!(err.code(), "mutation_command_mismatch");
    }

    #[test]
    fn late_evidence_resolves_an_uncertain_row_without_dispatching() {
        let uncertain = received()
            .apply(&MutationCommandEvent::MarkedUncertain { at: at(50) })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(uncertain.status(), MutationCommandStatus::Uncertain);
        let resolved = uncertain
            .apply(&MutationCommandEvent::ResultCommitted {
                result: SafeMutationResult::AlreadySatisfied,
                at: at(60),
            })
            .expect("committed evidence resolves uncertainty")
            .advanced()
            .unwrap();
        assert_eq!(resolved.status(), MutationCommandStatus::Completed);
        assert_eq!(uncertain.status(), MutationCommandStatus::Uncertain);
    }

    #[test]
    fn an_uncertain_marking_cannot_undo_a_committed_result() {
        let err = completed()
            .apply(&MutationCommandEvent::MarkedUncertain { at: at(50) })
            .expect_err("a committed result is not made uncertain later");
        assert_eq!(err.code(), "illegal_mutation_transition");
    }

    #[test]
    fn a_second_different_result_is_refused() {
        let err = completed()
            .apply(&MutationCommandEvent::ResultCommitted {
                result: SafeMutationResult::Refused {
                    conflict: ConflictKind::StaleClaim,
                },
                at: at(20),
            })
            .expect_err("one identity commits one result");
        assert_eq!(err.code(), "illegal_mutation_transition");
    }

    #[test]
    fn an_exact_repeat_of_the_committed_result_is_idempotent() {
        assert!(
            completed()
                .apply(&MutationCommandEvent::ResultCommitted {
                    result: SafeMutationResult::Applied { reference: None },
                    at: at(11),
                })
                .unwrap()
                .is_duplicate()
        );
    }

    #[test]
    fn fingerprints_are_deterministic_and_separate_distinct_operations() {
        assert_eq!(
            fingerprint("worker.resume", "turn-7"),
            fingerprint("worker.resume", "turn-7")
        );
        assert_ne!(
            fingerprint("worker.resume", "turn-7"),
            fingerprint("worker.resume", "turn-8")
        );
        assert_ne!(
            fingerprint("worker.resume", "turn-7"),
            fingerprint("worker.cancel", "turn-7")
        );
        // Length prefixing keeps concatenations from colliding.
        let a = SafeToken::new("ab").unwrap();
        let b = SafeToken::new("c").unwrap();
        let c = SafeToken::new("a").unwrap();
        let d = SafeToken::new("bc").unwrap();
        assert_ne!(
            MutationFingerprint::derive(&kind("k"), &[&a, &b]),
            MutationFingerprint::derive(&kind("k"), &[&c, &d])
        );
    }

    #[test]
    fn status_and_result_codes_are_stable() {
        assert_eq!(MutationCommandStatus::Received.code(), "received");
        assert_eq!(MutationCommandStatus::Completed.code(), "completed");
        assert_eq!(MutationCommandStatus::Uncertain.code(), "uncertain");
        assert_eq!(MutationCommandStatus::Acked.code(), "acked");
        assert_eq!(
            SafeMutationResult::Applied { reference: None }.kind_code(),
            "applied"
        );
        assert_eq!(
            SafeMutationResult::AlreadySatisfied.kind_code(),
            "already_satisfied"
        );
        assert_eq!(
            SafeMutationResult::Refused {
                conflict: ConflictKind::ExpiredClaim
            }
            .kind_code(),
            "refused"
        );
    }

    #[test]
    fn the_journal_indexes_by_the_full_identity() {
        let mut journal = MutationJournal::new();
        assert!(journal.is_empty());
        journal.record(received());
        journal.record(MutationCommand::received(
            actor(2),
            command(9),
            kind("worker.resume"),
            fingerprint("worker.resume", "turn-7"),
            DaemonEpoch::FIRST,
            at(10),
        ));
        assert_eq!(journal.len(), 2, "actor is part of the identity");
        assert!(journal.get(actor(1), command(9)).is_some());
        assert!(journal.get(actor(3), command(9)).is_none());
    }
}
