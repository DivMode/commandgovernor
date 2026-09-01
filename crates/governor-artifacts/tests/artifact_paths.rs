//! ART-004 and SEC-008 — nothing escapes the daemon-owned root.
//!
//! `docs/testing.md` ART-004: *attempt traversal, absolute paths, symlinks,
//! unsafe parents, and relevant hard-link edge cases. No read/write escapes the
//! daemon-owned root under the platform's supported rooted/no-follow policy.*
//!
//! Two layers do the work, and the tests are split along them.
//!
//! 1. **A key cannot express an escape.** `storage_ref` is a
//!    [`StorageKey`](governor_artifacts::StorageKey): a validated single
//!    component. Traversal, an absolute path and a separator are refused before
//!    any file is opened, so the attempts below fail at the type rather than at
//!    the syscall.
//! 2. **A planted name cannot redirect an open.** Every open is `O_NOFOLLOW`
//!    and every create is `O_EXCL`, so a symlink at a key fails with `ELOOP`
//!    and a name that already exists fails with `EEXIST`.
//!
//! Honest limit: `SECURITY.md` "Local trust model". Planting a symlink inside a
//! `0700` root requires the daemon's own OS user, which is outside the V1
//! boundary in the first place. What these tests prove is that such an attempt
//! **fails closed and is visible**, not that a same-user process is contained.

mod support;

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

use governor_artifacts::{ArtifactError, ArtifactRoot, StorageKey, UnsafePathReason};
use governor_core::artifact::{ArtifactDigest, ResultArtifact};
use support::{FINAL_RESULT, Harness, SequentialKeys, config_with_grace, publish_bytes, token};

const OUTSIDE_SECRET: &[u8] = b"this file is outside the artifact root and must never be read\n";

/// Metadata naming a `storage_ref` the daemon never allocated.
fn metadata_for(storage_ref: &str, len: u64) -> ResultArtifact {
    ResultArtifact::new(
        support::id(0),
        token(storage_ref),
        ArtifactDigest::from_bytes([0u8; 32]),
        len,
        governor_core::time::Timestamp::from_unix_millis(0),
    )
}

#[test]
fn a_storage_ref_that_is_a_path_never_becomes_one() {
    let harness = Harness::new();
    let artifacts = harness.open_artifacts();
    let outside = harness.state_root().join("outside.bin");
    std::fs::write(&outside, OUTSIDE_SECRET).expect("planting a file outside the root");

    // A tampered `result_artifacts.storage_ref` is the attack: the row is the
    // only thing that decides which file a read opens. Two layers refuse it,
    // and they refuse different shapes.
    //
    // Layer one: anything containing a separator is not even a `SafeToken`, so
    // it cannot be put in the row in the first place and cannot be handed to
    // this crate at all.
    for attempt in [
        "../outside.bin",
        "../../etc/passwd",
        "/etc/passwd",
        "objects/../../outside.bin",
        "sub/dir",
    ] {
        assert!(
            governor_core::fence::SafeToken::new(attempt).is_err(),
            "{attempt:?} must be unrepresentable as a storage_ref"
        );
    }

    // Layer two: token-shaped values that would still name something other
    // than a child of `objects/`.
    for attempt in ["..", ".", ".hidden"] {
        let error = artifacts
            .read(&metadata_for(attempt, 1))
            .expect_err("a dot-shaped storage_ref must be refused");
        assert!(
            matches!(error, ArtifactError::InvalidKey(_)),
            "{attempt:?}: expected a key refusal, got {error}"
        );
    }

    // Nothing outside was touched.
    assert_eq!(
        std::fs::read(&outside).expect("the outside file"),
        OUTSIDE_SECRET
    );
}

#[test]
fn a_symlink_planted_at_a_key_fails_closed_instead_of_redirecting_the_read() {
    let harness = Harness::new();
    let artifacts = harness.open_artifacts();
    let outside = harness.state_root().join("outside.bin");
    std::fs::write(&outside, OUTSIDE_SECRET).expect("planting a file outside the root");
    std::os::unix::fs::symlink(
        &outside,
        harness.artifact_root().join("objects").join("ra-link"),
    )
    .expect("planting a symlink at a key");

    let len = u64::try_from(OUTSIDE_SECRET.len()).expect("length fits");
    let error = artifacts
        .read(&metadata_for("ra-link", len))
        .expect_err("an O_NOFOLLOW open must refuse a symlink");
    assert!(
        matches!(
            error,
            ArtifactError::UnsafePath {
                reason: UnsafePathReason::Symlink,
                ..
            }
        ),
        "expected a symlink refusal, got {error}"
    );
    assert!(error.is_artifact_unusable());
}

#[test]
fn a_symlink_planted_where_a_staging_name_would_go_refuses_the_write() {
    let harness = Harness::new();
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let outside = harness.state_root().join("outside.bin");
    std::fs::write(&outside, OUTSIDE_SECRET).expect("planting a file outside the root");
    // The first key the deterministic source hands out, and the staging name
    // publication would compose from it.
    std::os::unix::fs::symlink(
        &outside,
        harness
            .artifact_root()
            .join("incoming")
            .join("ra-00000001.1.staging"),
    )
    .expect("planting a symlink at the staging name");
    let error = publish_bytes(&mut artifacts, FINAL_RESULT)
        .expect_err("an O_EXCL create must refuse an existing name");
    assert!(
        matches!(error, ArtifactError::Io { .. }),
        "expected a create refusal, got {error}"
    );
    assert_eq!(
        std::fs::read(&outside).expect("the outside file"),
        OUTSIDE_SECRET,
        "the write must not have gone through the link"
    );
    assert!(harness.files_in("objects").is_empty());
}

#[test]
fn a_key_already_taken_by_a_hard_link_refuses_publication() {
    // The hard-link edge case on the write side: a name planted at a key is a
    // name, whatever inode is behind it, and `link(2)` refuses it.
    let harness = Harness::new();
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let outside = harness.state_root().join("outside.bin");
    std::fs::write(&outside, OUTSIDE_SECRET).expect("planting a file outside the root");
    std::fs::hard_link(
        &outside,
        harness.artifact_root().join("objects").join("ra-00000001"),
    )
    .expect("planting a hard link at a key");
    let error =
        publish_bytes(&mut artifacts, FINAL_RESULT).expect_err("a taken key must be refused");
    assert!(
        matches!(error, ArtifactError::AlreadyPublished { ref key } if key == "ra-00000001"),
        "expected an immutability refusal, got {error}"
    );
    assert_eq!(
        std::fs::read(&outside).expect("the outside file"),
        OUTSIDE_SECRET,
        "the aliased inode must be untouched"
    );
}

#[test]
fn a_hard_link_alias_on_a_published_artifact_is_detected_on_read() {
    // The hard-link edge case on the read side. A second name for the inode
    // means somebody else can rewrite the bytes in place, which no mode bit
    // prevents once the attacker is the same OS user. The digest is the real
    // defence; the link count makes the tampering visible before the read even
    // starts.
    let harness = Harness::new();
    let mut artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let published = publish_bytes(&mut artifacts, FINAL_RESULT).expect("publishing");
    let stored = harness
        .artifact_root()
        .join("objects")
        .join(published.key().to_string());
    assert_eq!(
        std::fs::metadata(&stored).expect("stored metadata").nlink(),
        1,
        "publication must leave exactly one link"
    );

    std::fs::hard_link(&stored, harness.state_root().join("alias.bin")).expect("aliasing");
    let error = artifacts
        .read_verified(published.key(), published.digest(), published.byte_len())
        .expect_err("an aliased inode must be refused");
    assert!(
        matches!(
            error,
            ArtifactError::UnsafePath {
                reason: UnsafePathReason::HardLinked,
                ..
            }
        ),
        "expected a hard-link refusal, got {error}"
    );
}

#[test]
fn a_directory_planted_at_a_key_is_not_a_result() {
    let harness = Harness::new();
    let artifacts = harness.open_artifacts();
    std::fs::create_dir(harness.artifact_root().join("objects").join("ra-dir"))
        .expect("planting a directory at a key");
    let error = artifacts
        .read(&metadata_for("ra-dir", 0))
        .expect_err("a directory is not a result");
    assert!(
        matches!(
            error,
            ArtifactError::UnsafePath {
                reason: UnsafePathReason::NotRegularFile,
                ..
            }
        ) || matches!(error, ArtifactError::Io { .. }),
        "expected a non-regular-file refusal, got {error}"
    );
}

#[test]
fn a_symlinked_layout_component_refuses_to_open_the_root() {
    let harness = Harness::new();
    let elsewhere = harness.state_root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("creating a directory outside the root");
    let root = harness.state_root().join("linked-artifacts");
    std::fs::create_dir(&root).expect("creating a root");
    std::os::unix::fs::symlink(&elsewhere, root.join("objects"))
        .expect("planting a symlinked layout component");

    let error = ArtifactRoot::open(&root).expect_err("a symlinked component must be refused");
    assert!(
        matches!(
            error,
            ArtifactError::UnsafePath {
                reason: UnsafePathReason::Symlink,
                ..
            }
        ),
        "expected a symlink refusal, got {error}"
    );
}

#[test]
fn a_root_that_is_itself_a_symlink_is_refused() {
    let harness = Harness::new();
    let elsewhere = harness.state_root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("creating a directory outside the root");
    let root = harness.state_root().join("linked-root");
    std::os::unix::fs::symlink(&elsewhere, &root).expect("planting a symlinked root");

    let error = ArtifactRoot::open(&root).expect_err("a symlinked root must be refused");
    assert!(
        matches!(
            error,
            ArtifactError::UnsafePath {
                reason: UnsafePathReason::Symlink,
                ..
            }
        ),
        "expected a symlink refusal, got {error}"
    );
}

#[test]
fn a_non_directory_at_the_root_path_is_refused() {
    let harness = Harness::new();
    let root = harness.state_root().join("not-a-directory");
    std::fs::write(&root, b"x").expect("planting a file where the root should be");
    let error = ArtifactRoot::open(&root).expect_err("a file is not a root");
    assert!(
        matches!(
            error,
            ArtifactError::UnsafePath {
                reason: UnsafePathReason::NotDirectory,
                ..
            }
        ),
        "expected a directory refusal, got {error}"
    );
}

#[test]
fn an_orphan_sweep_never_follows_a_planted_symlink_out_of_the_root() {
    let harness = Harness::new();
    let outside = harness.state_root().join("outside.bin");
    std::fs::write(&outside, OUTSIDE_SECRET).expect("planting a file outside the root");
    let artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    std::os::unix::fs::symlink(
        &outside,
        harness.artifact_root().join("objects").join("ra-link"),
    )
    .expect("planting a symlink at a key");

    let scan = artifacts
        .scan_orphans(
            &std::collections::BTreeSet::new(),
            governor_core::time::Timestamp::from_unix_millis(i64::MAX),
        )
        .expect("sweeping");
    // The link is moved, not followed: `rename(2)` operates on the name.
    assert_eq!(scan.quarantined.len(), 1);
    assert_eq!(
        std::fs::read(&outside).expect("the outside file"),
        OUTSIDE_SECRET,
        "quarantine must move the name, never the target"
    );
    assert!(
        std::fs::symlink_metadata(harness.artifact_root().join("quarantine").join("ra-link"))
            .expect("the quarantined name")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn quarantine_never_overwrites_earlier_evidence() {
    let harness = Harness::new();
    let artifacts = harness.open_artifacts_with(
        config_with_grace(0, 0),
        Box::new(SequentialKeys::new(0)),
        None,
    );
    let objects = harness.artifact_root().join("objects");
    let now = governor_core::time::Timestamp::from_unix_millis(i64::MAX);

    std::fs::write(objects.join("ra-dup"), b"first").expect("first orphan");
    artifacts
        .scan_orphans(&std::collections::BTreeSet::new(), now)
        .expect("first sweep");
    std::fs::write(objects.join("ra-dup"), b"second").expect("second orphan");
    let scan = artifacts
        .scan_orphans(&std::collections::BTreeSet::new(), now)
        .expect("second sweep");

    assert_eq!(scan.quarantined[0].quarantine_name, "ra-dup.1");
    let quarantine = harness.artifact_root().join("quarantine");
    assert_eq!(
        std::fs::read(quarantine.join("ra-dup")).expect("the first"),
        b"first"
    );
    assert_eq!(
        std::fs::read(quarantine.join("ra-dup.1")).expect("the second"),
        b"second"
    );
}

#[test]
fn a_key_the_daemon_never_allocated_reads_as_missing_not_as_an_escape() {
    let harness = Harness::new();
    let artifacts = harness.open_artifacts();
    let error = artifacts
        .read(&metadata_for("ra-never-allocated", 1))
        .expect_err("an unknown key has no bytes");
    assert!(
        matches!(error, ArtifactError::Missing { .. }),
        "expected a missing artifact, got {error}"
    );
}

#[test]
fn an_over_permissive_root_is_repaired_rather_than_adopted() {
    let harness = Harness::new();
    let root = harness.state_root().join("loose");
    std::fs::create_dir(&root).expect("creating a root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777))
        .expect("loosening the root");

    let opened = ArtifactRoot::open(&root).expect("an over-permissive root is repaired");
    for dir in [
        opened.path().to_path_buf(),
        opened.objects_path().to_path_buf(),
        opened.incoming_path().to_path_buf(),
        opened.quarantine_path().to_path_buf(),
    ] {
        let mode = std::fs::metadata(&dir).expect("layout metadata").mode() & 0o777;
        assert_eq!(mode, 0o700, "{} must be owner-only", dir.display());
    }
}

#[test]
fn every_public_entry_point_takes_a_key_and_never_a_path() {
    // A compile-time statement written as a test so it is read: the only way
    // to name a stored artifact from outside this crate is a `StorageKey`
    // parsed from a bounded safe token. There is no `&Path` in the surface.
    let key = StorageKey::parse("ra-00000001").expect("a valid key");
    assert_eq!(key.as_str(), "ra-00000001");
    assert!(StorageKey::parse("/absolute").is_err());
}
