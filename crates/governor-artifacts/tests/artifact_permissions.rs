//! ART-005 and SEC-007 — owner-only modes, and exactly what they claim.
//!
//! `docs/testing.md` ART-005: *on Unix verify intended private modes
//! regardless of host umask. This test proves privacy against other OS
//! principals, **not** hostile same-user worker containment.*
//!
//! # Why a child process
//!
//! The umask is per-process, and Rust's test harness runs tests as threads of
//! one process, so setting it in place would silently change the modes every
//! other test observes. `libc::umask` is also an `unsafe` call, which this
//! workspace denies outright. So the hostile-umask cases re-execute *this test
//! binary* under `sh -c 'umask 0777; exec …'`, filtered to the one test, and
//! the child does the asserting. The umask is then genuinely inherited from the
//! environment, which is the situation being tested.
//!
//! # SEC-007: what is not claimed
//!
//! `SECURITY.md` "Local trust model": the local OS user is the administrative
//! trust root, and `0600`/`0700` protect the store from **other OS
//! principals**. Claude and its tools normally run as that same user, so a
//! deliberately malicious same-user process is *not* contained by these modes —
//! containing it needs a separate-user, sandbox or broker design that V1 has
//! not built. [`the_modes_are_privacy_from_other_users_and_not_a_same_user_sandbox`]
//! asserts that limit out loud rather than leaving it to a comment, so that a
//! future change which quietly starts depending on same-user containment fails
//! a test instead of shipping.

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::process::Command;

use governor_artifacts::ArtifactFailpoint;
use support::{
    ArtifactFireOnce, FINAL_RESULT, Harness, SequentialKeys, config_with_grace, publish_bytes,
};

/// Marker telling a re-executed binary it is the child.
const CHILD_MARKER: &str = "CG_ARTIFACT_UMASK_CHILD";

/// The most hostile umask there is: every permission bit stripped from every
/// newly created file and directory.
const HOSTILE_UMASK: &str = "0777";

/// Runs `body` under a hostile umask, in a child process.
///
/// Returns immediately in the parent once the child has passed.
fn under_hostile_umask(test_name: &str, body: impl FnOnce()) {
    if std::env::var_os(CHILD_MARKER).is_some() {
        body();
        return;
    }
    let exe = std::env::current_exe().expect("the test binary");
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            r#"umask {HOSTILE_UMASK}; exec "$0" --exact --nocapture "$1""#
        ))
        .arg(&exe)
        .arg(test_name)
        .env(CHILD_MARKER, "1")
        .status()
        .expect("re-executing the test binary under a hostile umask");
    assert!(
        status.success(),
        "the hostile-umask child failed; its output is above"
    );
}

fn mode_of(path: &std::path::Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn layout_directories_are_owner_only_regardless_of_the_host_umask() {
    under_hostile_umask(
        "layout_directories_are_owner_only_regardless_of_the_host_umask",
        || {
            let harness = Harness::new();
            let artifacts = harness.open_artifacts();
            let root = artifacts.root();
            for dir in [
                root.path(),
                root.objects_path(),
                root.incoming_path(),
                root.quarantine_path(),
            ] {
                assert_eq!(
                    mode_of(dir),
                    0o700,
                    "{} must be 0700 under umask {HOSTILE_UMASK}",
                    dir.display()
                );
            }
        },
    );
}

#[test]
fn a_published_artifact_is_owner_only_regardless_of_the_host_umask() {
    under_hostile_umask(
        "a_published_artifact_is_owner_only_regardless_of_the_host_umask",
        || {
            let harness = Harness::new();
            let mut artifacts = harness.open_artifacts();
            let published = publish_bytes(&mut artifacts, FINAL_RESULT).expect("publishing");
            let stored = harness
                .artifact_root()
                .join("objects")
                .join(published.key().to_string());
            assert_eq!(
                mode_of(&stored),
                0o600,
                "a published artifact must be 0600 under umask {HOSTILE_UMASK}"
            );
            // And it is still readable by its owner, which the umask alone
            // would have prevented: `mode(0o600)` under this umask yields 000.
            assert_eq!(
                std::fs::read(&stored).expect("owner can read"),
                FINAL_RESULT
            );
        },
    );
}

#[test]
fn a_staging_file_is_owner_only_before_it_is_ever_published() {
    under_hostile_umask(
        "a_staging_file_is_owner_only_before_it_is_ever_published",
        || {
            // A crash right after creation leaves the staging file behind; the
            // window in which the bytes are least protected is exactly the one
            // worth checking.
            let harness = Harness::new();
            let mut artifacts = harness.open_artifacts_with(
                config_with_grace(0, 0),
                Box::new(SequentialKeys::new(0)),
                Some(Box::new(ArtifactFireOnce::new(
                    ArtifactFailpoint::AfterTempCreate,
                ))),
            );
            publish_bytes(&mut artifacts, FINAL_RESULT).expect_err("the injected crash");

            let staged = harness.files_in("incoming");
            assert_eq!(staged.len(), 1, "one staging file is the residue");
            assert_eq!(
                mode_of(&harness.artifact_root().join("incoming").join(&staged[0])),
                0o600,
                "a staging file must be 0600 under umask {HOSTILE_UMASK}"
            );
        },
    );
}

#[test]
fn a_quarantined_orphan_keeps_its_owner_only_mode() {
    under_hostile_umask("a_quarantined_orphan_keeps_its_owner_only_mode", || {
        let harness = Harness::new();
        let mut artifacts = harness.open_artifacts_with(
            config_with_grace(0, 0),
            Box::new(SequentialKeys::new(0)),
            None,
        );
        let published = publish_bytes(&mut artifacts, FINAL_RESULT).expect("publishing");
        let scan = artifacts
            .scan_orphans(
                &std::collections::BTreeSet::new(),
                governor_core::time::Timestamp::from_unix_millis(i64::MAX),
            )
            .expect("sweeping");
        assert_eq!(scan.quarantined.len(), 1);
        assert_eq!(
            mode_of(
                &harness
                    .artifact_root()
                    .join("quarantine")
                    .join(published.key().to_string())
            ),
            0o600,
            "quarantine must not loosen a mode"
        );
    });
}

#[test]
fn the_modes_are_privacy_from_other_users_and_not_a_same_user_sandbox() {
    // SEC-007. This test exists to make the boundary falsifiable rather than
    // rhetorical, so it asserts *both* halves.
    let harness = Harness::new();
    let mut artifacts = harness.open_artifacts();
    let published = publish_bytes(&mut artifacts, FINAL_RESULT).expect("publishing");
    let stored = harness
        .artifact_root()
        .join("objects")
        .join(published.key().to_string());

    // The half that is claimed: no group or other principal has any access,
    // to the bytes or to the directories leading to them.
    assert_eq!(mode_of(&stored) & 0o077, 0, "no group or other access");
    for dir in [
        harness.artifact_root(),
        harness.artifact_root().join("objects"),
        harness.artifact_root().join("incoming"),
        harness.artifact_root().join("quarantine"),
    ] {
        assert_eq!(
            mode_of(&dir) & 0o077,
            0,
            "{}: no group or other access",
            dir.display()
        );
    }

    // The half that is *not* claimed, demonstrated rather than described: this
    // test process is the same OS user as the "daemon" that wrote the file, and
    // it can read it, rewrite it, and unlink it. A worker running as that user
    // could do the same. Nothing here contains it, and no code in this crate
    // may be written as though something did.
    assert_eq!(
        std::fs::read(&stored).expect("the same user can read it"),
        FINAL_RESULT
    );
    std::fs::write(&stored, b"a same-user process rewrote this").expect("and can rewrite it");

    // What the store *does* guarantee against that actor is detection, not
    // prevention: the tampering is caught, closed, and never returned.
    let error = artifacts
        .read_verified(published.key(), published.digest(), published.byte_len())
        .expect_err("tampering must be detected");
    assert!(
        error.is_artifact_unusable(),
        "same-user tampering must fail closed, got {error}"
    );
}
