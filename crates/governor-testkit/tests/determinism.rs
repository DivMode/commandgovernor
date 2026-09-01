//! The property every other suite rests on: one seed replays exactly.
//!
//! # Coverage
//!
//! | Test | Requirement | Status |
//! | --- | --- | --- |
//! | [`one_seed_produces_an_identical_event_stream`] | `docs/testing.md` "Test architecture": deterministic fakes | covered here |
//! | [`one_seed_produces_an_identical_durable_state`] | as above, whole-database | covered here |
//! | [`different_seeds_produce_different_delivery_ids`] | DEL-001, invariant 17 | covered here |
//! | [`identity_and_randomness_are_two_streams`] | DEL-001: the correlation ID is not scheduling metadata | covered here |
//!
//! Why this matters more than it looks: every crash-matrix cell, every property
//! loop and every "zero rows changed" comparison in this crate is only evidence
//! if the same scenario really does produce the same bytes twice. If it did
//! not, a failing cell could not be reproduced and a passing one would prove
//! nothing in particular.
//!
//! And the converse has to hold too. A harness that produced identical
//! correlation IDs across seeds would make the SEC-003 and DEL-018 property
//! loops vacuous: they would be re-testing one value many times.

use governor_core::fence::DeliveryRevision;
use governor_core::obligation::Disposition;
use governor_testkit::dump::{LedgerDump, dump_domain};
use governor_testkit::harness::Harness;
use governor_testkit::rng::SeededPorts;
use governor_testkit::scenario::{
    LIVE_CLAIM, accepted_work, acknowledge, handoff, mint_claim, schedule_wake,
};

/// One representative scenario, driven end to end.
///
/// Deliberately broad: a real artifact publication, a browser wake through
/// acceptance, a resume revision, a claim, a handoff and a fenced ACK. Every
/// port a scenario can reach — the clock, the identity source, the CSPRNG and
/// the artifact key source — contributes to the result.
fn run_scenario(seed: u64) -> (LedgerDump, Vec<String>, String) {
    let harness = Harness::with_seed(seed);
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    let resumed = schedule_wake(
        &store,
        work.obligation,
        work.generation,
        DeliveryRevision::new(2),
    )
    .expect("a resume revision");

    let minted = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("a claim");
    handoff(&store, work.obligation, minted.claim).expect("a handoff");
    acknowledge(
        &store,
        work.obligation,
        work.generation,
        minted.claim,
        Disposition::Accepted,
    )
    .expect("a fenced ACK");
    drop(store);

    let conn = harness.inspect();
    let dump = dump_domain(&conn);
    let events = event_stream(&conn);
    (dump, events, resumed.delivery_id.expose_hex())
}

/// The immutable ledger, rendered in sequence order.
fn event_stream(conn: &rusqlite::Connection) -> Vec<String> {
    let mut statement = conn
        .prepare(
            "SELECT seq, kind, source_namespace, source_event_id, source_event_fence,
                    observed_at_ms, safe_metadata_json
               FROM events ORDER BY seq",
        )
        .expect("preparing the ledger query");
    let rows = statement
        .query_map([], |row| {
            Ok(format!(
                "{}|{}|{}/{}#{}|{}|{}",
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .expect("reading the ledger");
    rows.map(|row| row.expect("an event row")).collect()
}

#[test]
fn one_seed_produces_an_identical_event_stream() {
    let (_, first, _) = run_scenario(7);
    let (_, second, _) = run_scenario(7);
    assert!(!first.is_empty(), "the scenario must produce a ledger");
    assert_eq!(
        first, second,
        "the same seed must produce the same events, in the same order, at the \
         same instants, with the same identities"
    );
}

#[test]
fn one_seed_produces_an_identical_durable_state() {
    let (first, _, first_delivery) = run_scenario(11);
    let (second, _, second_delivery) = run_scenario(11);
    assert_eq!(
        first, second,
        "every projection row, every attempt, every artifact row must match"
    );
    assert_eq!(first_delivery, second_delivery);
}

#[test]
fn different_seeds_produce_different_delivery_ids() {
    let mut seen = std::collections::BTreeSet::new();
    let mut dumps = Vec::new();
    for seed in 1..=8u64 {
        let (dump, _, delivery) = run_scenario(seed);
        assert!(
            seen.insert(delivery.clone()),
            "seed {seed} reproduced a correlation ID another seed had already \
             drawn, which would make every possession fence untestable"
        );
        dumps.push(dump);
    }
    for pair in dumps.windows(2) {
        assert_ne!(
            pair[0], pair[1],
            "two seeds produced byte-identical durable state"
        );
    }
}

#[test]
fn identity_and_randomness_are_two_streams() {
    // If a testkit derived the correlation ID from the identity stream, every
    // invariant-17 assertion built on it would be vacuous. This is the guard.
    for seed in 0..1_024u64 {
        assert!(
            SeededPorts::streams_are_independent(seed),
            "seed {seed} collapsed identity and randomness into one stream"
        );
    }
}
