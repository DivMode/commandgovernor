//! The fake foreman/MCP boundary.
//!
//! Three things live here, and they are deliberately separate:
//!
//! 1. [`BootstrapView`] — what `foreman_bootstrap` may return. It is assembled
//!    from aggregate SQL only, so the low-information property of
//!    `docs/mcp-contract.md` is structural: there is no field for a repository
//!    ref, a task/session/turn ref, result content or an accepted
//!    `delivery_id`, and no query here selects one.
//! 2. [`ResumeBudget`] — the bounded automatic-resume policy, and the
//!    `foreman_unreachable` attention it produces when it runs out.
//! 3. [`WakeGate`] — the "never overlap an active or unknown ChatGPT turn"
//!    rule, asked before a wake is ever scheduled.
//!
//! `foreman_resume` and `foreman_ack` themselves are not modelled: they are
//! [`Store::mint_foreman_claim`](governor_store_sqlite::Store::mint_foreman_claim)
//! and
//! [`Store::acknowledge_obligation`](governor_store_sqlite::Store::acknowledge_obligation),
//! driven through [`crate::scenario`]. A fake wrapper around them would only be
//! a second place for the fences to be written down.

use governor_core::foreman_turn::{ForemanTurn, ForemanTurnState};
use governor_core::health::{HealthConditionKind, HealthLedger, HealthScope};
use governor_core::id::{HealthConditionId, ObligationId};
use governor_core::time::Timestamp;
use rusqlite::Connection;

/// The compatibility envelope every response carries.
pub const PROTOCOL_VERSION: &str = "command-governor-foreman/v1";

/// One attention bucket, aggregated.
///
/// Counts, a priority and an age. No identity of any kind: a bucket describes
/// *that* work exists, never *which*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionSummary {
    /// Obligation state label the bucket aggregates.
    pub kind: String,
    /// How many obligations are in it.
    pub count: u64,
    /// Highest scheduling priority in the bucket.
    pub highest_priority: i64,
    /// Age of the oldest member, in milliseconds.
    pub oldest_age_ms: i64,
    /// Coarse wake state across the bucket.
    pub wake_state: &'static str,
}

/// Health, aggregated.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthSummary {
    /// Feature-tested write capability of the bound surface.
    pub mcp_write_capability: String,
    /// Open `runtime_state_conflict` conditions.
    pub runtime_conflicts: u64,
    /// Wake revisions frozen at `ambiguous`.
    pub ambiguous_deliveries: u64,
    /// Kinds of every open condition, sorted and deduplicated.
    pub open_condition_kinds: Vec<String>,
}

/// Everything `foreman_bootstrap` is permitted to disclose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapView {
    /// Protocol the server speaks.
    pub protocol_version: &'static str,
    /// Connector ABI of the active binding, if any.
    pub connector_abi: Option<String>,
    /// Capability epoch of the active binding, if any.
    pub capability_epoch: Option<u64>,
    /// Whether a truthful state-changing MCP action is believed possible.
    pub write_actions_available: bool,
    /// Generation of the active binding, if any.
    pub binding_generation: Option<u64>,
    /// Coarse binding state.
    pub binding_state: &'static str,
    /// How many obligations are still owed.
    pub outstanding_count: u64,
    /// One bucket per attention kind, oldest first.
    pub attention: Vec<AttentionSummary>,
    /// Aggregate health.
    pub health: HealthSummary,
}

/// The obligation states a foreman may be asked to attend to.
const ATTENTION_STATES: &[&str] = &["completed_unprocessed", "failed", "needs_input"];

/// Assembles the bootstrap view an unrelated connector would receive.
///
/// Every value comes from a `COUNT`, a `MAX`, a `MIN` or the single active
/// binding row's compatibility columns. Nothing selects an identity column, an
/// artifact, a `storage_ref`, a `delivery_id` or a source-host reference, which
/// is why GPT-007 and SEC-002 can be asserted against the rendered value.
///
/// # Panics
///
/// Panics when the schema cannot be queried, which in a suite means the state
/// root is not a Command Governor database.
#[must_use]
pub fn bootstrap(conn: &Connection, now: Timestamp) -> BootstrapView {
    let binding: Option<(i64, String, i64, String)> = conn
        .query_row(
            "SELECT binding_generation, connector_abi, capability_epoch, write_capability_state
               FROM foreman_bindings WHERE is_active = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();

    let outstanding: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM obligations
              WHERE state NOT IN ('acknowledged', 'cancelled_by_user', 'superseded')",
            [],
            |row| row.get(0),
        )
        .expect("counting open obligations");

    let mut attention = Vec::new();
    for state in ATTENTION_STATES {
        let row: (i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), MAX(o.priority), MIN(e.observed_at_ms)
                   FROM obligations o JOIN events e ON e.seq = o.source_event_seq
                  WHERE o.state = ?1",
                rusqlite::params![state],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("aggregating an attention bucket");
        let (count, priority, oldest) = row;
        if count == 0 {
            continue;
        }
        attention.push(AttentionSummary {
            kind: (*state).to_owned(),
            count: u64::try_from(count).expect("a count is non-negative"),
            highest_priority: priority.unwrap_or(0),
            oldest_age_ms: oldest.map_or(0, |at| {
                now.saturating_elapsed_since(Timestamp::from_unix_millis(at))
                    .as_millis()
                    .try_into()
                    .unwrap_or(i64::MAX)
            }),
            wake_state: wake_state(conn, state),
        });
    }

    let ambiguous: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM browser_deliveries WHERE state = 'ambiguous'",
            [],
            |row| row.get(0),
        )
        .expect("counting ambiguous deliveries");

    let mut open_condition_kinds = Vec::new();
    {
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT kind FROM health_conditions WHERE state = 'open' ORDER BY kind",
            )
            .expect("listing open conditions");
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("iterating open conditions");
        for row in rows {
            open_condition_kinds.push(row.expect("a condition kind"));
        }
    }
    let runtime_conflicts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM health_conditions
              WHERE state = 'open' AND kind = 'runtime_state_conflict'",
            [],
            |row| row.get(0),
        )
        .expect("counting runtime conflicts");

    let write_capability = binding
        .as_ref()
        .map_or_else(|| "unknown".to_owned(), |(_, _, _, state)| state.clone());
    BootstrapView {
        protocol_version: PROTOCOL_VERSION,
        connector_abi: binding.as_ref().map(|(_, abi, _, _)| abi.clone()),
        capability_epoch: binding
            .as_ref()
            .map(|(_, _, epoch, _)| u64::try_from(*epoch).unwrap_or(0)),
        write_actions_available: write_capability == "proven",
        binding_generation: binding
            .as_ref()
            .map(|(generation, _, _, _)| u64::try_from(*generation).unwrap_or(0)),
        binding_state: if binding.is_some() {
            "healthy"
        } else {
            "unbound"
        },
        outstanding_count: u64::try_from(outstanding).expect("a count is non-negative"),
        attention,
        health: HealthSummary {
            mcp_write_capability: write_capability,
            runtime_conflicts: u64::try_from(runtime_conflicts).expect("a count is non-negative"),
            ambiguous_deliveries: u64::try_from(ambiguous).expect("a count is non-negative"),
            open_condition_kinds,
        },
    }
}

/// The coarsest true statement about the wakes behind one attention bucket.
fn wake_state(conn: &Connection, state: &str) -> &'static str {
    let accepted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM browser_deliveries d JOIN obligations o
                 ON o.obligation_id = d.obligation_id
               WHERE o.state = ?1 AND d.state = 'accepted'",
            rusqlite::params![state],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if accepted > 0 {
        return "scheduled_or_accepted";
    }
    let any: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM browser_deliveries d JOIN obligations o
                 ON o.obligation_id = d.obligation_id
               WHERE o.state = ?1",
            rusqlite::params![state],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if any > 0 { "pending" } else { "none" }
}

/// A bounded automatic-resume policy.
///
/// `docs/testing.md` GPT-006: after the configured number of automatic
/// resumes, Command Governor creates one `foreman_unreachable` condition, the
/// obligation stays open indefinitely, and there is no infinite wake loop.
#[derive(Debug, Clone)]
pub struct ResumeBudget {
    limit: u32,
    used: u32,
}

/// What the resume policy decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumeDecision {
    /// One more automatic resume revision is permitted.
    Schedule,
    /// The budget is spent. Raise attention; never send again.
    Exhausted,
}

impl ResumeBudget {
    /// Creates a budget of `limit` automatic resumes.
    #[must_use]
    pub const fn new(limit: u32) -> Self {
        Self { limit, used: 0 }
    }

    /// How many automatic resumes have been taken.
    #[must_use]
    pub const fn used(&self) -> u32 {
        self.used
    }

    /// Consumes one resume if the budget allows it.
    pub const fn take(&mut self) -> ResumeDecision {
        if self.used >= self.limit {
            return ResumeDecision::Exhausted;
        }
        self.used += 1;
        ResumeDecision::Schedule
    }

    /// Raises `foreman_unreachable` for one obligation, once.
    ///
    /// The condition is attention and nothing else: `governor-core` has no
    /// path from a [`HealthLedger`] to an obligation transition, so this cannot
    /// close, fail, or reschedule anything.
    ///
    /// # Panics
    ///
    /// Panics only if the health ledger ever gains a fallible transition; it
    /// has none today.
    #[must_use]
    pub fn exhausted(
        ledger: &HealthLedger,
        condition: HealthConditionId,
        obligation: ObligationId,
        at: Timestamp,
    ) -> HealthLedger {
        ledger
            .raise(
                condition,
                HealthConditionKind::ForemanUnreachable,
                HealthScope::obligation(obligation),
                at,
            )
            .expect("raising attention is infallible")
            .or_unchanged(ledger.clone())
    }
}

/// The "never overlap an active or unknown ChatGPT turn" gate.
///
/// `docs/testing.md` GPT-005. The rule itself is
/// [`ForemanTurnState::permits_new_wake`]; this is the scheduler-shaped caller
/// that a suite can drive, so the test exercises "the scheduler asked" rather
/// than "the enum has a method".
#[derive(Debug, Clone)]
pub struct WakeGate {
    turn: ForemanTurn,
}

impl WakeGate {
    /// Wraps one observed physical turn.
    #[must_use]
    pub const fn new(turn: ForemanTurn) -> Self {
        Self { turn }
    }

    /// The observed turn.
    #[must_use]
    pub const fn turn(&self) -> &ForemanTurn {
        &self.turn
    }

    /// Replaces the observation.
    pub fn observe(&mut self, turn: ForemanTurn) {
        self.turn = turn;
    }

    /// Reports whether a wake may be activated right now.
    #[must_use]
    pub const fn may_activate(&self) -> bool {
        self.turn.permits_new_wake()
    }

    /// The state that is blocking a wake, if one is.
    #[must_use]
    pub const fn blocked_by(&self) -> Option<ForemanTurnState> {
        if self.turn.permits_new_wake() {
            None
        } else {
            Some(self.turn.state())
        }
    }
}
