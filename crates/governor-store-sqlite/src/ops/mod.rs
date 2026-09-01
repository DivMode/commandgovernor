//! The durable write operations, one module per transaction boundary.
//!
//! Each operation implements [`crate::tx::WriteOp`], so each is split the same
//! way: gather ports before `BEGIN IMMEDIATE`, compare-then-mutate inside, and
//! surrender any durability assertion strictly after `COMMIT`.
//!
//! The boundaries here are the ones `docs/data-model.md` "Critical transaction
//! boundaries" names, plus the two protocols
//! `docs/research/2026-08-31-durable-orchestration-pattern-review.md` adds:
//!
//! | Module | Boundary |
//! | --- | --- |
//! | [`bootstrap`] | register the work structure; commit a foreman binding |
//! | [`worker`] | worker start, verified failure, terminal result publication |
//! | [`delivery`] | create/claim a wake, arm Send, record the outcome, reconcile an ambiguous one |
//! | [`claim`] | `foreman_resume` claim minting, handoff, fenced ACK |
//! | [`mutation`] | the mutation-command receipt journal |
//! | [`effect`] | the durable-intent/permit protocol for external effects |
//! | [`lease`] | exclusive resource ownership |
//! | [`health`] | durable attention records, and nothing else |
//! | [`recovery`] | startup quarantine, before any new external I/O |

pub(crate) mod bootstrap;
pub(crate) mod claim;
pub(crate) mod delivery;
pub(crate) mod effect;
pub(crate) mod health;
pub(crate) mod lease;
pub(crate) mod mutation;
pub(crate) mod recovery;
pub(crate) mod worker;

use governor_core::fence::{SafeToken, SourceRef};
use governor_core::id::{Id, IdKind};

use crate::error::{CorruptReason, CorruptValue, StoreResult};

/// Bounded completion evidence for a generic external attempt.
///
/// [`governor_core::effect::ExternalAttempt`] is parameterised by the exact
/// evidence its adapter can produce. The store persists attempts for adapters
/// that do not exist yet, so its evidence type is deliberately the smallest
/// thing that can identify a landed effect: one opaque reference the
/// destination handed back.
///
/// It is a [`SafeToken`], so a response body, a transcript or a credential is
/// not representable — the charset refuses whitespace and path separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptEvidence(SafeToken);

impl AttemptEvidence {
    /// Records the opaque reference the destination returned.
    #[must_use]
    pub const fn new(reference: SafeToken) -> Self {
        Self(reference)
    }

    /// Returns the reference, for persistence and diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// Namespace for source identities Command Governor mints for its own facts.
///
/// `docs/data-model.md`: *internal Command Governor events use their own
/// generated source IDs*. The event component is always an opaque identity this
/// process just minted, never a hash of content.
pub(crate) const INTERNAL_NAMESPACE: &str = "cg.internal";

/// Namespace for source identities startup recovery mints.
pub(crate) const RECOVERY_NAMESPACE: &str = "cg.recovery";

/// Builds the source identity for an internal fact about `subject`.
///
/// Deterministic in the subject and the label, which is what makes a repeated
/// internal write converge on the existing ledger row instead of appending a
/// second one.
pub(crate) fn internal_source<K: IdKind>(subject: Id<K>, label: &str) -> StoreResult<SourceRef> {
    internal_source_text(&subject.to_string(), label)
}

/// As [`internal_source`], for a subject already rendered for persistence.
pub(crate) fn internal_source_text(subject: &str, label: &str) -> StoreResult<SourceRef> {
    source_in(INTERNAL_NAMESPACE, subject, label)
}

/// Builds the source identity for a recovery finding about `subject`.
///
/// The fence is the daemon epoch, so recovery repeated within one epoch is
/// idempotent while a genuinely new lifetime records a new finding.
pub(crate) fn recovery_source<K: IdKind>(
    subject: Id<K>,
    label: &str,
    epoch: u64,
) -> StoreResult<SourceRef> {
    source_in(
        RECOVERY_NAMESPACE,
        &subject.to_string(),
        &format!("{label}.{epoch}"),
    )
}

fn source_in(namespace: &'static str, subject: &str, fence: &str) -> StoreResult<SourceRef> {
    let unsafe_token =
        |column: &'static str| CorruptValue::new("events", column, CorruptReason::UnsafeToken);
    Ok(SourceRef::new(
        SafeToken::new(namespace).map_err(|_| unsafe_token("source_namespace"))?,
        SafeToken::new(subject).map_err(|_| unsafe_token("source_event_id"))?,
        SafeToken::new(fence).map_err(|_| unsafe_token("source_event_fence"))?,
    ))
}

/// Writes the materialised copy of an obligation transition.
///
/// Two rows, always together: the `obligations` projection is overwritten from
/// the value the state machine produced, and one immutable `obligation_events`
/// row records the transition at its resulting version. The
/// `UNIQUE(obligation_id, obligation_version)` index on that table is what makes
/// a double-applied transition impossible to record rather than merely unlikely.
///
/// Nothing here decides anything: every field comes from `after`, which is the
/// output of [`governor_core::obligation::Obligation::apply`].
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-value error for an unstorable counter.
pub(crate) fn record_obligation_transition(
    tx: &crate::tx::Tx<'_>,
    before: &governor_core::obligation::Obligation,
    after: &governor_core::obligation::Obligation,
    seq: governor_core::fence::EventSeq,
    source_event_seq: governor_core::fence::EventSeq,
    actor: crate::codec::ActorClass,
    disposition: Option<governor_core::obligation::Disposition>,
) -> StoreResult<()> {
    use crate::codec::{
        encode_actor_class, encode_disposition, encode_obligation_state, id_text, store_u64,
    };
    use rusqlite::params;

    const TABLE: &str = "obligations";
    let stored_seq = crate::event::store_seq(seq)?;
    let closed = after.state().is_closed().then_some(stored_seq);

    tx.conn().execute(
        "UPDATE obligations
            SET state = ?2,
                current_version = ?3,
                current_binding_generation = ?4,
                current_claim_id = ?5,
                result_artifact_id = ?6,
                input_request_id = ?7,
                incarnation_generation = ?8,
                source_event_seq = ?9,
                latest_event_seq = ?10,
                closed_event_seq = COALESCE(closed_event_seq, ?11)
          WHERE obligation_id = ?1",
        params![
            id_text(after.id()),
            encode_obligation_state(after.state()),
            store_u64(after.version().get(), TABLE, "current_version")?,
            after
                .binding_generation()
                .map(|generation| store_u64(generation.get(), TABLE, "current_binding_generation"))
                .transpose()?,
            after.claim().map(id_text),
            after.result_artifact().map(id_text),
            after.input_request().map(id_text),
            store_u64(after.incarnation().get(), TABLE, "incarnation_generation")?,
            crate::event::store_seq(source_event_seq)?,
            stored_seq,
            closed,
        ],
    )?;

    tx.conn().execute(
        "INSERT INTO obligation_events (obligation_id, obligation_version, event_seq,
                                        from_state, to_state, disposition, actor_class,
                                        binding_generation, claim_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id_text(after.id()),
            store_u64(
                after.version().get(),
                "obligation_events",
                "obligation_version"
            )?,
            stored_seq,
            encode_obligation_state(before.state()),
            encode_obligation_state(after.state()),
            disposition
                .map(|value| encode_disposition(value, "obligation_events"))
                .transpose()?,
            encode_actor_class(actor),
            after
                .binding_generation()
                .map(|generation| store_u64(
                    generation.get(),
                    "obligation_events",
                    "binding_generation"
                ))
                .transpose()?,
            after.claim().map(id_text),
        ],
    )?;

    if let Some(artifact) = after.result_artifact() {
        crate::load::refresh_retention(tx, artifact)?;
    }
    Ok(())
}
