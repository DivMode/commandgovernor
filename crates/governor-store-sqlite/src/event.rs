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

use crate::codec::{id_text, parse_id, parse_source, parse_time, parse_u64, store_time};
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
    /// Exact later evidence promoted an ambiguous revision to accepted.
    ///
    /// No Send happened: this records a reconciliation, and the revision stays
    /// frozen. See the store's `reconcile_ambiguous_delivery` operation.
    BrowserDeliveryReconciled,
    /// Startup found a consequential external effect whose fate was lost.
    ///
    /// The intent was durable, the outcome never was. This event records the
    /// finding and is what the `reconciliation_required` health condition hangs
    /// off; it authorises nothing and replays nothing.
    ExternalAttemptQuarantined,
    /// A health condition was opened.
    ///
    /// Attention, never terminal worker state. The condition's *scope* is the
    /// event's own scope columns and its *kind* is the one allowlisted metadata
    /// field, so the health-ledger fold can rebuild the whole ledger from these
    /// events alone.
    HealthConditionOpened,
    /// A health condition was resolved by later verified evidence.
    HealthConditionResolved,
    /// An immutable capability-profile snapshot was recorded.
    CapabilityProfileRecorded,
    /// An immutable recursive-delegation-policy snapshot was recorded.
    DelegationPolicyRecorded,
    /// An immutable model-policy snapshot was recorded.
    ModelPolicyRecorded,
    /// A private immutable managed-configuration artifact was recorded.
    ManagedConfigRecorded,
    /// A fully resolved immutable worker loadout was recorded.
    WorkerLoadoutResolved,
    /// One session incarnation was bound to its launch loadout.
    SessionLoadoutBound,
    /// A durable parent/child logical session lineage edge was recorded.
    SessionLineageRecorded,
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
            Self::BrowserDeliveryReconciled => "browser_delivery_reconciled",
            Self::ExternalAttemptQuarantined => "external_attempt_quarantined",
            Self::HealthConditionOpened => "health_condition_opened",
            Self::HealthConditionResolved => "health_condition_resolved",
            Self::CapabilityProfileRecorded => "capability_profile_recorded",
            Self::DelegationPolicyRecorded => "delegation_policy_recorded",
            Self::ModelPolicyRecorded => "model_policy_recorded",
            Self::ManagedConfigRecorded => "managed_config_recorded",
            Self::WorkerLoadoutResolved => "worker_loadout_resolved",
            Self::SessionLoadoutBound => "session_loadout_bound",
            Self::SessionLineageRecorded => "session_lineage_recorded",
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
            EventKind::BrowserDeliveryReconciled,
            EventKind::ExternalAttemptQuarantined,
            EventKind::HealthConditionOpened,
            EventKind::HealthConditionResolved,
            EventKind::CapabilityProfileRecorded,
            EventKind::DelegationPolicyRecorded,
            EventKind::ModelPolicyRecorded,
            EventKind::ManagedConfigRecorded,
            EventKind::WorkerLoadoutResolved,
            EventKind::SessionLoadoutBound,
            EventKind::SessionLineageRecorded,
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
            // Reconciliation promotes the whole revision rather than a numbered
            // attempt — the fold's `ReconciledAccepted` carries no attempt — so
            // there is no `attempt_no` to record and none is allowed.
            Self::BrowserDeliveryReconciled => &["revision", "message_ref"],
            Self::ObligationSuperseded => &["replacement"],
            Self::ExternalAttemptQuarantined => &["external_attempt", "ambiguity_reason"],
            // A health condition's scope lives in the event's own scope
            // columns, so the only metadata it needs is which kind it is. The
            // two are never duplicated, and there is therefore nothing that can
            // disagree with the projection row.
            Self::HealthConditionOpened | Self::HealthConditionResolved => &["health_kind"],
            // Immutable snapshots. `events` has no column for a profile, a
            // policy, a configuration or a loadout identity, and adding six
            // would put a scope on every event that has none. So the identity
            // and its contents digest travel as allowlisted metadata instead.
            //
            // The digest is *not* the row's own `digest_hex` read back: it is
            // the value the resolver derived, recorded beside the identity so
            // an operator can tell which snapshot an event is about without
            // consulting the projection it is meant to check.
            Self::CapabilityProfileRecorded => &["capability_profile", "digest", "entry_count"],
            Self::DelegationPolicyRecorded => &["delegation_policy", "digest", "entry_count"],
            Self::ModelPolicyRecorded => &["model_policy", "digest"],
            Self::ManagedConfigRecorded => &["managed_config", "digest", "hook_contract_epoch"],
            // Identities and the loadout digest only. `role`, `worker_kind` and
            // `runtime_kind` are deliberately absent: each is an opaque token
            // that belongs in exactly one column, and a second copy here would
            // be a second place for it to leak from.
            Self::WorkerLoadoutResolved => &[
                "loadout_id",
                "digest",
                "capability_profile",
                "delegation_policy",
                "model_policy",
                "managed_config",
            ],
            // The session and its incarnation are the event's own scope
            // columns; only the loadout it was bound to needs saying.
            Self::SessionLoadoutBound => &["loadout_id", "digest"],
            // The event's `session` scope is the *child*. These three fields
            // are what makes `replay::compare_lineage` a genuine ledger fold:
            // without them the edge could not be rebuilt at all, and the
            // comparison would degenerate into re-reading the row it is
            // supposed to be checking.
            Self::SessionLineageRecorded => &["parent_session", "parent_turn", "relation"],
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
    /// The entities the event is about, read back from the scope columns.
    ///
    /// Read rather than duplicated into metadata: a health condition's scope is
    /// part of its identity, and two copies of it would be two things that can
    /// disagree.
    pub(crate) scope: EventScope,
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
    let mut statement = tx.conn().prepare(&format!(
        "SELECT {EVENT_COLUMNS} FROM events WHERE obligation_id = ?1 ORDER BY seq"
    ))?;
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
    let mut statement = tx
        .conn()
        .prepare(&format!("SELECT {EVENT_COLUMNS} FROM events ORDER BY seq"))?;
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

/// The projection every ledger read selects, in [`RawEvent`]'s field order.
const EVENT_COLUMNS: &str = "seq, kind, source_namespace, source_event_id, source_event_fence, \
     observed_at_ms, safe_metadata_json, project_id, task_id, session_id, \
     session_incarnation_id, turn_id, obligation_id";

/// One `events` row, still as columns.
struct RawEvent {
    seq: i64,
    kind: String,
    namespace: String,
    event: String,
    fence: String,
    observed: i64,
    metadata: String,
    project: Option<String>,
    task: Option<String>,
    session: Option<String>,
    incarnation: Option<String>,
    turn: Option<String>,
    obligation: Option<String>,
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEvent> {
    Ok(RawEvent {
        seq: row.get(0)?,
        kind: row.get(1)?,
        namespace: row.get(2)?,
        event: row.get(3)?,
        fence: row.get(4)?,
        observed: row.get(5)?,
        metadata: row.get(6)?,
        project: row.get(7)?,
        task: row.get(8)?,
        session: row.get(9)?,
        incarnation: row.get(10)?,
        turn: row.get(11)?,
        obligation: row.get(12)?,
    })
}

fn collect(
    rows: impl Iterator<Item = rusqlite::Result<RawEvent>>,
) -> StoreResult<Vec<LedgerEvent>> {
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        let kind = EventKind::parse(&row.kind)?;
        out.push(LedgerEvent {
            seq: parse_seq(row.seq, "events", "seq")?,
            kind,
            source: parse_source(&row.namespace, &row.event, &row.fence)?,
            observed_at: parse_time(row.observed),
            scope: EventScope {
                project: row
                    .project
                    .map(|text| parse_id(&text, "events", "project_id"))
                    .transpose()?,
                task: row
                    .task
                    .map(|text| parse_id(&text, "events", "task_id"))
                    .transpose()?,
                session: row
                    .session
                    .map(|text| parse_id(&text, "events", "session_id"))
                    .transpose()?,
                incarnation: row
                    .incarnation
                    .map(|text| parse_id(&text, "events", "session_incarnation_id"))
                    .transpose()?,
                turn: row
                    .turn
                    .map(|text| parse_id(&text, "events", "turn_id"))
                    .transpose()?,
                obligation: row
                    .obligation
                    .map(|text| parse_id(&text, "events", "obligation_id"))
                    .transpose()?,
            },
            metadata: SafeMetadata::parse(&row.metadata, kind.allowed_metadata_fields())?,
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
