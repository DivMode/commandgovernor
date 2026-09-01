//! ART-003 — a digest or length mismatch fails closed.
//!
//! `docs/testing.md` ART-003: *modify artifact bytes after DB commit. MCP read
//! reports integrity failure and the obligation remains open.*
//!
//! Two things are being proven, and the second is the one that is easy to get
//! wrong. The read must **fail**, and the read must return **no bytes**: a
//! caller that receives a truncated result along with a warning will use the
//! truncated result. The API shape is what enforces it — the bytes and the
//! error are not both representable — and the assertions below check the
//! consequences: work stays open, and the artifact stays pinned.

mod support;

use governor_artifacts::ArtifactError;
use governor_core::artifact::{ArtifactIntegrityError, RetentionState};
use support::{
    FINAL_RESULT, Harness, SequentialKeys, artifact_rows, committed_keys, config_with_grace,
    open_turn, publish_bytes, publish_result, start_worker,
};

/// Publishes one result and returns the harness plus the committed key.
fn published() -> (Harness, String) {
    let harness = Harness::new();
    let store = harness.open_store().expect("opening the store");
    let mut artifacts = harness.open_artifacts();
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
    let key = published.key().to_string();
    drop(store);
    (harness, key)
}

/// Overwrites the stored bytes behind the store's back.
fn tamper(harness: &Harness, key: &str, bytes: &[u8]) {
    let path = harness.artifact_root().join("objects").join(key);
    std::fs::write(path, bytes).expect("tampering");
}

#[test]
fn substituted_bytes_of_the_same_length_are_a_digest_failure() {
    let (harness, key) = published();
    let mut forged = FINAL_RESULT.to_vec();
    let last = forged.len() - 1;
    forged[last] ^= 0xFF;
    tamper(&harness, &key, &forged);

    let conn = harness.inspect();
    let row = artifact_rows(&conn).remove(0);
    let artifacts = harness.open_artifacts();
    let error = artifacts
        .read(&row.as_metadata())
        .expect_err("a tampered artifact must not be readable");
    assert!(
        matches!(
            error,
            ArtifactError::Integrity {
                source: ArtifactIntegrityError::DigestMismatch,
                ..
            }
        ),
        "expected a digest mismatch, got {error}"
    );
    assert!(error.is_artifact_unusable());

    // The obligation is untouched, and the artifact is still pinned by it. A
    // corrupt result is not a reason to close work; it is a reason to raise
    // attention.
    let states = support::obligation_states(&conn);
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].1, "completed_unprocessed");
    assert_eq!(row.retention(), RetentionState::Pinned);
}

#[test]
fn a_truncated_artifact_is_a_length_failure_not_a_short_read() {
    let (harness, key) = published();
    tamper(&harness, &key, &FINAL_RESULT[..10]);

    let conn = harness.inspect();
    let row = artifact_rows(&conn).remove(0);
    let artifacts = harness.open_artifacts();
    let error = artifacts
        .read(&row.as_metadata())
        .expect_err("a truncated artifact must not be readable");
    match error {
        ArtifactError::Integrity {
            source: ArtifactIntegrityError::LengthMismatch { expected, observed },
            ..
        } => {
            assert_eq!(expected, row.byte_len);
            assert_eq!(observed, 10);
        }
        other => panic!("expected a length mismatch, got {other}"),
    }
    assert_eq!(
        support::obligation_states(&conn)[0].1,
        "completed_unprocessed"
    );
}

#[test]
fn an_extended_artifact_is_refused_without_reading_it_all() {
    let (harness, key) = published();
    let mut grown = FINAL_RESULT.to_vec();
    grown.extend(std::iter::repeat_n(b'x', 4_096));
    tamper(&harness, &key, &grown);

    let conn = harness.inspect();
    let row = artifact_rows(&conn).remove(0);
    let artifacts = harness.open_artifacts();
    match artifacts.read(&row.as_metadata()) {
        Err(ArtifactError::Integrity {
            source: ArtifactIntegrityError::LengthMismatch { expected, observed },
            ..
        }) => {
            assert_eq!(expected, row.byte_len);
            // One byte past the recorded length is all that is read: enough to
            // know the file grew, never enough for a tampered row to drive the
            // allocation.
            assert_eq!(observed, row.byte_len + 1);
        }
        other => panic!("expected a length mismatch, got {other:?}"),
    }
}

#[test]
fn a_deleted_artifact_is_missing_rather_than_empty() {
    let (harness, key) = published();
    std::fs::remove_file(harness.artifact_root().join("objects").join(&key)).expect("removing");

    let conn = harness.inspect();
    let row = artifact_rows(&conn).remove(0);
    let artifacts = harness.open_artifacts();
    let error = artifacts
        .read(&row.as_metadata())
        .expect_err("a missing artifact must not read as empty");
    assert!(
        matches!(error, ArtifactError::Missing { .. }),
        "expected a missing artifact, got {error}"
    );
    assert!(error.is_artifact_unusable());
    assert_eq!(
        support::obligation_states(&conn)[0].1,
        "completed_unprocessed"
    );

    // A missing artifact is still referenced, so the sweep must not treat the
    // *other* files in the root as orphans on its account.
    let scan = artifacts
        .scan_orphans(
            &committed_keys(&conn),
            governor_core::time::Timestamp::from_unix_millis(i64::MAX),
        )
        .expect("sweeping");
    assert!(scan.quarantined.is_empty());
}

#[test]
fn a_row_claiming_more_bytes_than_the_root_would_ever_store_is_refused() {
    // A tampered `byte_len` must not become an allocation instruction.
    let (harness, key) = published();
    let artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let error = artifacts
        .read_verified(
            &governor_artifacts::StorageKey::parse(&key).expect("valid key"),
            governor_core::artifact::ArtifactDigest::from_bytes([0u8; 32]),
            u64::MAX,
        )
        .expect_err("an absurd recorded length must be refused");
    assert!(
        matches!(error, ArtifactError::TooLarge { .. }),
        "expected a bound refusal, got {error}"
    );
}

#[test]
fn a_second_write_to_a_published_key_is_refused() {
    // Immutability: there is no overwrite path. `link(2)` fails with `EEXIST`
    // rather than replacing the bytes the way `rename(2)` would.
    let harness = Harness::new();
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(support::FixedKey::new("ra-fixed")),
        None,
    );
    let first = publish_bytes(&mut artifacts, FINAL_RESULT).expect("the first publication");
    assert_eq!(first.key().to_string(), "ra-fixed");

    let error =
        publish_bytes(&mut artifacts, b"different bytes").expect_err("a key is published once");
    assert!(
        matches!(error, ArtifactError::AlreadyPublished { ref key } if key == "ra-fixed"),
        "expected an immutability refusal, got {error}"
    );

    // And the original bytes are exactly as they were.
    let bytes = artifacts
        .read_verified(first.key(), first.digest(), first.byte_len())
        .expect("the first artifact is untouched");
    assert_eq!(bytes, FINAL_RESULT);
}

#[test]
fn a_result_larger_than_the_bound_is_refused_before_any_file_exists() {
    let harness = Harness::new();
    let mut artifacts = harness.open_artifacts_with(
        governor_artifacts::ArtifactConfig {
            max_bytes: 64,
            ..governor_artifacts::ArtifactConfig::default()
        },
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let error = publish_bytes(&mut artifacts, &[b'x'; 65]).expect_err("the bound must hold");
    match error {
        ArtifactError::TooLarge { limit, actual } => {
            assert_eq!(limit, 64);
            assert_eq!(actual, 65);
        }
        other => panic!("expected a bound refusal, got {other}"),
    }
    assert!(
        harness.files_in("objects").is_empty() && harness.files_in("incoming").is_empty(),
        "an oversized result must never become a partial artifact"
    );

    // Exactly at the bound is fine: the limit is inclusive.
    publish_bytes(&mut artifacts, &[b'x'; 64]).expect("the bound is inclusive");
}

/// Rewrites a file the moment publication reaches a named point.
struct TamperAt {
    point: governor_artifacts::ArtifactFailpoint,
    path: std::path::PathBuf,
    bytes: &'static [u8],
}

impl governor_artifacts::ArtifactFailpointHook for TamperAt {
    fn reached(
        &self,
        _op: &'static str,
        point: governor_artifacts::ArtifactFailpoint,
    ) -> governor_artifacts::ArtifactResult<()> {
        if point == self.point {
            std::fs::write(&self.path, self.bytes).expect("tampering");
        }
        Ok(())
    }
}

#[test]
fn bytes_altered_after_the_directory_sync_deny_the_proof() {
    // The publication post-condition. The digest is computed from the bytes
    // handed in, but the proof rests on the bytes that are *there*: a short
    // write, a device error `fsync` did not surface, or a same-user rewrite in
    // the window before the transaction must not become a completed
    // obligation.
    let harness = Harness::new();
    let root = harness.artifact_root();
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(support::FixedKey::new("ra-fixed")),
        Some(Box::new(TamperAt {
            point: governor_artifacts::ArtifactFailpoint::AfterDirSync,
            path: root.join("objects").join("ra-fixed"),
            bytes: b"substituted after the barrier",
        })),
    );

    let error = publish_bytes(&mut artifacts, FINAL_RESULT)
        .expect_err("the post-condition must deny the proof");
    assert!(
        matches!(error, ArtifactError::Integrity { .. }),
        "expected an integrity refusal, got {error}"
    );

    // No proof means no transaction, and the substituted file is an orphan.
    assert_eq!(harness.files_in("objects"), vec!["ra-fixed".to_owned()]);
    let scan = artifacts
        .scan_orphans(
            &std::collections::BTreeSet::new(),
            governor_core::time::Timestamp::from_unix_millis(i64::MAX),
        )
        .expect("sweeping");
    assert_eq!(scan.quarantined.len(), 1);
}
