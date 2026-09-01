//! The fake consequential-external-effect destination.
//!
//! The generic half of the same discipline [`crate::browser`] enforces for
//! wakes: an adapter must not be reachable before its intent is durable, and
//! the dispatch fence must be committed immediately before the call.
//!
//! This is where worker-command delivery is driven from in Phase 1. The
//! `worker_commands` and `worker_command_attempts` tables exist in the schema,
//! but no store operation writes them yet, so a fake worker transport would
//! have nothing to be fenced by. The
//! [`external_attempts`](governor_store_sqlite::Store::record_external_intent)
//! protocol *is* the fenced transport the worker adapter will use, and it is
//! drivable today, so that is what the suites exercise. The worker-specific
//! projection is a later gate.

use governor_core::effect::ExternalExecutionPermit;
use governor_core::id::ExternalAttemptId;
use governor_store_sqlite::AttemptEvidence;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _};
use std::path::Path;

use crate::scenario::token;

/// A destination that refuses to act before the store says it may.
#[derive(Debug)]
pub struct FakeExternalDestination {
    conn: Connection,
    delivered: Vec<ExternalAttemptId>,
    calls: Vec<&'static str>,
    next_reference: u32,
}

impl FakeExternalDestination {
    /// Attaches to a state root's database.
    ///
    /// # Panics
    ///
    /// Panics when the database cannot be opened for reading.
    #[must_use]
    pub fn attach(database: &Path) -> Self {
        Self {
            conn: Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("the fake destination's own read-only connection"),
            delivered: Vec::new(),
            calls: Vec::new(),
            next_reference: 0,
        }
    }

    /// Every effect this destination actually produced.
    #[must_use]
    pub fn delivered(&self) -> &[ExternalAttemptId] {
        &self.delivered
    }

    /// Every method that was invoked, in order.
    #[must_use]
    pub fn calls(&self) -> &[&'static str] {
        &self.calls
    }

    /// Asserts the destination was never reached.
    ///
    /// # Panics
    ///
    /// Panics naming the calls that did happen.
    pub fn assert_untouched(&self, context: &str) {
        assert!(
            self.calls.is_empty(),
            "{context}: the destination was reached: {:?}",
            self.calls
        );
    }

    /// Looks at the durable intent without producing an effect.
    ///
    /// Research test 1 made mechanical: the adapter genuinely looks, through
    /// its own connection, so "observable" means committed rather than merely
    /// written inside a transaction the writer still holds.
    ///
    /// # Panics
    ///
    /// Panics unless exactly one `intent_recorded` row exists for the permit's
    /// attempt.
    pub fn probe_intent(&mut self, permit: &ExternalExecutionPermit) {
        self.calls.push("probe_intent");
        let state = self.attempt_state(permit.attempt());
        assert_eq!(
            state.as_deref(),
            Some("intent_recorded"),
            "an adapter must never be reachable before its intent is durable; \
             the store showed {state:?}"
        );
    }

    /// Produces the effect the permit authorises.
    ///
    /// # Panics
    ///
    /// Panics unless the intent is durable **and** the dispatch fence is
    /// committed. `docs/data-model.md` puts the fence immediately before the
    /// call, so a call that arrives without it is the crash window the fence
    /// exists to close.
    pub fn deliver(&mut self, permit: &ExternalExecutionPermit) -> AttemptEvidence {
        self.calls.push("deliver");
        let attempt = permit.attempt();
        let state = self.attempt_state(attempt);
        assert_eq!(
            state.as_deref(),
            Some("intent_recorded"),
            "an adapter must never be reachable before its intent is durable; \
             the store showed {state:?}"
        );
        assert_eq!(
            self.dispatch_fence(attempt),
            Some(1),
            "the dispatch fence must be committed immediately before the call"
        );
        self.delivered.push(attempt);
        self.next_reference += 1;
        AttemptEvidence::new(token(&format!("dest-ref-{}", self.next_reference)))
    }

    fn attempt_state(&self, attempt: ExternalAttemptId) -> Option<String> {
        self.conn
            .query_row(
                "SELECT state FROM external_attempts WHERE external_attempt_id = ?1",
                rusqlite::params![attempt.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .expect("reading the attempt state")
    }

    fn dispatch_fence(&self, attempt: ExternalAttemptId) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT dispatched FROM external_attempts WHERE external_attempt_id = ?1",
                rusqlite::params![attempt.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .expect("reading the dispatch fence")
    }
}
