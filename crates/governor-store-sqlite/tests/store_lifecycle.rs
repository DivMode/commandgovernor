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
//! | [`a_bounded_retry_keeps_the_revision_and_its_correlation_id`] | data model |
//! | [`a_second_revision_is_refused_while_the_first_is_live`] | one live revision |
//! | [`a_superseded_revision_cannot_claim_another_attempt`] | one live revision |
//! | [`an_ack_records_when_the_released_bytes_may_go`] | ACK layer 3, retention |
//! | [`claim_expiry_returns_the_attention_it_came_from`] | OBL-002, ART-002 |
//! | [`an_expired_claim_can_be_replaced_by_a_new_one`] | OBL-008 reclaim |
//! | [`the_displaced_claim_cannot_ack_after_a_reclaim`] | OBL-004 |
//! | [`a_live_claim_cannot_be_expired`] | claim/ACK fencing |
//! | [`an_expired_claim_cannot_deliver_a_handoff`] | claim/ACK fencing |
//! | [`projection_replay_equals_committed_state`] | DB-001, research test 11 |
//! | [`replay_still_matches_across_a_claim_expiry`] | DB-001 with expiry |
//! | [`attention_is_refused_for_closed_work`] | health conditions are attention |
//! | [`attention_must_name_the_artifact_the_obligation_pins`] | health condition scope |
//! | [`a_health_condition_replays_from_its_events`] | DB-001 over `health_conditions` |
//! | [`a_tampered_binding_ladder_is_caught_by_replay`] | DB-001 over `foreman_bindings`, invariant 9 |
//! | [`a_tampered_binding_activity_flag_is_caught_by_replay`] | DB-001 over `foreman_bindings`, invariant 9 |
//! | [`a_tampered_claim_row_is_caught_by_replay`] | DB-001 over `foreman_claims` |
//! | [`a_delivery_mismatch_names_the_key_and_never_the_correlation_id`] | invariant 17, SEC-001 |

mod support;

use governor_core::delivery::DeliveryKey;
use governor_core::fence::{AttemptNo, BindingGeneration, DeliveryRevision, ObligationVersion};
use governor_core::id::{ClaimId, ObligationId};
use governor_core::obligation::{Disposition, ObligationState};
use governor_core::outbound::{DeliveryState, FailureClass};
use governor_core::time::DurationMs;
use governor_core::worker_evidence::WorkerFailureClass;
use governor_store_sqlite::{
    AcknowledgeRequest, ClaimedDelivery, CreateOrClaimDeliveryRequest, DeliverHandoffRequest,
    DeliveryOutcome, ExpireClaimRequest, MintClaimRequest, RaiseForemanUnreachableRequest,
    RecordDeliveryOutcomeRequest, RecordWorkerFailureRequest, RecordWorkerStartedRequest,
    ResultArtifactMissingRequest, Store, StoreError,
};
use support::{
    Harness, StepClock, accept_wake, bind, count, open_turn, publish_result, schedule_wake, source,
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

/// Retention delay the ACK fixtures apply. Long enough that nothing in these
/// suites is deletable, so a released artifact is released and not gone.
const RETENTION_GRACE: DurationMs = DurationMs::from_millis(86_400_000);

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
            retention_grace: RETENTION_GRACE,
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
fn a_bounded_retry_keeps_the_revision_and_its_correlation_id() {
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

    // A proven pre-Send failure is the one case a bounded retry is safe.
    store
        .record_delivery_outcome(RecordDeliveryOutcomeRequest {
            delivery_id: first.delivery_id.clone(),
            attempt: first.attempt,
            outcome: DeliveryOutcome::Failed {
                failure: FailureClass::ComposerNotReady,
            },
        })
        .expect("recording a proven pre-submit failure");

    let second = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("a bounded retry claims the next attempt");
    assert!(
        !second.created,
        "the revision already existed and was found by its deterministic key"
    );
    assert_eq!(
        second.delivery_id, first.delivery_id,
        "one revision keeps one correlation ID for its whole life"
    );
    assert_eq!(second.revision, first.revision);
    assert_eq!(second.attempt, AttemptNo::new(2));

    let conn = harness.inspect();
    assert_eq!(
        count(&conn, "browser_deliveries"),
        1,
        "never a second physical revision identity"
    );
    assert_eq!(count(&conn, "delivery_attempts"), 2);
    assert!(store.verify_projections().is_ok());
}

/// Builds the request for one further wake revision of the same obligation.
fn revision_request(
    obligation: ObligationId,
    generation: BindingGeneration,
    version: ObligationVersion,
    fenced_source: governor_core::fence::SourceRef,
    revision: u32,
) -> CreateOrClaimDeliveryRequest {
    CreateOrClaimDeliveryRequest {
        obligation,
        binding_generation: generation,
        expected_version: version,
        expected_source: fenced_source,
        revision: DeliveryRevision::new(revision),
        attempt_budget: 3,
        wake_protocol: token("composer.v1"),
    }
}

#[test]
fn a_second_revision_is_refused_while_the_first_is_live() {
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
    .expect("scheduling revision one");

    let conn = harness.inspect();
    let deliveries = count(&conn, "browser_deliveries");
    let events = count(&conn, "events");

    // Revision one holds a claimed attempt, so it could still act. A second
    // revision now would be a second chance at the same external effect.
    let error = store
        .create_or_claim_delivery(revision_request(
            turn.obligation,
            generation,
            snapshot.version,
            snapshot.source.clone(),
            2,
        ))
        .expect_err("revision one could still act");
    assert_eq!(error.conflict_code(), Some("delivery_revision_still_live"));
    assert_eq!(
        count(&conn, "browser_deliveries"),
        deliveries,
        "zero rows changed"
    );
    assert_eq!(count(&conn, "events"), events);

    // Once revision one is settled — a proven pre-submit failure is terminal
    // for the aggregate — the successor may be created.
    store
        .record_delivery_outcome(RecordDeliveryOutcomeRequest {
            delivery_id: first.delivery_id.clone(),
            attempt: first.attempt,
            outcome: DeliveryOutcome::Failed {
                failure: FailureClass::ComposerNotReady,
            },
        })
        .expect("settling revision one");
    let second = store
        .create_or_claim_delivery(revision_request(
            turn.obligation,
            generation,
            snapshot.version,
            snapshot.source.clone(),
            2,
        ))
        .expect("revision two after the first settled");
    assert!(second.created);
    assert_ne!(second.delivery_id, first.delivery_id);
    assert!(store.verify_projections().is_ok());
}

#[test]
fn a_superseded_revision_cannot_claim_another_attempt() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");

    // Revision one fails with attempt budget to spare, and revision two takes
    // over.
    let first = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling revision one");
    store
        .record_delivery_outcome(RecordDeliveryOutcomeRequest {
            delivery_id: first.delivery_id.clone(),
            attempt: first.attempt,
            outcome: DeliveryOutcome::Failed {
                failure: FailureClass::ComposerNotReady,
            },
        })
        .expect("settling revision one");
    store
        .create_or_claim_delivery(revision_request(
            turn.obligation,
            generation,
            snapshot.version,
            snapshot.source.clone(),
            2,
        ))
        .expect("revision two");

    let conn = harness.inspect();
    let attempts = count(&conn, "delivery_attempts");
    let events = count(&conn, "events");

    // A bounded retry of revision one would normally be legal — the budget is
    // not spent — but the successor makes it a resurrection.
    let error = store
        .create_or_claim_delivery(revision_request(
            turn.obligation,
            generation,
            snapshot.version,
            snapshot.source.clone(),
            1,
        ))
        .expect_err("a superseded revision may never act again");
    assert_eq!(error.conflict_code(), Some("delivery_revision_superseded"));
    assert_eq!(
        count(&conn, "delivery_attempts"),
        attempts,
        "zero rows changed"
    );
    assert_eq!(count(&conn, "events"), events);
    assert!(store.verify_projections().is_ok());
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
fn a_tampered_binding_ladder_is_caught_by_replay() {
    // Invariant 9 fences every wake and every claim on `binding_generation`,
    // so the generation ladder is load-bearing and replay must derive it from
    // the events rather than trust the rows.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    bind(&store, "conv-A");
    let second = bind(&store, "conv-B");
    assert_eq!(second.get(), 2, "the ledger assigns highest + 1");
    store.verify_projections().expect("replay before tampering");
    drop(store);

    // Renumber the displaced generation. Nothing in the rows contradicts
    // itself; only the ledger knows the first `bound` event took generation 1.
    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute(
        "UPDATE foreman_bindings SET binding_generation = 7 WHERE binding_generation = 1",
        [],
    )
    .expect("tampering with the ladder");
    drop(conn);

    let error = harness.open().expect_err("the renumbering fails closed");
    let StoreError::RepairNeeded(repair) = error else {
        panic!("expected a repair-needed failure");
    };
    assert!(
        repair
            .mismatches
            .iter()
            .any(|mismatch| mismatch.table == "foreman_bindings"),
        "the binding ladder must be named: {:?}",
        repair.mismatches
    );
}

#[test]
fn a_tampered_binding_activity_flag_is_caught_by_replay() {
    // Which generation is active is the other half of invariant 9's fence.
    // The partial unique index stops a *second* row claiming to be active, so
    // the reachable corruption is the opposite one: no row active at all,
    // while the ledger's last `bound` event says generation 2 is.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    bind(&store, "conv-A");
    bind(&store, "conv-B");
    drop(store);

    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute(
        "UPDATE foreman_bindings SET is_active = 0 WHERE binding_generation = 2",
        [],
    )
    .expect("retiring the active binding behind the ledger's back");
    drop(conn);

    let error = harness
        .open()
        .expect_err("a silently retired binding fails closed");
    let StoreError::RepairNeeded(repair) = error else {
        panic!("expected a repair-needed failure");
    };
    assert!(
        repair.mismatches.iter().any(|mismatch| {
            mismatch.table == "foreman_bindings" && mismatch.column == "is_active"
        }),
        "{:?}",
        repair.mismatches
    );
}

#[test]
fn a_tampered_claim_row_is_caught_by_replay() {
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
    store
        .mint_foreman_claim(MintClaimRequest {
            obligation: turn.obligation,
            presented_delivery_id: claimed.delivery_id.clone(),
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime: DurationMs::from_millis(60_000),
        })
        .expect("minting a claim");
    store.verify_projections().expect("replay before tampering");
    drop(store);

    // A claim row that says the obligation was already dealt with, without any
    // event saying so.
    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute("UPDATE foreman_claims SET state = 'closed'", [])
        .expect("tampering with the claim lifecycle");
    drop(conn);

    let error = harness.open().expect_err("the claim row fails closed");
    let StoreError::RepairNeeded(repair) = error else {
        panic!("expected a repair-needed failure");
    };
    assert!(
        repair
            .mismatches
            .iter()
            .any(|mismatch| mismatch.table == "foreman_claims" && mismatch.column == "state"),
        "{:?}",
        repair.mismatches
    );
}

#[test]
fn a_delivery_mismatch_names_the_key_and_never_the_correlation_id() {
    // A `RepairNeeded` message is printed to stderr by the daemon and written
    // to its log, so whatever names the disagreeing row is published. For a
    // browser delivery that must be the deterministic, non-secret
    // `delivery_key` — never `delivery_id`, which `foreman_resume` accepts as
    // proof of possession.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let wake = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    accept_wake(&store, &wake, generation, "msg-1");

    let correlation_hex = wake.delivery_id.expose_hex();
    let key_hex =
        DeliveryKey::derive(turn.obligation, generation, DeliveryRevision::FIRST).to_hex();
    drop(store);

    // Disagree with the ledger on both halves of the delivery projection.
    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute("UPDATE browser_deliveries SET state = 'failed'", [])
        .expect("tampering with the delivery projection");
    conn.execute("UPDATE delivery_attempts SET state = 'failed'", [])
        .expect("tampering with the attempt projection");
    drop(conn);

    let error = harness.open().expect_err("the disagreement fails closed");
    let StoreError::RepairNeeded(repair) = error else {
        panic!("expected a repair-needed failure");
    };
    assert_eq!(
        repair.mismatches.len(),
        2,
        "both halves disagree: {:?}",
        repair.mismatches
    );

    // The rendered message is the surface that leaks, so assert on it and not
    // only on the struct.
    let rendered = format!("{repair}");
    for mismatch in &repair.mismatches {
        assert!(
            mismatch.row.starts_with(&key_hex),
            "{} named the row {:?}, which is not the delivery key",
            mismatch.table,
            mismatch.row
        );
        assert!(!mismatch.row.contains(&correlation_hex));
    }
    assert!(
        !rendered.contains(&correlation_hex),
        "the correlation ID reached a printable error: {rendered}"
    );
    assert!(
        rendered.contains(&key_hex),
        "the operator still gets a way to address the row: {rendered}"
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

/// Drives one obligation to an attention state, accepts a wake for it, mints a
/// claim, and hands the work over.
///
/// `needs_input` is deliberately absent: Phase 1 has no store write path to it
/// — there is no input-boundary event kind — so it cannot be driven from here.
/// Its expiry behaviour is the state machine's, and `governor-core` owns it.
fn handed_over(
    store: &Store,
    attention: ObligationState,
    lifetime: DurationMs,
) -> (ObligationId, BindingGeneration, ClaimedDelivery, ClaimId) {
    let turn = open_turn(store);
    let generation = bind(store, "conv-A");
    start_worker(store, turn.obligation, "run-1");
    match attention {
        ObligationState::CompletedUnprocessed => {
            publish_result(store, turn.obligation, "run-1").expect("publication");
        }
        ObligationState::Failed => {
            store
                .record_worker_failure(RecordWorkerFailureRequest {
                    obligation: turn.obligation,
                    source: source("claude.result", "run-1", "error"),
                    incarnation: governor_core::fence::IncarnationGeneration::FIRST,
                    failure: WorkerFailureClass::StructuredError,
                })
                .expect("a verified terminal worker failure");
        }
        other => panic!("{other:?} is not a reachable attention state here"),
    }

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(snapshot.state, attention);
    let claimed = schedule_wake(
        store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling a wake");
    accept_wake(store, &claimed, generation, "msg-1");

    let minted = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: turn.obligation,
            presented_delivery_id: claimed.delivery_id.clone(),
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime,
        })
        .expect("minting a claim from the accepted wake");
    store
        .deliver_handoff(DeliverHandoffRequest {
            obligation: turn.obligation,
            claim: minted.claim,
        })
        .expect("handing the work to the claiming foreman");

    (turn.obligation, generation, claimed, minted.claim)
}

/// The bounded lifetime these tests mint claims with.
///
/// Long enough that the handoff a scenario delivers immediately afterwards
/// happens under a live claim — the store refuses a handoff on a lapsed one —
/// and lapsed deterministically by advancing the shared clock past it.
const CLAIM_LIFETIME: DurationMs = DurationMs::from_millis(60_000);

/// Advancing the shared clock by this much lapses a `CLAIM_LIFETIME` claim.
const PAST_CLAIM_LIFETIME: i64 = 61_000;

#[test]
fn an_ack_records_when_the_released_bytes_may_go() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let grace = DurationMs::from_millis(10_000);
    let (obligation, generation, _wake, claim) = handed_over(
        &store,
        ObligationState::CompletedUnprocessed,
        DurationMs::from_millis(60_000),
    );

    let conn = harness.inspect();
    let row = || -> (String, Option<i64>) {
        conn.query_row(
            "SELECT retention_state, eligible_for_delete_at_ms FROM result_artifacts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("reading the artifact row")
    };
    assert_eq!(
        row(),
        ("pinned".to_owned(), None),
        "a pinned artifact carries no deletion instant"
    );

    let processing = store.read_obligation(obligation).expect("snapshot");
    let request = AcknowledgeRequest {
        obligation,
        expected_version: processing.version,
        expected_source: processing.source.clone(),
        binding_generation: generation,
        claim,
        disposition: Disposition::Accepted,
        retention_grace: grace,
    };
    store
        .acknowledge_obligation(request.clone())
        .expect("a fully fenced ACK");

    let acked_at: i64 = conn
        .query_row(
            "SELECT observed_at_ms FROM events WHERE kind = 'foreman_acked'",
            [],
            |row| row.get(0),
        )
        .expect("the ACK is in the ledger");
    assert_eq!(
        row(),
        (
            "eligible".to_owned(),
            Some(acked_at + i64::try_from(grace.as_millis()).expect("fits"))
        ),
        "the ACK instant plus the delay it was given, and nothing invented"
    );

    // An idempotent repeat must not push the deletion further out.
    store
        .acknowledge_obligation(request)
        .expect("an exact repeat is idempotent success");
    assert_eq!(
        row().1,
        Some(acked_at + i64::try_from(grace.as_millis()).expect("fits"))
    );
}

#[test]
fn claim_expiry_returns_the_attention_it_came_from() {
    for attention in [
        ObligationState::CompletedUnprocessed,
        ObligationState::Failed,
    ] {
        let harness = Harness::new();
        let (store, clock) = harness.open_clocked().expect("opening");
        let (obligation, _generation, _wake, claim) =
            handed_over(&store, attention, CLAIM_LIFETIME);
        let processing = store.read_obligation(obligation).expect("snapshot");
        assert_eq!(processing.state, ObligationState::Processing);
        clock.advance(PAST_CLAIM_LIFETIME);

        let expired = store
            .expire_foreman_claim(ExpireClaimRequest { obligation, claim })
            .expect("a lapsed claim expires");
        assert_eq!(
            expired.obligation.state, attention,
            "expiry restores exactly the attention state the claim was taken from"
        );
        assert!(
            expired.obligation.version > processing.version,
            "the transition advances the compare-and-swap version"
        );

        let snapshot = store.read_obligation(obligation).expect("snapshot");
        assert!(snapshot.open, "expiry never closes work");
        assert!(snapshot.claim.is_none(), "the claim no longer holds it");
        assert_eq!(snapshot.result_artifact, processing.result_artifact);

        let conn = harness.inspect();
        let (state, released): (String, Option<i64>) = conn
            .query_row(
                "SELECT state, released_event_seq FROM foreman_claims WHERE claim_id = ?1",
                rusqlite::params![claim.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("reading the claim row");
        assert_eq!(state, "expired");
        assert!(released.is_some(), "the release is recorded durably");
        let closed: Option<i64> = conn
            .query_row(
                "SELECT closed_event_seq FROM obligations WHERE obligation_id = ?1",
                rusqlite::params![obligation.to_string()],
                |row| row.get(0),
            )
            .expect("reading the projection");
        assert!(closed.is_none(), "no closing disposition was recorded");

        if attention == ObligationState::CompletedUnprocessed {
            let retention: (String, Option<i64>) = conn
                .query_row(
                    "SELECT retention_state, eligible_for_delete_at_ms FROM result_artifacts",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("reading the artifact row");
            assert_eq!(
                retention,
                ("pinned".to_owned(), None),
                "ART-002: expiry releases nothing"
            );
        }
    }
}

#[test]
fn a_live_claim_cannot_be_expired() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let (obligation, _generation, _wake, claim) = handed_over(
        &store,
        ObligationState::CompletedUnprocessed,
        DurationMs::from_millis(60_000),
    );
    let conn = harness.inspect();
    let events = count(&conn, "events");
    let transitions = count(&conn, "obligation_events");

    let error = store
        .expire_foreman_claim(ExpireClaimRequest { obligation, claim })
        .expect_err("a claim that still has time left is not expired");
    assert_eq!(error.conflict_code(), Some("obligation_already_claimed"));
    assert_eq!(count(&conn, "events"), events);
    assert_eq!(count(&conn, "obligation_events"), transitions);
    assert_eq!(
        store.read_obligation(obligation).expect("snapshot").state,
        ObligationState::Processing
    );
}

#[test]
fn an_expired_claim_cannot_deliver_a_handoff() {
    let harness = Harness::new();
    let (store, clock) = harness.open_clocked().expect("opening");

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
            lifetime: CLAIM_LIFETIME,
        })
        .expect("minting a claim from the accepted wake");

    // The claim's lease runs out before the handoff arrives — the expiry
    // sweep has not run, so the obligation is still `claimed_by_foreman` and
    // the claim row still says `live`. Its lifetime, not its row state, is
    // what stops authorising the mutation.
    clock.advance(PAST_CLAIM_LIFETIME);

    let conn = harness.inspect();
    let events = count(&conn, "events");
    let transitions = count(&conn, "obligation_events");

    let error = store
        .deliver_handoff(DeliverHandoffRequest {
            obligation: turn.obligation,
            claim: minted.claim,
        })
        .expect_err("a lapsed claim no longer authorises the handoff");
    assert_eq!(error.conflict_code(), Some("expired_claim"));

    assert_eq!(count(&conn, "events"), events, "zero rows changed");
    assert_eq!(count(&conn, "obligation_events"), transitions);
    let after = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(
        after.state,
        ObligationState::ClaimedByForeman,
        "the refused handoff moved nothing"
    );
    assert!(after.open, "the work is still owed");
}

/// Expires a lapsed claim and mints a new one from the same accepted wake.
fn reclaimed(
    store: &Store,
    clock: &StepClock,
) -> (ObligationId, BindingGeneration, ClaimId, ClaimId) {
    let (obligation, generation, wake, first) =
        handed_over(store, ObligationState::CompletedUnprocessed, CLAIM_LIFETIME);
    clock.advance(PAST_CLAIM_LIFETIME);
    let expired = store
        .expire_foreman_claim(ExpireClaimRequest {
            obligation,
            claim: first,
        })
        .expect("a lapsed claim expires");
    assert!(
        expired.wake_repointed,
        "the accepted wake follows the obligation it is still about"
    );

    let snapshot = store.read_obligation(obligation).expect("snapshot");
    let second = store
        .mint_foreman_claim(MintClaimRequest {
            obligation,
            presented_delivery_id: wake.delivery_id.clone(),
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime: DurationMs::from_millis(60_000),
        })
        .expect("OBL-008: an obligation may be reclaimed after claim expiry")
        .claim;
    assert_ne!(first, second, "a reclaim mints a genuinely new claim");
    (obligation, generation, first, second)
}

#[test]
fn an_expired_claim_can_be_replaced_by_a_new_one() {
    let harness = Harness::new();
    let (store, clock) = harness.open_clocked().expect("opening");
    let (obligation, _generation, first, second) = reclaimed(&store, &clock);

    let snapshot = store.read_obligation(obligation).expect("snapshot");
    assert_eq!(snapshot.state, ObligationState::ClaimedByForeman);
    assert_eq!(snapshot.claim, Some(second));

    let conn = harness.inspect();
    let states: Vec<(String, String)> = {
        let mut statement = conn
            .prepare("SELECT claim_id, state FROM foreman_claims ORDER BY claim_id")
            .expect("preparing");
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("querying claims");
        rows.map(|row| row.expect("claim row")).collect()
    };
    assert_eq!(states.len(), 2, "the expired claim is history, not deleted");
    assert!(
        states
            .iter()
            .any(|(id, state)| id == &first.to_string() && state == "expired")
    );
    assert!(
        states
            .iter()
            .any(|(id, state)| id == &second.to_string() && state == "live")
    );
}

#[test]
fn the_displaced_claim_cannot_ack_after_a_reclaim() {
    let harness = Harness::new();
    let (store, clock) = harness.open_clocked().expect("opening");
    let (obligation, generation, first, second) = reclaimed(&store, &clock);
    store
        .deliver_handoff(DeliverHandoffRequest {
            obligation,
            claim: second,
        })
        .expect("handing the work to the new claim");

    let processing = store.read_obligation(obligation).expect("snapshot");
    let conn = harness.inspect();
    let events = count(&conn, "events");
    let transitions = count(&conn, "obligation_events");

    let error = store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation,
            expected_version: processing.version,
            expected_source: processing.source.clone(),
            binding_generation: generation,
            claim: first,
            disposition: Disposition::Accepted,
            retention_grace: RETENTION_GRACE,
        })
        .expect_err("OBL-004: the displaced claim cannot close the work");
    assert_eq!(error.conflict_code(), Some("stale_claim"));

    assert_eq!(count(&conn, "events"), events, "zero rows changed");
    assert_eq!(count(&conn, "obligation_events"), transitions);
    let after = store.read_obligation(obligation).expect("snapshot");
    assert_eq!(after.state, ObligationState::Processing);
    assert_eq!(after.version, processing.version);
    assert!(after.open);
    let retention: String = conn
        .query_row("SELECT retention_state FROM result_artifacts", [], |row| {
            row.get(0)
        })
        .expect("reading retention");
    assert_eq!(retention, "pinned", "the artifact is still required");
}

#[test]
fn replay_still_matches_across_a_claim_expiry() {
    let harness = Harness::new();
    let (store, clock) = harness.open_clocked().expect("opening");
    let (obligation, generation, _first, second) = reclaimed(&store, &clock);
    store
        .deliver_handoff(DeliverHandoffRequest {
            obligation,
            claim: second,
        })
        .expect("handing the work over again");
    let processing = store.read_obligation(obligation).expect("snapshot");
    store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation,
            expected_version: processing.version,
            expected_source: processing.source.clone(),
            binding_generation: generation,
            claim: second,
            disposition: Disposition::Accepted,
            retention_grace: RETENTION_GRACE,
        })
        .expect("the second claim closes the work");

    // The ledger now carries a claim, a handoff, an expiry, a second claim, a
    // second handoff and an ACK for one obligation. DB-001 must still hold.
    let verified = store.verify_projections().expect("replay equivalence");
    assert_eq!(verified.obligations, 1);
    assert_eq!(verified.deliveries, 1);

    drop(store);
    let store = harness.open().expect("reopen replays before serving");
    assert_eq!(store.startup().projections.obligations, 1);
    assert_eq!(
        store.read_obligation(obligation).expect("snapshot").state,
        ObligationState::Acknowledged
    );
}

// --- Durable health conditions ------------------------------------------------

#[test]
fn attention_is_refused_for_closed_work() {
    let (harness, obligation) = acknowledged();
    let store = harness.open().expect("reopen");
    let before = store.read_obligation(obligation).expect("snapshot");
    let events = count(&harness.inspect(), "events");

    let error = store
        .raise_foreman_unreachable(RaiseForemanUnreachableRequest { obligation })
        .expect_err("nobody is owed anything, so there is nothing to attend to");
    assert_eq!(error.conflict_code(), Some("obligation_closed"));

    assert_eq!(count(&harness.inspect(), "health_conditions"), 0);
    assert_eq!(
        count(&harness.inspect(), "events"),
        events,
        "a refused raise appends nothing"
    );
    assert_eq!(store.read_obligation(obligation).expect("snapshot"), before);
}

#[test]
fn attention_must_name_the_artifact_the_obligation_pins() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let events = count(&harness.inspect(), "events");

    let error = store
        .raise_result_artifact_missing(ResultArtifactMissingRequest {
            obligation: turn.obligation,
            artifact: support::id(9_999),
        })
        .expect_err("an artifact this obligation does not require");
    assert_eq!(error.conflict_code(), Some("illegal_obligation_transition"));
    assert_eq!(count(&harness.inspect(), "health_conditions"), 0);
    assert_eq!(count(&harness.inspect(), "events"), events);

    // Resolving one that was never opened is convergence, not an error, and it
    // writes nothing either.
    let artifact = store
        .read_obligation(turn.obligation)
        .expect("snapshot")
        .result_artifact
        .expect("a published result");
    assert!(
        store
            .resolve_result_artifact_missing(ResultArtifactMissingRequest {
                obligation: turn.obligation,
                artifact,
            })
            .expect("nothing to resolve is not a refusal")
            .duplicate
    );
    assert_eq!(count(&harness.inspect(), "events"), events);
}

#[test]
fn a_health_condition_replays_from_its_events() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let artifact = store
        .read_obligation(turn.obligation)
        .expect("snapshot")
        .result_artifact
        .expect("a published result");

    let request = ResultArtifactMissingRequest {
        obligation: turn.obligation,
        artifact,
    };
    store
        .raise_result_artifact_missing(request)
        .expect("entering repair");
    store.verify_projections().expect("replay after the raise");
    store
        .resolve_result_artifact_missing(request)
        .expect("leaving repair");
    store
        .verify_projections()
        .expect("replay after the resolution");

    // Raise, resolve, raise again: the second condition is a *new* row, and the
    // fold has to reproduce both.
    store
        .raise_result_artifact_missing(request)
        .expect("entering repair again");
    assert_eq!(count(&harness.inspect(), "health_conditions"), 2);
    store
        .verify_projections()
        .expect("replay after the second raise");

    drop(store);
    let store = harness.open().expect("reopen");
    assert_eq!(
        store
            .open_health_conditions()
            .expect("reading conditions")
            .len(),
        1,
        "one open, one resolved"
    );
}
