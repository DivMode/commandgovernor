//! Startup quarantine, before any new external I/O.
//!
//! `docs/state-machines.md` invariant 12 and `docs/testing.md` DB-006: a
//! restart must quarantine effects whose fate was lost *before* it schedules
//! anything new. This operation is that quarantine, and it is one transaction
//! so a crash during recovery leaves either the old state or the fully
//! quarantined one.
//!
//! Three families, one rule each:
//!
//! | Family | Left as | Becomes | Automatic replay |
//! | --- | --- | --- | --- |
//! | browser delivery attempt | `claimed` / `activation_armed` | `ambiguous` | never |
//! | mutation command | `received` | `uncertain` | never |
//! | external attempt | `intent_recorded` | `ambiguous` + `reconciliation_required` | never |
//!
//! A browser attempt still live at startup is by definition from a previous
//! process: this one has performed no browser I/O yet. That can conservatively
//! quarantine a wake that crashed before Send, and the data model says so
//! explicitly — duplicate avoidance wins over guessing.
//!
//! Nothing here dispatches, retries, or resolves anything. Every outcome is a
//! recorded uncertainty for a human or an explicit reconciliation procedure.

use governor_core::effect::{EffectAmbiguityReason, ExternalAttemptEvent};
use governor_core::fence::DaemonEpoch;
use governor_core::health::{HealthConditionKind, HealthConditionState};
use governor_core::id::{EventId, HealthConditionId};
use governor_core::outbound::DeliveryEvent;
use governor_core::time::Timestamp;
use rusqlite::params;

use crate::codec::{
    encode_ambiguity, encode_attempt_state, encode_effect_ambiguity, encode_health_kind,
    encode_health_state, id_text, parse_delivery_id, store_time, store_u64,
};
use crate::error::StoreResult;
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::load;
use crate::ops::delivery::set_delivery_state;
use crate::ops::{effect, mutation, recovery_source};
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// Running startup quarantine under a freshly advanced daemon epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoverStartupRequest {
    /// The epoch this process is running under.
    pub(crate) daemon_epoch: DaemonEpoch,
}

/// What startup recovery found and quarantined.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupRecovery {
    /// Wake revisions whose live attempts became `ambiguous`.
    pub quarantined_deliveries: usize,
    /// Mutation identities that became `uncertain`.
    pub uncertain_mutations: usize,
    /// External attempts that became `ambiguous`.
    pub ambiguous_attempts: usize,
    /// `reconciliation_required` conditions opened for those attempts.
    pub reconciliation_conditions: usize,
}

/// Quarantines every effect whose fate a previous process lost.
pub(crate) struct RecoverStartup {
    request: RecoverStartupRequest,
    now: Timestamp,
    /// Minted up front, because a transaction body cannot reach the identity
    /// port. Unused ones are simply discarded.
    identities: Vec<(EventId, HealthConditionId)>,
}

/// How many quarantine identities one recovery pass may need.
///
/// Recovery is bounded on purpose: a state root with more outstanding
/// ambiguity than this has a bigger problem than a slow startup, and the
/// remainder is picked up by the next pass rather than blocking this one.
const MAX_QUARANTINED_PER_PASS: usize = 256;

impl WriteOp for RecoverStartup {
    type Request = RecoverStartupRequest;
    type Committed = StartupRecovery;
    type Output = StartupRecovery;

    const NAME: &'static str = "recover_startup";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        let identities = (0..MAX_QUARANTINED_PER_PASS)
            .map(|_| (ports.next_id(), ports.next_id()))
            .collect();
        Ok(Self {
            request,
            now: ports.now(),
            identities,
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let mut report = StartupRecovery::default();
        let mut minted = self.identities.iter();

        // 1. Browser wakes, first: no browser recovery may run until every
        //    attempt whose outcome was lost is frozen.
        for delivery_hex in live_delivery_ids(tx)? {
            let Some((event_id, _)) = minted.next() else {
                break;
            };
            let delivery_id = parse_delivery_id(&delivery_hex, "delivery_attempts", "delivery_id")?;
            let loaded = load::wake_by_delivery_id(tx, &delivery_id)?;
            let wake = loaded.wake;
            let transition = wake.apply(&DeliveryEvent::OrphanQuarantined { at: self.now })?;
            let Some(next) = transition.advanced() else {
                continue;
            };
            let obligation = load::obligation(tx, wake.target().obligation)?;

            let seq = event::append(
                tx,
                &NewEvent {
                    event_id: *event_id,
                    kind: EventKind::BrowserDeliveryOrphanQuarantined,
                    source: recovery_source(
                        obligation.projection.id(),
                        &format!("wake.{}", wake.revision()),
                        self.request.daemon_epoch.get(),
                    )?,
                    observed_at: self.now,
                    occurred_at: None,
                    scope: EventScope {
                        task: Some(obligation.identity.task),
                        obligation: Some(obligation.projection.id()),
                        ..EventScope::default()
                    },
                    metadata: SafeMetadata::new().int("revision", i64::from(wake.revision().get())),
                },
            )?
            .seq();

            for attempt in next.delivery().attempts() {
                tx.conn().execute(
                    "UPDATE delivery_attempts
                        SET state = ?3, terminal_event_seq = ?4, finished_at_ms = ?5,
                            evidence_class = ?6
                      WHERE delivery_id = ?1 AND attempt_no = ?2
                        AND state IN ('claimed', 'activation_armed')",
                    params![
                        delivery_hex,
                        i64::from(attempt.number().get()),
                        encode_attempt_state(attempt.state()),
                        event::store_seq(seq)?,
                        store_time(self.now),
                        attempt
                            .ambiguity()
                            .map(|reason| encode_ambiguity(reason, "delivery_attempts"))
                            .transpose()?,
                    ],
                )?;
            }
            set_delivery_state(tx, &delivery_id, next.state(), None, Some(seq))?;
            report.quarantined_deliveries += 1;
        }

        // 2. Mutation commands: `received` with no committed result is
        //    uncertain, and uncertain is never redispatched.
        for row in mutation::unresolved_before(tx, self.request.daemon_epoch)? {
            mutation::mark_uncertain(tx, &row, self.now)?;
            report.uncertain_mutations += 1;
        }

        // 3. External attempts: a durable intent with no proven outcome is
        //    ambiguous, and each one opens its own attention record. Zero
        //    automatic I/O follows.
        for attempt in effect::unresolved_before(tx, self.request.daemon_epoch)? {
            let Some((event_id, condition_id)) = minted.next() else {
                break;
            };
            let next = attempt
                .apply(&ExternalAttemptEvent::OutcomeUnknown {
                    reason: EffectAmbiguityReason::OrphanedByRestart,
                    at: self.now,
                })?
                .or_unchanged(attempt.clone());
            effect::write_terminal(tx, attempt.id(), &next)?;

            let seq = event::append(
                tx,
                &NewEvent {
                    event_id: *event_id,
                    kind: EventKind::ExternalAttemptQuarantined,
                    source: recovery_source(
                        attempt.id(),
                        "external_attempt",
                        self.request.daemon_epoch.get(),
                    )?,
                    observed_at: self.now,
                    occurred_at: None,
                    scope: EventScope::default(),
                    metadata: SafeMetadata::new()
                        .id("external_attempt", attempt.id())
                        .label(
                            "ambiguity_reason",
                            encode_effect_ambiguity(EffectAmbiguityReason::OrphanedByRestart),
                        ),
                },
            )?
            .seq();
            report.ambiguous_attempts += 1;

            // `ON CONFLICT DO NOTHING` against the partial unique index is the
            // durable half of `HealthLedger::raise`'s deduplication: one open
            // condition per (kind, scope), so a second recovery pass over the
            // same attempt adds nothing.
            let opened = tx.conn().execute(
                "INSERT INTO health_conditions (health_condition_id, kind, state,
                        external_attempt_id, opened_event_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT DO NOTHING",
                params![
                    id_text(*condition_id),
                    encode_health_kind(HealthConditionKind::ReconciliationRequired),
                    encode_health_state(HealthConditionState::Open),
                    id_text(attempt.id()),
                    event::store_seq(seq)?,
                ],
            )?;
            report.reconciliation_conditions += opened;
        }

        // Recorded last so a reader can tell the epoch this pass ran under.
        crate::meta::put(
            tx.conn(),
            crate::meta::DAEMON_EPOCH,
            &store_u64(self.request.daemon_epoch.get(), "meta", "daemon_epoch")?.to_string(),
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(report)
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Every delivery with at least one attempt still owning an external effect.
fn live_delivery_ids(tx: &Tx<'_>) -> StoreResult<Vec<String>> {
    let mut statement = tx.conn().prepare(
        "SELECT DISTINCT delivery_id FROM delivery_attempts
          WHERE state IN ('claimed', 'activation_armed')
          ORDER BY delivery_id",
    )?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
