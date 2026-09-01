//! Parallel callers against the single-writer actor.
//!
//! The actor serialises every mutation onto one connection on one OS thread
//! (`docs/adr/0002-rust-daemon-and-sqlite.md`), and every public method is a
//! synchronous call any thread may make. These tests are the missing
//! contention half of that claim: many OS threads hammering one store at
//! once, with the ledger required to come out exactly consistent — every
//! accepted operation appended once, racing duplicates converging on one
//! transition, and projection replay still equal to committed state.

mod support;

use governor_core::obligation::ObligationState;
use governor_store_sqlite::RecordWorkerStartedRequest;
use support::{Harness, count, open_turn, source, start_worker};

/// Concurrent independent writers: every operation commits exactly once.
#[test]
fn parallel_turns_all_commit_and_replay_matches() {
    const THREADS: usize = 8;
    const TURNS_PER_THREAD: usize = 25;

    let harness = Harness::new();
    let store = harness.open().expect("opening");

    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let store = &store;
            scope.spawn(move || {
                for turn_no in 0..TURNS_PER_THREAD {
                    let turn = open_turn(store);
                    // A distinct source identity per obligation, so none of
                    // these are duplicates of each other.
                    start_worker(store, turn.obligation, &format!("run-{thread}-{turn_no}"));
                }
            });
        }
    });

    let total = i64::try_from(THREADS * TURNS_PER_THREAD).expect("fits");
    let conn = harness.inspect();
    assert_eq!(count(&conn, "obligations"), total);
    let running: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM obligations WHERE state = 'running'",
            [],
            |row| row.get(0),
        )
        .expect("counting running obligations");
    assert_eq!(running, total, "every start committed exactly once");

    // Replay is the oracle: a lost or doubled write cannot fold back to the
    // committed projections.
    let verified = store.verify_projections().expect("replay equivalence");
    assert_eq!(i64::try_from(verified.obligations).expect("fits"), total);
}

/// Concurrent *identical* writers: the unique source identity admits one.
#[test]
fn racing_duplicate_starts_converge_on_one_transition() {
    const RACERS: usize = 16;

    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);

    let conn = harness.inspect();
    let events_before = count(&conn, "events");
    let transitions_before = count(&conn, "obligation_events");

    let request = RecordWorkerStartedRequest {
        obligation: turn.obligation,
        source: source("claude.init", "run-race", "start"),
        incarnation: governor_core::fence::IncarnationGeneration::FIRST,
    };
    let accepted: usize = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..RACERS)
            .map(|_| {
                let store = &store;
                let request = request.clone();
                scope.spawn(move || {
                    store
                        .record_worker_started(request)
                        .expect("a racing duplicate is accepted, not an error")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("racer thread"))
            .filter(|started| !started.duplicate)
            .count()
    });

    assert_eq!(accepted, 1, "exactly one racer performed the transition");
    assert_eq!(
        count(&conn, "events"),
        events_before + 1,
        "one appended event, however many racers"
    );
    assert_eq!(count(&conn, "obligation_events"), transitions_before + 1);
    assert_eq!(
        store
            .read_obligation(turn.obligation)
            .expect("snapshot")
            .state,
        ObligationState::Running
    );
    assert!(store.verify_projections().is_ok());
}
