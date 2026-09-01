//! Result-artifact acceptance tests: ART-001 … ART-005.
//!
//! `governor-artifacts` already proves the file half on its own — modes,
//! `O_NOFOLLOW`, digests, atomic name publication, one crash per publication
//! step. What is added here is the *composition*: the exhaustive crash matrix
//! across both halves at once, and the retention, integrity and path cases that
//! only exist once a SQLite obligation is pointing at the bytes.
//!
//! # Coverage
//!
//! | Test | `docs/testing.md` | Status |
//! | --- | --- | --- |
//! | [`art_001_artifact_durable_before_completed_obligation`] | ART-001 | exhaustive matrix covered here; the single-failpoint sweep is also in `governor-artifacts` `artifact_durability` |
//! | [`art_002_open_obligation_pins_retention`] | ART-002 | pin *derivation* in `governor-artifacts` `artifact_retention`; the real sweep at each stage, across a restart, covered here |
//! | [`art_003_artifact_tamper_fails_closed_for_the_foreman`] | ART-003 | byte-level detection in `governor-artifacts` `artifact_integrity`; the "MCP read fails and the obligation stays open" half covered here |
//! | [`art_004_a_tampered_storage_ref_never_becomes_a_path`] | ART-004 | traversal/symlink/hard-link matrix in `governor-artifacts` `artifact_paths`; the tampered-row path covered here |
//! | [`art_005_private_modes_survive_a_composed_lifecycle`] | ART-005 | umask matrix in `governor-artifacts` `artifact_permissions`; survival across publish/restart/sweep covered here |
//!
//! ART-006 … ART-011 are Phase 2: they need the worker-host, the managed-run
//! staging area and the hook inbox, none of which Phase 1 builds. Windows ACL
//! policy is a separate platform suite, as `docs/testing.md` ART-005 says.

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt as _;

use governor_artifacts::{
    ArtifactConfig, ArtifactError, ArtifactFailpoint, OrphanReason, RetentionDecision, StorageKey,
};
use governor_core::artifact::RetentionState;
use governor_core::obligation::{Disposition, ObligationState};
use governor_core::time::{DurationMs, Timestamp};
use governor_store_sqlite::Failpoint;
use governor_testkit::failpoints::{ArtifactCrash, StoreCrash};
use governor_testkit::harness::Harness;
use governor_testkit::keys::SeededKeys;
use governor_testkit::scenario::{
    ALREADY_LAPSED, ArtifactRow, FINAL_RESULT, LIVE_CLAIM, accepted_work, acknowledge,
    artifact_rows, assert_no_completion_without_durable_bytes, committed_keys, expire_claim,
    handed_over, handoff, mint_claim, open_turn, publish_bytes, publish_result, snapshot,
    start_worker,
};

/// The store failpoints the publication transaction actually passes through.
const PUBLICATION_POINTS: &[Option<Failpoint>] = &[
    None,
    Some(Failpoint::AfterEventAppend),
    Some(Failpoint::AfterProjectionUpdate),
    Some(Failpoint::BeforeCommit),
];

#[test]
fn art_001_artifact_durable_before_completed_obligation() {
    // Every artifact failpoint crossed with every store failpoint that can
    // follow it, plus the two "no crash in this half" rows. Thirty-two cells,
    // one forbidden outcome.
    let mut file_crashes: Vec<Option<ArtifactFailpoint>> = vec![None];
    file_crashes.extend(ArtifactFailpoint::ALL.iter().copied().map(Some));

    for file_point in file_crashes {
        for db_point in PUBLICATION_POINTS {
            let cell = format!("{file_point:?} then {db_point:?}");
            let harness = Harness::new();
            let store_crash = db_point.map(|point| StoreCrash::at("publish_worker_result", point));
            let store = harness
                .open_with(store_crash.as_ref().map(StoreCrash::boxed))
                .expect("opening");
            let artifact_crash = file_point.map(ArtifactCrash::at);
            let mut artifacts = harness.open_artifacts_with(
                ArtifactConfig::default(),
                Box::new(harness.keys()),
                artifact_crash.as_ref().map(ArtifactCrash::boxed),
            );

            let turn = open_turn(&store);
            start_worker(&store, turn.obligation, "run-1");
            let outcome = publish_result(
                &store,
                &mut artifacts,
                turn.obligation,
                "run-1",
                FINAL_RESULT,
            );

            // Whatever happened, the forbidden outcome must not exist — before
            // any recovery has had a chance to tidy up.
            assert_no_completion_without_durable_bytes(&harness, &cell);

            let committed = outcome.is_ok();
            assert_eq!(
                snapshot(&store, turn.obligation).state == ObligationState::CompletedUnprocessed,
                committed,
                "{cell}: the obligation completes exactly when the publication did"
            );
            drop(store);

            // And it must still not exist after a restart, whose replay
            // verification is the independent oracle.
            let store = harness.open().expect("reopen");
            store
                .verify_projections()
                .unwrap_or_else(|error| panic!("{cell}: replay after reopen: {error}"));
            assert_no_completion_without_durable_bytes(&harness, &format!("{cell}, after reopen"));
            assert_eq!(
                snapshot(&store, turn.obligation).state == ObligationState::CompletedUnprocessed,
                committed,
                "{cell}: the restart did not invent or lose a completion"
            );

            // The allowed residue: an unreferenced file. Never a referenced
            // missing one.
            let conn = harness.inspect();
            let referenced = committed_keys(&conn);
            let on_disk: BTreeSet<StorageKey> = harness
                .files_in("objects")
                .into_iter()
                .filter_map(|name| StorageKey::parse(&name).ok())
                .collect();
            assert!(
                referenced.is_subset(&on_disk),
                "{cell}: a committed row references bytes that are not there"
            );
        }
    }
}

#[test]
fn art_002_open_obligation_pins_retention() {
    // A sweep with no grace at all, so "the artifact survived" is a statement
    // about the pin and not about a timer that had not expired yet.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts_with(
        ArtifactConfig {
            orphan_grace: DurationMs::ZERO,
            retention_grace: DurationMs::ZERO,
            ..ArtifactConfig::default()
        },
        Box::new(harness.keys()),
        None,
    );
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    let key = work.artifact.key().clone();
    let far_future = Timestamp::from_unix_millis(i64::MAX);

    let sweep = |harness: &Harness, artifacts: &governor_artifacts::ArtifactStore, stage: &str| {
        let conn = harness.inspect();
        let inputs: Vec<_> = artifact_rows(&conn).iter().map(retention_input).collect();
        let report = artifacts
            .collect(&inputs, far_future)
            .expect("a sweep never fails on a pinned artifact");
        assert!(
            report.deleted.is_empty(),
            "{stage}: garbage collection deleted work in flight"
        );
        assert!(
            report
                .kept
                .iter()
                .all(|(_, decision)| *decision == RetentionDecision::Pinned),
            "{stage}: the sweep must report the pin, not a timer"
        );
        assert!(
            harness
                .files_in("objects")
                .iter()
                .any(|name| name == key.as_str()),
            "{stage}: the bytes are gone"
        );
    };

    // 1. Before ACK.
    sweep(&harness, &artifacts, "before ACK");

    // 2. During a claim, and after the handoff.
    let minted = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        ALREADY_LAPSED,
    )
    .expect("minting a claim");
    sweep(&harness, &artifacts, "during a claim");
    handoff(&store, work.obligation, minted.claim).expect("handing over");
    sweep(
        &harness,
        &artifacts,
        "after physical settlement and handoff",
    );

    // 3. After claim expiry.
    expire_claim(&store, work.obligation, minted.claim).expect("a lapsed claim expires");
    sweep(&harness, &artifacts, "after claim expiry");

    // 4. After a restart, because a pin that only exists in memory is not one.
    drop(store);
    let store = harness.open().expect("reopen");
    sweep(&harness, &artifacts, "after a restart");

    // Only a valid closing disposition, plus the recorded delay, releases it.
    let reclaim = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("reclaiming");
    handoff(&store, work.obligation, reclaim.claim).expect("handing over again");
    acknowledge(
        &store,
        work.obligation,
        work.generation,
        reclaim.claim,
        Disposition::Accepted,
    )
    .expect("a fully fenced ACK");

    let conn = harness.inspect();
    let rows = artifact_rows(&conn);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].retention(), RetentionState::Eligible);
    let deletable_at = rows[0]
        .deletable_at
        .expect("the ACK recorded when it may go");

    // Inside the recorded grace it is still kept; the sweep obeys the stamp
    // rather than its own configured delay.
    let inputs: Vec<_> = rows.iter().map(retention_input).collect();
    let early = artifacts
        .collect(
            &inputs,
            Timestamp::from_unix_millis(deletable_at.as_unix_millis() - 1),
        )
        .expect("a sweep inside the grace period");
    assert!(early.deleted.is_empty());

    let late = artifacts
        .collect(&inputs, deletable_at)
        .expect("a sweep at the recorded instant");
    assert_eq!(late.deleted, vec![key.clone()]);
    assert!(
        !harness
            .files_in("objects")
            .iter()
            .any(|n| n == key.as_str())
    );
}

/// The retention facts one committed row carries into a sweep.
fn retention_input(row: &ArtifactRow) -> governor_artifacts::RetentionInput {
    row.as_retention_input()
}

#[test]
fn art_003_artifact_tamper_fails_closed_for_the_foreman() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (work, claim) = handed_over(&store, &mut artifacts, "conv-A", LIVE_CLAIM);

    // Modify the bytes after the database has committed.
    let path = harness
        .artifact_root()
        .join("objects")
        .join(work.artifact.key().as_str());
    let mut tampered = FINAL_RESULT.to_vec();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    let mode = std::fs::metadata(&path)
        .expect("the artifact exists")
        .permissions();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("making the artifact writable for the tamper");
    std::fs::write(&path, &tampered).expect("tampering with the bytes");
    std::fs::set_permissions(&path, mode).expect("restoring the mode");

    // The read the claiming foreman would perform reports integrity failure,
    // and hands back no bytes at all.
    let error = artifacts
        .read_verified(
            work.artifact.key(),
            work.artifact.digest(),
            work.artifact.byte_len(),
        )
        .expect_err("a tampered artifact must never reach review");
    assert!(
        matches!(error, ArtifactError::Integrity { .. }),
        "{error:?}"
    );

    // The obligation is untouched: it is still owed, still claimed, still
    // pinning the artifact it cannot currently read.
    let current = snapshot(&store, work.obligation);
    assert_eq!(current.state, ObligationState::Processing);
    assert!(current.open, "ART-003: the obligation remains open");
    assert_eq!(current.claim, Some(claim));
    assert_eq!(
        artifact_rows(&harness.inspect())[0].retention(),
        RetentionState::Pinned
    );
}

#[test]
fn art_004_a_tampered_storage_ref_never_becomes_a_path() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    drop(store);

    // A row is not a trusted input: anyone who can edit the database can put a
    // path in it. Re-validating on the way out is what stops that becoming a
    // read outside the root.
    let writable = rusqlite::Connection::open(harness.database_path()).expect("write connection");
    for forged in ["../../etc/passwd", "/etc/passwd", ".hidden", "..", "a/b"] {
        writable
            .execute(
                "UPDATE result_artifacts SET storage_ref = ?1",
                rusqlite::params![forged],
            )
            .expect("planting a hostile storage ref");
        let rows = artifact_rows(&harness.inspect());
        assert!(
            rows[0].key().is_err(),
            "{forged} was accepted as a storage key"
        );
    }

    // Restored, the same row reads fine, so the refusals were about the value.
    writable
        .execute(
            "UPDATE result_artifacts SET storage_ref = ?1",
            rusqlite::params![work.artifact.key().as_str()],
        )
        .expect("restoring the real key");
    drop(writable);
    let rows = artifact_rows(&harness.inspect());
    let key = rows[0].key().expect("the real key validates");
    assert_eq!(
        artifacts
            .read_verified(&key, rows[0].digest(), rows[0].byte_len)
            .expect("and reads"),
        FINAL_RESULT
    );
}

#[test]
fn art_005_private_modes_survive_a_composed_lifecycle() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts_with(
        ArtifactConfig {
            orphan_grace: DurationMs::ZERO,
            ..ArtifactConfig::default()
        },
        Box::new(harness.keys()),
        None,
    );
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    let assert_private = |stage: &str| {
        for dir in ["objects", "incoming", "quarantine"] {
            let mode = std::fs::metadata(harness.artifact_root().join(dir))
                .expect("a layout directory")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700, "{stage}: {dir} is not owner-only");
        }
        for name in harness.files_in("objects") {
            let mode = std::fs::metadata(harness.artifact_root().join("objects").join(&name))
                .expect("a published artifact")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{stage}: objects/{name} is not owner-only");
        }
    };
    assert_private("after publication");

    // A restart re-opens and repairs the layout; it must not widen it.
    drop(store);
    let store = harness.open().expect("reopen");
    let artifacts = harness.open_artifacts_with(
        ArtifactConfig {
            orphan_grace: DurationMs::ZERO,
            ..ArtifactConfig::default()
        },
        Box::new(harness.keys()),
        None,
    );
    assert_private("after a restart");

    // An orphan sweep moves a file into quarantine, which must keep its mode.
    // A second key source publishes bytes the database never learns about,
    // which is exactly the residue a crash before the commit leaves.
    let mut orphan_writer = harness.open_artifacts_with(
        ArtifactConfig {
            orphan_grace: DurationMs::ZERO,
            ..ArtifactConfig::default()
        },
        Box::new(SeededKeys::new(0xFEED)),
        None,
    );
    publish_bytes(&mut orphan_writer, b"an unreferenced result").expect("an orphan file");
    let scan = artifacts
        .scan_orphans(
            &committed_keys(&harness.inspect()),
            Timestamp::from_unix_millis(i64::MAX),
        )
        .expect("an orphan sweep");
    assert_eq!(scan.quarantined.len(), 1);
    assert_eq!(scan.quarantined[0].reason, OrphanReason::UnreferencedObject);
    for name in harness.files_in("quarantine") {
        let mode = std::fs::metadata(harness.artifact_root().join("quarantine").join(&name))
            .expect("a quarantined file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "quarantine/{name} is not owner-only");
    }
    assert_private("after an orphan sweep");

    // The referenced artifact was never touched, and the obligation is intact.
    assert!(
        harness
            .files_in("objects")
            .iter()
            .any(|name| name == work.artifact.key().as_str()),
        "the referenced artifact was swept"
    );
    assert!(snapshot(&store, work.obligation).open);
}
