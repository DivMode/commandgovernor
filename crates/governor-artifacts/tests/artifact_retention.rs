//! ART-002 — an open obligation pins its artifact.
//!
//! `docs/testing.md` ART-002: *try GC before ACK, during claim, after physical
//! ChatGPT settlement, and after claim expiry. Artifact remains. Only after a
//! valid closing disposition and retention delay may GC delete it.*
//!
//! The four attempts are the four ways someone might mistake progress for
//! completion. None of them closes an obligation, so all four must leave the
//! bytes exactly where they are — and the sweep must reach that conclusion from
//! the durable authority's own `retention_state`, never from its own opinion
//! about what "looks finished".

mod support;

use governor_artifacts::{ArtifactStore, RetentionDecision, decide};
use governor_core::artifact::RetentionState;
use governor_core::obligation::Disposition;
use governor_core::time::{DurationMs, Timestamp};
use governor_store_sqlite::{AcknowledgeRequest, DeliverHandoffRequest, MintClaimRequest, Store};
use rusqlite::Connection;
use support::{
    ArtifactRow, FINAL_RESULT, Harness, SequentialKeys, accept_wake, artifact_rows, bind,
    config_with_grace, open_turn, publish_result, schedule_wake, start_worker,
};

/// The one row the scenario is about.
fn row(conn: &Connection) -> ArtifactRow {
    let rows = artifact_rows(conn);
    assert_eq!(rows.len(), 1, "the scenario publishes exactly one artifact");
    rows.into_iter().next().expect("one row")
}

/// Runs a sweep and asserts the bytes survived it.
fn sweep_keeps_the_bytes(
    harness: &Harness,
    artifacts: &ArtifactStore,
    stage: &str,
) -> RetentionDecision {
    let conn = harness.inspect();
    let row = row(&conn);
    // The furthest-future instant there is: nothing here may be kept merely
    // because a timer has not expired yet.
    let report = artifacts
        .collect(
            &[row.as_retention_input()],
            Timestamp::from_unix_millis(i64::MAX),
        )
        .expect("sweeping");
    assert!(
        report.deleted.is_empty(),
        "{stage}: garbage collection must not delete a pinned artifact"
    );
    assert_eq!(
        harness.files_in("objects").len(),
        1,
        "{stage}: the bytes must still be there"
    );
    report
        .kept
        .first()
        .map(|(_, decision)| *decision)
        .expect("one decision")
}

#[test]
fn no_amount_of_progress_short_of_a_closing_disposition_releases_the_artifact() {
    let harness = Harness::new();
    let store = harness.open_store().expect("opening the store");
    // A retention grace of zero: if anything below survives collection it is
    // because it was *pinned*, not because a timer had not expired yet.
    let artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let mut publisher = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );

    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(
        &store,
        &mut publisher,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("a clean publication");

    // 1. Before ACK: the obligation is `completed_unprocessed`, which is open.
    assert_eq!(
        sweep_keeps_the_bytes(&harness, &artifacts, "before ACK"),
        RetentionDecision::Pinned
    );

    // 2. After physical ChatGPT settlement. Accepted is not settled is not ACK
    //    (`docs/state-machines.md` invariant 14), and none of them closes work.
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    );
    accept_wake(&store, &claimed, generation, "msg-1");
    assert_eq!(
        sweep_keeps_the_bytes(&harness, &artifacts, "after settlement"),
        RetentionDecision::Pinned
    );

    // 3. During a foreman claim, with a lifetime short enough to lapse.
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claim = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: turn.obligation,
            presented_delivery_id: claimed.delivery_id.clone(),
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime: DurationMs::from_millis(5),
        })
        .expect("minting a claim");
    assert_eq!(
        sweep_keeps_the_bytes(&harness, &artifacts, "during claim"),
        RetentionDecision::Pinned
    );

    // 4. After claim expiry. A lapsed claim returns attention; it never closes
    //    work, so the artifact is still required.
    drop(store);
    let store = harness
        .open_store_with(claim.expires_at.as_unix_millis() + 10_000, None)
        .expect("reopening the store much later");
    assert!(
        store
            .read_obligation(turn.obligation)
            .expect("snapshot")
            .open,
        "an expired claim must not close an obligation"
    );
    assert_eq!(
        sweep_keeps_the_bytes(&harness, &artifacts, "after claim expiry"),
        RetentionDecision::Pinned
    );

    // And the durable authority agrees: still pinned, after all four.
    let conn = harness.inspect();
    assert_eq!(row(&conn).retention(), RetentionState::Pinned);
    assert_eq!(row(&conn).released_at, None);
}

#[test]
fn acknowledgement_makes_the_artifact_eligible_but_the_delay_still_holds_it() {
    let harness = Harness::new();
    let store = harness.open_store().expect("opening the store");
    let grace: i64 = 10_000;
    let grace_ms = u64::try_from(grace).expect("positive");
    let artifacts = harness.open_artifacts_with(
        config_with_grace(0, grace_ms),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let mut publisher = harness.open_artifacts_with(
        config_with_grace(0, grace_ms),
        Box::new(SequentialKeys::new(0)),
        None,
    );

    let released_at = acknowledge_one_result(&harness, &store, &mut publisher);
    let conn = harness.inspect();
    let input = row(&conn).as_retention_input();
    assert_eq!(
        input.state,
        RetentionState::Eligible,
        "a valid closing disposition releases the pin"
    );
    assert_eq!(input.released_at, Some(released_at));

    // Inside the delay: eligible, and still kept. ACK is not deletion.
    let report = artifacts
        .collect(
            std::slice::from_ref(&input),
            Timestamp::from_unix_millis(released_at.as_unix_millis() + grace - 1),
        )
        .expect("sweeping");
    assert!(report.deleted.is_empty());
    assert_eq!(
        report.kept[0].1,
        RetentionDecision::WithinGrace {
            deletable_at: Timestamp::from_unix_millis(released_at.as_unix_millis() + grace)
        }
    );
    assert_eq!(harness.files_in("objects").len(), 1);

    // Past the delay: deleted, and only then.
    let report = artifacts
        .collect(
            std::slice::from_ref(&input),
            Timestamp::from_unix_millis(released_at.as_unix_millis() + grace),
        )
        .expect("sweeping");
    assert_eq!(report.deleted.len(), 1);
    assert!(harness.files_in("objects").is_empty());

    // And the sweep is idempotent: a second pass over a row whose bytes are
    // already gone is not an error.
    artifacts
        .collect(&[input], Timestamp::from_unix_millis(i64::MAX))
        .expect("a second sweep");
}

#[test]
fn an_eligible_artifact_with_no_recorded_release_instant_is_never_deleted() {
    // `result_artifacts.eligible_for_delete_at_ms` exists in the schema but no
    // store operation writes it, so a caller that cannot supply the closing
    // instant must not get a deletion by default.
    let harness = Harness::new();
    let store = harness.open_store().expect("opening the store");
    let artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let mut publisher = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    acknowledge_one_result(&harness, &store, &mut publisher);

    let conn = harness.inspect();
    let mut input = row(&conn).as_retention_input();
    input.released_at = None;
    assert_eq!(
        decide(
            &input,
            Timestamp::from_unix_millis(i64::MAX),
            DurationMs::ZERO
        ),
        RetentionDecision::ReleaseInstantUnknown
    );
    let report = artifacts
        .collect(&[input], Timestamp::from_unix_millis(i64::MAX))
        .expect("sweeping");
    assert!(report.deleted.is_empty());
    assert_eq!(harness.files_in("objects").len(), 1);
}

/// Drives one result all the way to a valid ACK, returning the closing instant.
fn acknowledge_one_result(
    harness: &Harness,
    store: &Store,
    publisher: &mut ArtifactStore,
) -> Timestamp {
    let turn = open_turn(store);
    let generation = bind(store, "conv-A");
    start_worker(store, turn.obligation, "run-1");
    publish_result(store, publisher, turn.obligation, "run-1", FINAL_RESULT)
        .expect("a clean publication");

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    );
    accept_wake(store, &claimed, generation, "msg-1");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claim = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: turn.obligation,
            presented_delivery_id: claimed.delivery_id.clone(),
            binding_generation: generation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            lifetime: DurationMs::from_millis(60_000),
        })
        .expect("minting a claim");
    store
        .deliver_handoff(DeliverHandoffRequest {
            obligation: turn.obligation,
            claim: claim.claim,
        })
        .expect("delivering the handoff");

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation: turn.obligation,
            expected_version: snapshot.version,
            expected_source: snapshot.source.clone(),
            binding_generation: generation,
            claim: snapshot.claim.expect("a current claim"),
            disposition: Disposition::Accepted,
        })
        .expect("acknowledging");

    let conn = harness.inspect();
    row(&conn)
        .released_at
        .expect("the closing event carries an instant")
}
