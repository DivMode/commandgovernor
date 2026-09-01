//! The kill-window driver: open, run, die at a named point, reopen, assert.
//!
//! `docs/testing.md` DB-002 asks for a crash injected around *each* multi-row
//! transition, with one answer every time: reopening yields the prior complete
//! state or the committed next state, never a half-transition. That is a
//! uniform oracle, so it is written once here rather than per test:
//!
//! 1. build the scenario prefix with the crash already armed — it targets one
//!    named point of one named operation, so the prefix is unaffected;
//! 2. fingerprint the whole database through an independent connection;
//! 3. run the operation under test;
//! 4. fingerprint again, **before** reopening, so recovery cannot mask a
//!    half-transition: a rejected operation must have changed nothing at all;
//! 5. reopen, which re-verifies projection replay and quarantines orphans, and
//!    require that to succeed.
//!
//! Step 5 is what makes replay the oracle rather than a hand-written expected
//! state: a half-committed transition is exactly what
//! [`Store::verify_projections`](governor_store_sqlite::Store::verify_projections)
//! refuses.
//!
//! A cell whose failpoint never fires is not a failed injection. It means the
//! operation does not pass through that point, the operation therefore
//! committed, and the same two assertions still apply.

use governor_store_sqlite::{Failpoint, Store, StoreError, StoreResult};

use crate::dump::{LedgerDump, assert_unchanged, dump_domain};
use crate::failpoints::{StoreCrash, TRANSACTION_FAILPOINTS, WRITE_OPERATIONS};
use crate::harness::Harness;

/// One cell of the crash matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillWindow {
    /// Name of the write operation to interrupt.
    pub op: &'static str,
    /// Point inside its transaction to interrupt at.
    pub point: Failpoint,
}

impl KillWindow {
    /// A readable label for assertion messages.
    #[must_use]
    pub fn label(self) -> String {
        format!(
            "{}/{}",
            self.op,
            crate::failpoints::store_failpoint_label(self.point)
        )
    }
}

/// Every operation crossed with every in-transaction failpoint.
///
/// `recover_startup` is included: it is a write operation like any other, but
/// it runs inside [`OpenStore::start`](governor_store_sqlite::OpenStore), so a
/// caller drives it by opening rather than by calling a method.
#[must_use]
pub fn transaction_windows() -> Vec<KillWindow> {
    let mut windows = Vec::with_capacity(WRITE_OPERATIONS.len() * TRANSACTION_FAILPOINTS.len());
    for op in WRITE_OPERATIONS {
        for point in TRANSACTION_FAILPOINTS {
            windows.push(KillWindow { op, point: *point });
        }
    }
    windows
}

/// What one cell of the matrix did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillReport {
    /// The cell.
    pub window: KillWindow,
    /// Whether the injected failure actually fired.
    pub fired: bool,
    /// Whether the operation under test reported success.
    pub committed: bool,
    /// The database as it stood before the operation.
    pub before: LedgerDump,
    /// The database as it stood after the operation, before any reopen.
    pub after: LedgerDump,
}

/// Runs one kill-window cell end to end.
///
/// `prefix` builds whatever state the operation needs; `operation` is the one
/// call under test. Both receive the store opened with the crash armed.
///
/// # Panics
///
/// Panics when the store cannot be opened, when a rejected operation changed
/// any row, or when reopening cannot replay the ledger.
pub fn run_kill_window<T>(
    harness: &Harness,
    window: KillWindow,
    prefix: impl FnOnce(&Store) -> T,
    operation: impl FnOnce(&Store, T) -> StoreResult<()>,
) -> KillReport {
    let crash = StoreCrash::at(window.op, window.point);
    let store = harness
        .open_with(Some(crash.boxed()))
        .unwrap_or_else(|error| panic!("{}: opening: {error}", window.label()));
    let carried = prefix(&store);

    let before = dump_domain(&harness.inspect());
    let outcome = operation(&store, carried);
    let after = dump_domain(&harness.inspect());

    match &outcome {
        Err(StoreError::Sqlite(_)) | Ok(()) => {}
        Err(other) => panic!(
            "{}: the operation failed for an unexpected reason: {other}",
            window.label()
        ),
    }
    if outcome.is_err() {
        assert_unchanged(
            &before,
            &after,
            &format!(
                "{}: an interrupted transaction must roll back completely",
                window.label()
            ),
        );
    }

    drop(store);
    let reopened = harness
        .open()
        .unwrap_or_else(|error| panic!("{}: reopening: {error}", window.label()));
    reopened
        .verify_projections()
        .unwrap_or_else(|error| panic!("{}: replay after reopen: {error}", window.label()));

    KillReport {
        window,
        fired: crash.fired(),
        committed: outcome.is_ok(),
        before,
        after,
    }
}

/// Reopens a state root repeatedly, proving each open replays cleanly.
///
/// `docs/testing.md` DB-007 asks for a duplicate-event replay across 100
/// restarts; this is the restart half, and the caller supplies what to do in
/// each incarnation.
///
/// # Panics
///
/// Panics when any open, or any replay verification, fails.
pub fn restart_loop(harness: &Harness, times: usize, mut body: impl FnMut(usize, &Store)) {
    for round in 0..times {
        let store = harness
            .open()
            .unwrap_or_else(|error| panic!("restart {round}: opening: {error}"));
        store
            .verify_projections()
            .unwrap_or_else(|error| panic!("restart {round}: replay: {error}"));
        body(round, &store);
        drop(store);
    }
}
