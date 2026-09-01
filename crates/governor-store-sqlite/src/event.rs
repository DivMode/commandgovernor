//! The immutable event ledger: append, dedupe, and read back.
//!
//! Every accepted event carries a source identity, and the ledger's
//! `UNIQUE(source_namespace, source_event_id, source_event_fence)` index is the
//! durable half of `docs/state-machines.md` global rule *duplicate source events
//! are idempotent*. [`append`] is the only writer, and a duplicate source
//! returns the sequence of the event that is already there — it never inserts a
//! second row, so the caller's projection transition never runs twice
//! (`docs/testing.md` DB-007).
//!
//! Event kinds are a closed enum. A kind the store cannot decode is a corrupt
//! row, not an extension point, and each kind's allowlisted metadata fields are
//! declared here alongside it.

use governor_core::fence::{EventSeq, SourceRef};
use governor_core::id::{
    EventId, ObligationId, ProjectId, SessionId, SessionIncarnationId, TaskId, TurnId,
};
use governor_core::time::Timestamp;
use rusqlite::OptionalExtension as _;
use rusqlite::params;

use crate::codec::{id_text, parse_source, parse_time, parse_u64, store_time};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::safe_metadata::{MetadataFields, SafeMetadata};
use crate::tx::{Failpoint, Tx};

/// Schema version stamped on every event this binary appends.
pub(crate) const EVENT_SCHEMA_VERSION: i64 = 1;

/// The closed set of domain events Phase 1 can append.
///
/// Adding a kind is a deliberate, reviewed change: the durable ledger, the
/// replay loader, and the metadata allowlist all key off this enum, so a kind
/// with no replay rule cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum EventKind {
    /// A source-host project reference was recorded.
    ProjectRegistered,
    /// A unit of delegated work was recorded.
    TaskRegistered,
    /// A logical worker session was recorded.
    SessionRegistered,
    /// A concrete session incarnation started.
    SessionIncarnationStarted,
    /// A worker turn started within an incarnation.
    TurnStarted,
    /// An obligation was created in `created`.
    ObligationCreated,
    /// A worker turn was verified to be running.
    WorkerStarted,
    /// A verified terminal worker failure. Unprocessed work, not a closure.
    WorkerFailed,
    /// A confirmed final result whose artifact was already made durable.
    ResultPublished,
    /// A verified foreman binding was committed at a new generation.
    ForemanBindingBound,
    /// A later capability observation for the active binding.
    ForemanBindingCapabilityObserved,
    /// The bound surface was displaced.
    ForemanBindingDisplaced,
    /// A browser delivery attempt was claimed, before any browser I/O.
    BrowserDeliveryAttemptClaimed,
    /// The Send ambiguity fence was armed, immediately before Send.
    BrowserDeliveryActivationArmed,
    /// Exact semantic evidence proved the wake was submitted.
    BrowserDeliveryAccepted,
    /// A wake attempt was proven not to have submitted anything.
    BrowserDeliveryFailed,
    /// A wake attempt's outcome could not be determined.
    BrowserDeliveryAmbiguous,
    /// Startup quarantined an attempt orphaned by a previous process.
    BrowserDeliveryOrphanQuarantined,
    /// A foreman claim was minted from an accepted current-generation wake.
    ForemanClaimMinted,
    /// The result or input request was handed to the claiming foreman.
    ForemanHandoffDelivered,
    /// A claim's bound lifetime elapsed.
    ForemanClaimExpired,
    /// An explicit fenced disposition closed an obligation.
    ForemanAcked,
    /// The user cancelled the work.
    ObligationCancelledByUser,
    /// A later obligation replaced this one.
    ObligationSuperseded,
    /// Startup found a consequential external effect whose fate was lost.
    ///
    /// The intent was durable, the outcome never was. This event records the
    /// finding and is what the `reconciliation_required` health condition hangs
    /// off; it authorises nothing and replays nothing.
    ExternalAttemptQuarantined,
}

impl EventKind {
    /// The stable `snake_case` label persisted in `events.kind`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProjectRegistered => "project_registered",
            Self::TaskRegistered => "task_registered",
            Self::SessionRegistered => "session_registered",
            Self::SessionIncarnationStarted => "session_incarnation_started",
            Self::TurnStarted => "turn_started",
            Self::ObligationCreated => "obligation_created",
            Self::WorkerStarted => "worker_started",
            Self::WorkerFailed => "worker_failed",
            Self::ResultPublished => "result_published",
            Self::ForemanBindingBound => "foreman_binding_bound",
            Self::ForemanBindingCapabilityObserved => "foreman_binding_capability_observed",
            Self::ForemanBindingDisplaced => "foreman_binding_displaced",
            Self::BrowserDeliveryAttemptClaimed => "browser_delivery_attempt_claimed",
            Self::BrowserDeliveryActivationArmed => "browser_delivery_activation_armed",
            Self::BrowserDeliveryAccepted => "browser_delivery_accepted",
            Self::BrowserDeliveryFailed => "browser_delivery_failed",
            Self::BrowserDeliveryAmbiguous => "browser_delivery_ambiguous",
            Self::BrowserDeliveryOrphanQuarantined => "browser_delivery_orphan_quarantined",
            Self::ForemanClaimMinted => "foreman_claim_minted",
            Self::ForemanHandoffDelivered => "foreman_handoff_delivered",
            Self::ForemanClaimExpired => "foreman_claim_expired",
            Self::ForemanAcked => "foreman_acked",
            Self::ObligationCancelledByUser => "obligation_cancelled_by_user",
            Self::ObligationSuperseded => "obligation_superseded",
            Self::ExternalAttemptQuarantined => "external_attempt_quarantined",
        }
    }

    /// Decodes a persisted `events.kind` label.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row error for a kind this binary does not implement.
    pub(crate) fn parse(text: &str) -> StoreResult<Self> {
        const ALL: &[EventKind] = &[
            EventKind::ProjectRegistered,
            EventKind::TaskRegistered,
            EventKind::SessionRegistered,
            EventKind::SessionIncarnationStarted,
            EventKind::TurnStarted,
            EventKind::ObligationCreated,
            EventKind::WorkerStarted,
            EventKind::WorkerFailed,
            EventKind::ResultPublished,
            EventKind::ForemanBindingBound,
            EventKind::ForemanBindingCapabilityObserved,
            EventKind::ForemanBindingDisplaced,
            EventKind::BrowserDeliveryAttemptClaimed,
            EventKind::BrowserDeliveryActivationArmed,
            EventKind::BrowserDeliveryAccepted,
            EventKind::BrowserDeliveryFailed,
            EventKind::BrowserDeliveryAmbiguous,
            EventKind::BrowserDeliveryOrphanQuarantined,
            EventKind::ForemanClaimMinted,
            EventKind::ForemanHandoffDelivered,
            EventKind::ForemanClaimExpired,
            EventKind::ForemanAcked,
            EventKind::ObligationCancelledByUser,
            EventKind::ObligationSuperseded,
            EventKind::ExternalAttemptQuarantined,
        ];
        ALL.iter()
            .copied()
            .find(|kind| kind.label() == text)
            .ok_or_else(|| CorruptValue::new("events", "kind", CorruptReason::UnknownLabel).into())
    }

    /// The allowlisted `safe_metadata_json` fields for this kind.
    ///
    /// Anything else in a stored document is discarded on read, and there is no
    /// writer that could have put it there.
    #[must_use]
    pub const fn allowed_metadata_fields(self) -> &'static [&'static str] {
        match self {
            Self::ProjectRegistered
            | Self::TaskRegistered
            | Self::SessionRegistered
            | Self::ObligationCancelledByUser => &[],
            Self::SessionIncarnationStarted | Self::ForemanBindingDisplaced => &["generation"],
            Self::TurnStarted => &["turn_generation"],
            Self::ObligationCreated => &["obligation_kind", "incarnation"],
            Self::WorkerStarted => &["incarnation"],
            Self::WorkerFailed => &["incarnation", "failure_class"],
            Self::ResultPublished => &["incarnation", "run_ref", "artifact_id"],
            Self::ForemanBindingBound | Self::ForemanBindingCapabilityObserved => {
                &["generation", "capability_epoch", "write_capability"]
            }
            // Every delivery event names its revision as well as its attempt:
            // replay folds one revision's attempt machine, and an event that
            // could not say which revision it belonged to would be unfoldable.
            Self::BrowserDeliveryAttemptClaimed | Self::BrowserDeliveryActivationArmed => {
                &["revision", "attempt_no"]
            }
            // Quarantine applies to every live attempt of the revision at once,
            // exactly as `DeliveryEvent::OrphanQuarantined` does.
            Self::BrowserDeliveryOrphanQuarantined => &["revision"],
            Self::BrowserDeliveryAccepted => &["revision", "attempt_no", "message_ref"],
            Self::BrowserDeliveryFailed => &["revision", "attempt_no", "failure_class"],
            Self::BrowserDeliveryAmbiguous => &["revision", "attempt_no", "evidence_class"],
            Self::ForemanHandoffDelivered | Self::ForemanClaimExpired => &["claim_id"],
            // `expected_version` is the fence the caller actually presented.
            // Recording it lets replay *check* the fold rather than feed the
            // machine whatever version the fold happens to be at, which would
            // make the compare-and-swap trivially true on every replay.
            Self::ForemanClaimMinted => &["claim_id", "binding_generation", "expected_version"],
            Self::ForemanAcked => &[
                "claim_id",
                "binding_generation",
                "expected_version",
                "disposition",
            ],
            Self::ObligationSuperseded => &["replacement"],
            Self::ExternalAttemptQuarantined => &["external_attempt", "ambiguity_reason"],
        }
    }
}

/// The domain entities an event is about.
///
/// Every field is an opaque identity or nothing; there is no free-text scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EventScope {
    pub(crate) project: Option<ProjectId>,
    pub(crate) task: Option<TaskId>,
    pub(crate) session: Option<SessionId>,
    pub(crate) incarnation: Option<SessionIncarnationId>,
    pub(crate) turn: Option<TurnId>,
    pub(crate) obligation: Option<ObligationId>,
}

/// One event ready to be appended.
///
/// `event_id` is minted in `prepare`, outside the transaction. On a duplicate
/// source it is discarded along with the rest of the row.
#[derive(Debug, Clone)]
pub(crate) struct NewEvent {
    pub(crate) event_id: EventId,
    pub(crate) kind: EventKind,
    pub(crate) source: SourceRef,
    pub(crate) observed_at: Timestamp,
    pub(crate) occurred_at: Option<Timestamp>,
    pub(crate) scope: EventScope,
    pub(crate) metadata: SafeMetadata,
}

/// What appending produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Appended {
    /// A new row; the caller must apply the projection transition.
    Inserted(EventSeq),
    /// The source identity was already in the ledger. The caller must *not*
    /// apply a second transition, and must report the existing result.
    Duplicate(EventSeq),
}

impl Appended {
    pub(crate) const fn seq(self) -> EventSeq {
        match self {
            Self::Inserted(seq) | Self::Duplicate(seq) => seq,
        }
    }

    pub(crate) const fn is_duplicate(self) -> bool {
        matches!(self, Self::Duplicate(_))
    }
}

/// Appends an event, converging on the existing row for a duplicate source.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error when the existing duplicate
/// cannot be read back.
pub(crate) fn append(tx: &Tx<'_>, event: &NewEvent) -> StoreResult<Appended> {
    let metadata = event.metadata.to_json();
    let changed = tx.conn().execute(
        "INSERT INTO events (
             event_id, kind, schema_version, observed_at_ms, occurred_at_ms,
             project_id, task_id, session_id, session_incarnation_id, turn_id,
             obligation_id,
             source_namespace, source_event_id, source_event_fence,
             safe_metadata_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(source_namespace, source_event_id, source_event_fence) DO NOTHING",
        params![
            id_text(event.event_id),
            event.kind.label(),
            EVENT_SCHEMA_VERSION,
            store_time(event.observed_at),
            event.occurred_at.map(store_time),
            event.scope.project.map(id_text),
            event.scope.task.map(id_text),
            event.scope.session.map(id_text),
            event.scope.incarnation.map(id_text),
            event.scope.turn.map(id_text),
            event.scope.obligation.map(id_text),
            event.source.namespace().as_str(),
            event.source.event().as_str(),
            event.source.fence().as_str(),
            metadata,
        ],
    )?;

    let seq: i64 = tx.conn().query_row(
        "SELECT seq FROM events
          WHERE source_namespace = ?1 AND source_event_id = ?2 AND source_event_fence = ?3",
        params![
            event.source.namespace().as_str(),
            event.source.event().as_str(),
            event.source.fence().as_str(),
        ],
        |row| row.get(0),
    )?;
    let seq = EventSeq::new(parse_u64(seq, "events", "seq")?);

    tx.reach(Failpoint::AfterEventAppend)?;

    Ok(if changed == 1 {
        Appended::Inserted(seq)
    } else {
        Appended::Duplicate(seq)
    })
}

/// One event read back from the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerEvent {
    pub(crate) seq: EventSeq,
    pub(crate) kind: EventKind,
    pub(crate) source: SourceRef,
    pub(crate) observed_at: Timestamp,
    pub(crate) metadata: MetadataFields,
}

/// Reads one obligation's ledger slice, oldest first.
///
/// This is what a fenced compare-then-mutate reads: the obligation's current
/// state is *folded from its events* rather than trusted from its projection
/// row, so the value a transition is applied to was necessarily built by the
/// `governor-core` state machine. The projection row written in the same
/// transaction is a materialised cache of that fold, and
/// [`crate::replay`] proves the two agree.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an undecodable event.
pub(crate) fn read_for_obligation(
    tx: &Tx<'_>,
    obligation: ObligationId,
) -> StoreResult<Vec<LedgerEvent>> {
    let mut statement = tx.conn().prepare(
        "SELECT seq, kind, source_namespace, source_event_id, source_event_fence,
                observed_at_ms, safe_metadata_json
           FROM events WHERE obligation_id = ?1 ORDER BY seq",
    )?;
    let rows = statement.query_map(params![id_text(obligation)], decode_row)?;
    collect(rows)
}

/// Reads the whole ledger, oldest first.
///
/// Used by projection replay verification. Phase 1 ledgers are small; when that
/// stops being true this becomes a windowed read from the verified watermark.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an undecodable event.
pub(crate) fn read_all(tx: &Tx<'_>) -> StoreResult<Vec<LedgerEvent>> {
    let mut statement = tx.conn().prepare(
        "SELECT seq, kind, source_namespace, source_event_id, source_event_fence,
                observed_at_ms, safe_metadata_json
           FROM events ORDER BY seq",
    )?;
    let rows = statement.query_map([], decode_row)?;
    collect(rows)
}

/// The highest sequence the ledger holds, if it holds anything.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an unreadable sequence.
pub(crate) fn highest_seq(tx: &Tx<'_>) -> StoreResult<Option<EventSeq>> {
    let value: Option<i64> = tx
        .conn()
        .query_row("SELECT MAX(seq) FROM events", [], |row| row.get(0))
        .optional()?
        .flatten();
    value.map(|seq| parse_seq(seq, "events", "seq")).transpose()
}

type RawRow = (i64, String, String, String, String, i64, String);

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn collect(rows: impl Iterator<Item = rusqlite::Result<RawRow>>) -> StoreResult<Vec<LedgerEvent>> {
    let mut out = Vec::new();
    for row in rows {
        let (seq, kind, namespace, event, fence, observed, metadata) = row?;
        let kind = EventKind::parse(&kind)?;
        out.push(LedgerEvent {
            seq: parse_seq(seq, "events", "seq")?,
            kind,
            source: parse_source(&namespace, &event, &fence)?,
            observed_at: parse_time(observed),
            metadata: SafeMetadata::parse(&metadata, kind.allowed_metadata_fields())?,
        });
    }
    Ok(out)
}

/// Renders an event sequence for a `*_event_seq` column.
pub(crate) fn store_seq(seq: EventSeq) -> StoreResult<i64> {
    crate::codec::store_u64(seq.get(), "events", "seq")
}

/// Rehydrates an event sequence from a `*_event_seq` column.
pub(crate) fn parse_seq(
    value: i64,
    table: &'static str,
    column: &'static str,
) -> StoreResult<EventSeq> {
    Ok(EventSeq::new(parse_u64(value, table, column)?))
}
