//! ART-001 — the result artifact is durable before the obligation opens.
//!
//! `docs/testing.md` ART-001: *inject a crash at each candidate-validation /
//! file write / fsync / rename / directory-sync / DB point. Forbidden: a
//! committed `completed_unprocessed` references a missing or non-durable result
//! artifact. Allowed: an unreferenced orphan file later quarantined/GCed.*
//!
//! The matrix here is both halves of the publication:
//!
//! - every [`ArtifactFailpoint`], which aborts before the store is ever
//!   called;
//! - the store's own [`Failpoint`]s inside `publish_worker_result`, which abort
//!   after the bytes are already durable.
//!
//! Every cell ends at
//! [`assert_no_completion_without_durable_bytes`](support::assert_no_completion_without_durable_bytes),
//! so the forbidden outcome is checked the same way regardless of where the
//! crash landed.

mod support;

use std::collections::BTreeSet;

use governor_artifacts::{ArtifactError, ArtifactFailpoint, OrphanReason};
use governor_core::artifact::RetentionState;
use governor_core::time::Timestamp;
use governor_store_sqlite::Failpoint;
use support::{
    ArtifactFireOnce, FINAL_RESULT, Harness, PublicationFailure, SequentialKeys, StoreFireOnce,
    artifact_rows, assert_no_completion_without_durable_bytes, committed_keys,
    completed_unprocessed_refs, config_with_grace, obligation_states, open_turn, publish_result,
    start_worker,
};

#[test]
fn a_clean_publication_makes_the_bytes_durable_and_then_opens_the_obligation() {
    let harness = Harness::new();
    let store = harness.open_store().expect("opening the store");
    let mut artifacts = harness.open_artifacts();
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");

    let (published, committed) = publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("a clean publication");

    // The bytes exist under the allocated key, and only there.
    assert_eq!(
        harness.files_in("objects"),
        vec![published.key().to_string()]
    );
    assert!(
        harness.files_in("incoming").is_empty(),
        "the staging name must be gone once the immutable name exists"
    );

    // The database references exactly those bytes, and the obligation is open.
    let conn = harness.inspect();
    assert_eq!(
        completed_unprocessed_refs(&conn),
        vec![published.key().to_string()]
    );
    let rows = artifact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].byte_len, published.byte_len());
    assert_eq!(rows[0].retention(), RetentionState::Pinned);
    assert_eq!(committed.artifact.to_string(), rows[0].artifact_id);

    // And they read back, verified against the committed metadata.
    let bytes = artifacts
        .read(&rows[0].as_metadata())
        .expect("a clean read");
    assert_eq!(bytes, FINAL_RESULT);
}

#[test]
fn a_crash_at_any_artifact_failpoint_leaves_no_completion_only_an_orphan() {
    for point in ArtifactFailpoint::ALL {
        let harness = Harness::new();
        let store = harness.open_store().expect("opening the store");
        let mut artifacts = harness.open_artifacts_with(
            config_with_grace(0, 0),
            Box::new(SequentialKeys::new(0)),
            Some(Box::new(ArtifactFireOnce::new(*point))),
        );
        let turn = open_turn(&store);
        start_worker(&store, turn.obligation, "run-1");

        let failure = publish_result(
            &store,
            &mut artifacts,
            turn.obligation,
            "run-1",
            FINAL_RESULT,
        )
        .expect_err("the injected crash must abort the publication");
        match failure {
            PublicationFailure::Artifact(ArtifactError::Injected { point: fired, .. }) => {
                assert_eq!(fired, *point);
            }
            other => panic!("{point}: expected an injected artifact failure, got {other:?}"),
        }

        // No proof was handed over, so the transaction never ran: no artifact
        // row, no completion, and the obligation is still the worker's.
        let conn = harness.inspect();
        assert!(
            artifact_rows(&conn).is_empty(),
            "{point}: an aborted publication must commit no artifact metadata"
        );
        assert!(
            completed_unprocessed_refs(&conn).is_empty(),
            "{point}: an aborted publication must open no completion"
        );
        assert_eq!(
            obligation_states(&conn)
                .into_iter()
                .map(|(_, state)| state)
                .collect::<Vec<_>>(),
            vec!["running".to_owned()],
            "{point}: the obligation must stay where the worker left it"
        );
        assert_no_completion_without_durable_bytes(&harness, &format!("artifact/{point}"));

        // The residue is exactly what the ordering predicts, which is what
        // proves the ordering.
        let objects = harness.files_in("objects");
        let incoming = harness.files_in("incoming");
        let staged_but_unpublished = matches!(
            point,
            ArtifactFailpoint::AfterTempCreate
                | ArtifactFailpoint::AfterWrite
                | ArtifactFailpoint::AfterFileSync
        );
        let published_but_uncommitted = matches!(
            point,
            ArtifactFailpoint::AfterPublishRename
                | ArtifactFailpoint::AfterDirSync
                | ArtifactFailpoint::BeforeProofHandoff
        );
        if *point == ArtifactFailpoint::BeforeTempCreate {
            assert!(
                objects.is_empty() && incoming.is_empty(),
                "{point}: nothing may exist before the staging file is created"
            );
        }
        if staged_but_unpublished {
            assert_eq!(
                objects.len(),
                0,
                "{point}: the immutable name must not exist"
            );
            assert_eq!(
                incoming.len(),
                1,
                "{point}: one staging file is the residue"
            );
        }
        if published_but_uncommitted {
            assert_eq!(objects.len(), 1, "{point}: the immutable name exists");
            assert!(
                incoming.is_empty(),
                "{point}: the staging name is gone once the immutable name exists"
            );
        }
        assert_eq!(
            point.may_leave_bytes(),
            !objects.is_empty() || !incoming.is_empty(),
            "{point}: may_leave_bytes must describe what actually happened"
        );

        // Whatever is left is an orphan, and the sweep sets it aside rather
        // than deleting it.
        let scan = artifacts
            .scan_orphans(
                &committed_keys(&conn),
                Timestamp::from_unix_millis(i64::MAX),
            )
            .expect("sweeping");
        assert_eq!(
            scan.quarantined.len(),
            objects.len() + incoming.len(),
            "{point}: every unreferenced file must be quarantined"
        );
        assert!(
            harness.files_in("objects").is_empty() && harness.files_in("incoming").is_empty(),
            "{point}: the sweep must clear the working directories"
        );
        assert_eq!(
            harness.files_in("quarantine").len(),
            objects.len() + incoming.len(),
            "{point}: quarantine keeps the evidence"
        );
    }
}

#[test]
fn a_crash_inside_the_publication_transaction_leaves_durable_bytes_unreferenced() {
    // The store half of the ART-001 matrix. The artifact layer has already
    // finished — the bytes and the name are durable — and the transaction that
    // would have referenced them rolls back.
    for point in [
        Failpoint::AfterEventAppend,
        Failpoint::AfterProjectionUpdate,
        Failpoint::BeforeCommit,
    ] {
        let harness = Harness::new();
        let store = harness
            .open_store_with(
                support::DEFAULT_CLOCK_START,
                Some(Box::new(StoreFireOnce::new("publish_worker_result", point))),
            )
            .expect("opening the store");
        let mut artifacts = harness.open_artifacts_with(
            config_with_grace(0, 0),
            Box::new(SequentialKeys::new(0)),
            None,
        );
        let turn = open_turn(&store);
        start_worker(&store, turn.obligation, "run-1");

        let failure = publish_result(
            &store,
            &mut artifacts,
            turn.obligation,
            "run-1",
            FINAL_RESULT,
        )
        .expect_err("the injected crash must abort the transaction");
        assert!(
            matches!(failure, PublicationFailure::Store(_)),
            "{point:?}: the artifact half must have succeeded"
        );

        let conn = harness.inspect();
        assert!(
            artifact_rows(&conn).is_empty(),
            "{point:?}: the whole transaction rolls back"
        );
        assert!(completed_unprocessed_refs(&conn).is_empty());
        assert_no_completion_without_durable_bytes(&harness, &format!("store/{point:?}"));

        // Exactly the documented residue: a durable, unreferenced file.
        assert_eq!(
            harness.files_in("objects").len(),
            1,
            "{point:?}: the bytes were made durable before the transaction ran"
        );
        let scan = artifacts
            .scan_orphans(&BTreeSet::new(), Timestamp::from_unix_millis(i64::MAX))
            .expect("sweeping");
        assert_eq!(scan.quarantined.len(), 1);
        assert_eq!(scan.quarantined[0].reason, OrphanReason::UnreferencedObject);
    }
}

#[test]
fn an_unreferenced_file_inside_the_grace_period_is_left_alone() {
    // A publication that is merely slow looks exactly like a crashed one. The
    // grace period is what stops the sweep racing a real result, so it is
    // checked with the same fixture that produces a genuine orphan.
    let harness = Harness::new();
    let store = harness
        .open_store_with(
            support::DEFAULT_CLOCK_START,
            Some(Box::new(StoreFireOnce::new(
                "publish_worker_result",
                Failpoint::BeforeCommit,
            ))),
        )
        .expect("opening the store");
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(60_000, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    let _ = publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect_err("the injected crash must abort the transaction");

    // `now` is the epoch, so nothing on disk can be older than the grace.
    let scan = artifacts
        .scan_orphans(&BTreeSet::new(), Timestamp::from_unix_millis(0))
        .expect("sweeping");
    assert!(scan.quarantined.is_empty(), "nothing may be set aside yet");
    assert_eq!(scan.within_grace.len(), 1);
    assert_eq!(harness.files_in("objects").len(), 1);
}

#[test]
fn a_referenced_artifact_is_never_swept() {
    let harness = Harness::new();
    let store = harness.open_store().expect("opening the store");
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    let (published, _) = publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("a clean publication");

    let conn = harness.inspect();
    let scan = artifacts
        .scan_orphans(
            &committed_keys(&conn),
            Timestamp::from_unix_millis(i64::MAX),
        )
        .expect("sweeping");
    assert_eq!(scan.referenced, 1);
    assert!(scan.quarantined.is_empty());
    assert_eq!(
        harness.files_in("objects"),
        vec![published.key().to_string()]
    );
}
