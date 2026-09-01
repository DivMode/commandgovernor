//! Ledger identity, fenced transitions, the three ACK layers, and replay.
//!
//! | Test | Requirement |
//! | --- | --- |
//! | [`a_duplicate_source_identity_never_appends_twice`] | DB-007, invariant 3 |
//! | [`source_uniqueness_survives_restart`] | DB-007 across reopen |
//! | [`a_duplicate_terminal_event_returns_the_first_obligation`] | invariant 3 |
//! | [`a_stale_version_is_refused_with_zero_row_changes`] | fenced CAS |
//! | [`a_stale_source_fence_is_refused_with_zero_row_changes`] | fenced CAS |
//! | [`a_stale_binding_generation_cannot_schedule_a_wake`] | invariant 9 |
//! | [`worker_completion_leaves_the_obligation_open`] | the mission invariant |
//! | [`only_a_fenced_disposition_closes_an_obligation`] | invariant 1 |
//! | [`an_ack_pins_then_releases_its_artifact`] | invariant 2 |
//! | [`the_claim_path_requires_the_random_correlation_id`] | invariant 17 |
//! | [`duplicate_scheduling_converges_on_one_delivery_identity`] | data model |
//! | [`projection_replay_equals_committed_state`] | DB-001, research test 11 |

mod support;

use governor_core::fence::{AttemptNo, DeliveryRevision, ObligationVersion};
use governor_core::obligation::{Disposition, ObligationState};
use governor_core::outbound::DeliveryState;
use governor_core::time::DurationMs;
use governor_store_sqlite::{
    AcknowledgeRequest, CreateOrClaimDeliveryRequest, DeliverHandoffRequest, MintClaimRequest,
    RecordWorkerStartedRequest, StoreError,
};
use support::{
    Harness, accept_wake, bind, count, open_turn, publish_result, schedule_wake, source,
    start_worker, token,
};

#[test]
fn a_duplicate_source_identity_never_appends_twice() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);

    let request = RecordWorkerStartedRequest {
        obligation: turn.obligation,
        source: source("claude.init", "run-1", "start"),
        incarnation: governor_core::fence::IncarnationGeneration::FIRST,
    };
    let first = store
        .record_worker_started(request.clone())
        .expect("first start");
    assert!(!first.duplicate);
    assert_eq!(first.state, ObligationState::Running);

    let conn = harness.inspect();
    let events_after_first = count(&conn, "events");
    let transitions_after_first = count(&conn, "obligation_events");

    for _ in 0..25 {
        let repeat = store
            .record_worker_started(request.clone())
            .expect("a replayed provider callback is accepted, not an error");
        assert!(repeat.duplicate, "the source identity was already recorded");
        assert_eq!(
            repeat.version, first.version,
            "the version does not advance"
        );
    }

    assert_eq!(count(&conn, "events"), events_after_first);
    assert_eq!(count(&conn, "obligation_events"), transitions_after_first);
}

#[test]
fn source_uniqueness_survives_restart() {
    let harness = Harness::new();
    let request = |obligation| RecordWorkerStartedRequest {
        obligation,
        source: source("claude.init", "run-1", "start"),
        incarnation: governor_core::fence::IncarnationGeneration::FIRST,
    };

    let store = harness.open().expect("first open");
    let turn = open_turn(&store);
    store
        .record_worker_started(request(turn.obligation))
        .expect("first start");
    let events = count(&harness.inspect(), "events");
    drop(store);

    // The unique index is durable, so a replay after a restart converges on
    // the row that is already there rather than on a fresh transition.
    for _ in 0..10 {
        let store = harness.open().expect("reopen");
        let repeat = store
            .record_worker_started(request(turn.obligation))
            .expect("replay after restart");
        assert!(repeat.duplicate);
        drop(store);
        assert_eq!(count(&harness.inspect(), "events"), events);
    }
}

#[test]
fn a_duplicate_terminal_event_returns_the_first_obligation() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");

    let first = publish_result(&store, turn.obligation, "run-1").expect("publication");
    assert_eq!(
        first.obligation.state,
        ObligationState::CompletedUnprocessed
    );

    let conn = harness.inspect();
    assert_eq!(count(&conn, "result_artifacts"), 1);

    for _ in 0..10 {
        let repeat = publish_result(&store, turn.obligation, "run-1")
            .expect("a duplicate terminal source is idempotent");
        assert!(repeat.obligation.duplicate);
        assert_eq!(
            repeat.artifact, first.artifact,
            "the existing artifact is returned, never a second one"
        );
    }
    assert_eq!(
        count(&conn, "result_artifacts"),
        1,
        "invariant 3: at most one result obligation and artifact"
    );
    assert_eq!(count(&conn, "obligations"), 1);
}

#[test]
fn a_stale_version_is_refused_with_zero_row_changes() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let current = store.read_obligation(turn.obligation).expect("snapshot");

    let conn = harness.inspect();
    let before = (
        count(&conn, "events"),
        count(&conn, "browser_deliveries"),
        count(&conn, "delivery_attempts"),
        count(&conn, "obligation_events"),
    );

    let error = schedule_wake(
        &store,
        turn.obligation,
        generation,
        ObligationVersion::FIRST,
        current.source.clone(),
    )
    .expect_err("a superseded version cannot schedule a wake");
    assert_eq!(error.conflict_code(), Some("stale_obligation_version"));

    assert_eq!(
        (
            count(&conn, "events"),
            count(&conn, "browser_deliveries"),
            count(&conn, "delivery_attempts"),
            count(&conn, "obligation_events"),
        ),
        before,
        "a rejected fence rolls the whole transaction back"
    );
}

#[test]
fn a_stale_source_fence_is_refused_with_zero_row_changes() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let current = store.read_obligation(turn.obligation).expect("snapshot");

    let conn = harness.inspect();
    let events = count(&conn, "events");

    let error = schedule_wake(
        &store,
        turn.obligation,
        generation,
        current.version,
        source("claude.result", "run-0", "final"),
    )
    .expect_err("a superseded source fact cannot schedule a wake");
    assert_eq!(error.conflict_code(), Some("stale_source_fence"));
    assert_eq!(count(&conn, "events"), events);
}

#[test]
fn a_stale_binding_generation_cannot_schedule_a_wake() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let first = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");

    // Rebinding supersedes every older generation.
    let second = bind(&store, "conv-B");
    assert!(second > first);

    let current = store.read_obligation(turn.obligation).expect("snapshot");
    let error = schedule_wake(
        &store,
        turn.obligation,
        first,
        current.version,
        current.source.clone(),
    )
    .expect_err("an old conversation cannot act on current work");
    assert_eq!(error.conflict_code(), Some("stale_binding_generation"));

    // Exactly one active binding row, as the partial unique index requires.
    let conn = harness.inspect();
    let active: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM foreman_bindings WHERE is_active = 1",
            [],
            |row| row.get(0),
        )
        .expect("counting active bindings");
    assert_eq!(active, 1);
}

#[test]
fn worker_completion_leaves_the_obligation_open() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(snapshot.state, ObligationState::CompletedUnprocessed);
    assert!(
        snapshot.open,
        "worker completion never closes delegated work"
    );

    let conn = harness.inspect();
    let closed: Option<i64> = conn
        .query_row(
            "SELECT closed_event_seq FROM obligations WHERE obligation_id = ?1",
            rusqlite::params![turn.obligation.to_string()],
            |row| row.get(0),
        )
        .expect("reading the projection");
    assert!(closed.is_none());
}

/// Drives one obligation all the way to a fenced ACK and returns the harness.
fn acknowledged() -> (Harness, governor_core::id::ObligationId) {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling a wake");
    accept_wake(&store, &claimed, generation, "msg-1");

    let minted = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: turn.obligation,
            presented_delivery_id: claimed.delivery_id.clone(),
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime: DurationMs::from_millis(60_000),
        })
        .expect("minting a claim from the accepted wake");
    store
        .deliver_handoff(DeliverHandoffRequest {
            obligation: turn.obligation,
            claim: minted.claim,
        })
        .expect("handing the result to the claiming foreman");

    let processing = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(processing.state, ObligationState::Processing);
    assert!(processing.open, "processing is not closed");

    let acked = store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation: turn.obligation,
            expected_version: processing.version,
            expected_source: processing.source.clone(),
            binding_generation: generation,
            claim: minted.claim,
            disposition: Disposition::Accepted,
        })
        .expect("a fully fenced ACK closes the obligation");
    assert_eq!(acked.obligation.state, ObligationState::Acknowledged);
    drop(store);
    (harness, turn.obligation)
}

#[test]
fn only_a_fenced_disposition_closes_an_obligation() {
    let (harness, obligation) = acknowledged();
    let store = harness.open().expect("reopen");
    let snapshot = store.read_obligation(obligation).expect("snapshot");
    assert_eq!(snapshot.state, ObligationState::Acknowledged);
    assert!(!snapshot.open);

    let conn = harness.inspect();
    let closed: Option<i64> = conn
        .query_row(
            "SELECT closed_event_seq FROM obligations WHERE obligation_id = ?1",
            rusqlite::params![obligation.to_string()],
            |row| row.get(0),
        )
        .expect("reading the projection");
    assert!(closed.is_some(), "the closure is recorded durably");

    let disposition: Option<String> = conn
        .query_row(
            "SELECT disposition FROM obligation_events
              WHERE obligation_id = ?1 AND disposition IS NOT NULL",
            rusqlite::params![obligation.to_string()],
            |row| row.get(0),
        )
        .expect("reading the disposition event");
    assert_eq!(disposition.as_deref(), Some("accepted"));
}

#[test]
fn an_ack_pins_then_releases_its_artifact() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");

    let conn = harness.inspect();
    let retention = || -> String {
        conn.query_row("SELECT retention_state FROM result_artifacts", [], |row| {
            row.get(0)
        })
        .expect("reading retention")
    };
    assert_eq!(
        retention(),
        "pinned",
        "invariant 2: an open obligation pins its artifact"
    );

    // Cancellation is the other explicit closing disposition, and closing is
    // what releases the pin — nothing else can.
    store
        .cancel_obligation(governor_store_sqlite::CancelObligationRequest {
            obligation: turn.obligation,
            source: source("cg.cli", "cancel-1", "user"),
        })
        .expect("the user cancels the work");
    assert_eq!(retention(), "eligible");
}

#[test]
fn the_claim_path_requires_the_random_correlation_id() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    accept_wake(&store, &claimed, generation, "msg-1");

    // The deterministic key is public knowledge; the correlation ID is not.
    let derived = governor_core::delivery::DeliveryKey::derive(
        turn.obligation,
        generation,
        DeliveryRevision::FIRST,
    );
    let forged = governor_core::delivery::DeliveryId::from_persisted_bytes(*derived.as_bytes());
    let error = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: turn.obligation,
            presented_delivery_id: forged,
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime: DurationMs::from_millis(60_000),
        })
        .expect_err("invariant 17: scheduling metadata cannot derive the wake ID");
    assert_eq!(error.conflict_code(), Some("unknown_delivery_id"));

    let conn = harness.inspect();
    assert_eq!(count(&conn, "foreman_claims"), 0);
}

#[test]
fn duplicate_scheduling_converges_on_one_delivery_identity() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");

    let first = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("first scheduling");
    assert!(first.created);
    assert_eq!(first.attempt, AttemptNo::FIRST);

    // A second scheduling of the same logical revision finds the same row. It
    // cannot claim a second attempt on top of a live one, and says so.
    let error = store
        .create_or_claim_delivery(CreateOrClaimDeliveryRequest {
            obligation: turn.obligation,
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            revision: DeliveryRevision::FIRST,
            attempt_budget: 3,
            wake_protocol: token("composer.v1"),
        })
        .expect_err("an attempt already owns the external effect");
    assert_eq!(error.conflict_code(), Some("illegal_delivery_transition"));

    let conn = harness.inspect();
    assert_eq!(
        count(&conn, "browser_deliveries"),
        1,
        "never a second physical revision identity"
    );
    assert_eq!(count(&conn, "delivery_attempts"), 1);
}

#[test]
fn an_accepted_revision_is_frozen_and_never_resent() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");

    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    accept_wake(&store, &claimed, generation, "msg-1");

    let error = store
        .create_or_claim_delivery(CreateOrClaimDeliveryRequest {
            obligation: turn.obligation,
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            revision: DeliveryRevision::FIRST,
            attempt_budget: 3,
            wake_protocol: token("composer.v1"),
        })
        .expect_err("invariant 13: accepted is never automatically resent");
    assert_eq!(error.conflict_code(), Some("delivery_revision_frozen"));
}

#[test]
fn projection_replay_equals_committed_state() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    accept_wake(&store, &claimed, generation, "msg-1");

    let verified = store.verify_projections().expect("replay equivalence");
    assert_eq!(verified.obligations, 1);
    assert_eq!(verified.deliveries, 1);
    assert!(verified.verified_through.is_some());

    // The watermark survives, and the next process sees how far the last one
    // had proven.
    drop(store);
    let store = harness.open().expect("reopen");
    assert_eq!(
        store.startup().previously_verified_through,
        verified.verified_through
    );
    assert_eq!(store.startup().projections.obligations, 1);
}

#[test]
fn a_tampered_projection_row_fails_closed_on_replay() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    drop(store);

    // Rewrite the projection so it disagrees with the ledger it came from.
    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute(
        "UPDATE obligations SET state = 'acknowledged', closed_event_seq = source_event_seq",
        [],
    )
    .expect("tampering with the projection");
    drop(conn);

    let error = harness
        .open()
        .expect_err("a projection that disagrees with its ledger fails closed");
    match error {
        StoreError::RepairNeeded(repair) => {
            assert!(
                repair
                    .mismatches
                    .iter()
                    .any(|mismatch| mismatch.column == "state"),
                "the disagreeing column is named: {:?}",
                repair.mismatches
            );
        }
        other => panic!("expected a repair-needed failure, got {other:?}"),
    }
    assert!(
        matches!(harness.open(), Err(StoreError::RepairNeeded(_))),
        "and it keeps failing closed rather than repairing itself"
    );
}

#[test]
fn a_delivery_attempt_is_claimed_before_the_send_fence_is_armed() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");

    let conn = harness.inspect();
    let state: String = conn
        .query_row(
            "SELECT state FROM delivery_attempts WHERE attempt_no = 1",
            [],
            |row| row.get(0),
        )
        .expect("reading the attempt");
    assert_eq!(
        state, "claimed",
        "invariant 10: claimed is durable before any browser I/O"
    );
    let armed: Option<i64> = conn
        .query_row(
            "SELECT activation_armed_event_seq FROM delivery_attempts WHERE attempt_no = 1",
            [],
            |row| row.get(0),
        )
        .expect("reading the fence column");
    assert!(armed.is_none(), "the Send fence is not armed yet");

    accept_wake(&store, &claimed, generation, "msg-1");
    let armed: Option<i64> = conn
        .query_row(
            "SELECT activation_armed_event_seq FROM delivery_attempts WHERE attempt_no = 1",
            [],
            |row| row.get(0),
        )
        .expect("reading the fence column");
    assert!(
        armed.is_some(),
        "invariant 11: the fence is durable before the exact Send"
    );

    let delivery_state: String = conn
        .query_row("SELECT state FROM browser_deliveries", [], |row| row.get(0))
        .expect("reading the delivery");
    assert_eq!(delivery_state, "accepted");
    assert_eq!(
        store
            .verify_projections()
            .expect("replay still matches")
            .deliveries,
        1
    );
    let _ = DeliveryState::Accepted;
}
