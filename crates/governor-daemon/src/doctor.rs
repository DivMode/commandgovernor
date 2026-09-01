//! State-root diagnosis that never takes authority.
//!
//! # The rule that shapes this module
//!
//! `doctor` must be safe to run against a state root a daemon already owns, and
//! safe to run against one nobody owns. Both cases forbid the same thing:
//! writing. A diagnostic that migrated a schema, advanced a daemon epoch, or
//! recorded a replay watermark would be a second daemon under another name,
//! which is exactly what `docs/testing.md` DB-005 refuses.
//!
//! So every check here is a read:
//!
//! - the instance lock is probed with a **shared**, immediately released lock,
//!   which answers "is a daemon holding this?" and cannot displace one;
//! - the database is opened `READ_ONLY` through
//!   [`governor_store_sqlite::inspect`], which reports the recorded schema
//!   epoch, the recorded daemon epoch, and the replay watermark against the
//!   ledger head — not a fresh replay, because proving replay equivalence
//!   writes the watermark;
//! - the filesystem checks are `symlink_metadata` and nothing else;
//! - when a daemon *is* running, its own half of the report is fetched over the
//!   control socket, so the authoritative numbers come from the authority.
//!
//! There is exactly one exception, and it is named rather than buried: learning
//! this process's effective user without a raw C call means creating a file and
//! reading its owner ([`crate::layout::effective_uid`]). `doctor` therefore
//! creates a zero-byte probe inside the state root and removes it immediately.
//! It mutates no durable state, opens no transaction, and takes no authority;
//! when the probe cannot be created that is itself a finding, reported as
//! `state_root_writable`.
//!
//! # The trust-model statement
//!
//! `docs/testing.md` SEC-007 requires that security metadata report the V1
//! trust model accurately, and specifically that no hostile same-user
//! containment be inferred from owner-only file modes. [`Diagnosis::lines`]
//! emits that as data — `trust.same_user_containment=false` and a named model —
//! rather than as prose a reader might not reach.

use std::path::Path;

use governor_store_sqlite::{ReadOnlyDiagnosis, StoreError};

use crate::error::PathDefect;
use crate::ipc::{self, Request};
use crate::layout::{self, PathClass, StateRoot};
use crate::lock::{self, LockStatus};
use crate::ports::SystemClock;
use crate::{report, startup};
use governor_store_sqlite::Clock as _;

/// The V1 administrative trust root, as `docs/architecture.md` states it.
const TRUST_MODEL: &str = "os_user_account";

/// One named check and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Stable check name.
    pub name: &'static str,
    /// Stable `snake_case` result code.
    pub result: &'static str,
    /// Optional bounded detail: a class or a counter, never content.
    pub detail: Option<String>,
    /// Whether this check passed.
    pub healthy: bool,
}

impl Check {
    fn ok(name: &'static str) -> Self {
        Self {
            name,
            result: "ok",
            detail: None,
            healthy: true,
        }
    }

    fn with(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn note(name: &'static str, result: &'static str) -> Self {
        Self {
            name,
            result,
            detail: None,
            healthy: true,
        }
    }

    fn fail(name: &'static str, result: &'static str) -> Self {
        Self {
            name,
            result,
            detail: None,
            healthy: false,
        }
    }
}

/// Everything a read-only look at a state root found.
#[derive(Debug, Clone)]
pub struct Diagnosis {
    /// The state root that was examined.
    pub state_root: StateRoot,
    /// What the instance lock says.
    pub lock: LockStatus,
    /// The named checks, in the order they ran.
    pub checks: Vec<Check>,
    /// Aggregate lines about open work, when the database could be read.
    pub summary: Vec<String>,
    /// The running daemon's own half of the report, when one answered.
    ///
    /// Rendered with a `live.` prefix, so a reader can tell an authoritative
    /// number the daemon supplied from one this read-only pass derived.
    pub daemon: Vec<String>,
}

impl Diagnosis {
    /// Whether every check passed.
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.checks.iter().all(|check| check.healthy)
    }

    /// The whole diagnosis, as stable greppable lines.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("doctor.state_root={}", self.state_root.path().display()),
            format!("doctor.daemon_running={}", self.lock.daemon_running()),
        ];
        for check in &self.checks {
            lines.push(match &check.detail {
                Some(detail) => format!(
                    "check name={} result={} detail={detail}",
                    check.name, check.result
                ),
                None => format!("check name={} result={}", check.name, check.result),
            });
        }
        lines.extend(self.summary.iter().cloned());
        lines.extend(self.daemon.iter().map(|line| format!("live.{line}")));

        // SEC-007: the trust model is reported, in the accurate form, every
        // time. It is not conditional on anything, because the guarantee it
        // describes is not conditional on anything either.
        lines.push(format!("trust.model={TRUST_MODEL}"));
        lines.push("trust.owner_only_file_modes=true".to_owned());
        lines.push("trust.protects_from_other_os_users=true".to_owned());
        lines.push("trust.same_user_containment=false".to_owned());
        lines.push("trust.ipc_peer_credential_check=false".to_owned());
        lines.push("trust.ipc_boundary=owner_only_directory_mode".to_owned());

        lines.push(format!(
            "doctor.result={}",
            if self.healthy() {
                "healthy"
            } else {
                "unhealthy"
            }
        ));
        lines
    }
}

/// Diagnoses a state root without taking authority over it.
#[must_use]
pub fn diagnose(root: &StateRoot) -> Diagnosis {
    let mut checks = Vec::new();

    let lock = lock::inspect(&root.lock_path());
    checks.push(lock_check(lock));

    let owner_uid = layout::effective_uid(root.path()).ok();
    checks.push(match owner_uid {
        Some(_) => Check::ok("state_root_writable"),
        None => Check::fail("state_root_writable", "not_writable_by_this_user"),
    });

    for &class in PathClass::ALL {
        let path = root.directory(class);
        let check = match owner_uid {
            None => Check::note(class_check_name(class), "not_checked"),
            Some(uid) => match layout::audit_dir(&path, uid) {
                Ok(()) => Check::ok(class_check_name(class)),
                Err(PathDefect::Uncreatable) if !path.exists() => {
                    Check::note(class_check_name(class), "absent")
                }
                Err(defect) => Check::fail(class_check_name(class), defect_code(defect)),
            },
        };
        checks.push(check);
    }

    checks.extend(artifact_layout_checks(root, owner_uid));

    // The socket *address* is diagnosable without touching the socket: a
    // state root whose pathname cannot fit a Unix socket address will refuse
    // to serve however healthy everything else looks, and a daemon start now
    // preflights the same check before it advances any durable state.
    checks.push(match ipc::check_socket_path(&root.socket_path()) {
        Ok(()) => Check::ok("control_socket_path"),
        Err(_) => Check::fail("control_socket_path", "path_too_long_for_socket_address"),
    });

    match owner_uid {
        None => checks.push(Check::note("control_socket", "not_checked")),
        Some(uid) => match ipc::audit(root, uid).into_iter().next() {
            Some((what, defect)) => checks.push(Check::fail(what, defect_code(defect))),
            None if root.socket_path().exists() => checks.push(Check::ok("control_socket")),
            None => checks.push(Check::note("control_socket", "absent")),
        },
    }

    if let Some(uid) = owner_uid {
        checks.extend(database_file_checks(root, uid));
    }

    let (store_checks, summary) = inspect_store(&root.database_path());
    checks.extend(store_checks);

    let daemon = if lock.daemon_running() {
        ipc::request(&root.socket_path(), Request::Doctor).unwrap_or_default()
    } else {
        Vec::new()
    };
    if lock.daemon_running() && daemon.is_empty() {
        checks.push(Check::fail("daemon_reachable", "socket_did_not_answer"));
    }

    Diagnosis {
        state_root: root.clone(),
        lock,
        checks,
        summary,
        daemon,
    }
}

fn lock_check(status: LockStatus) -> Check {
    match status {
        LockStatus::Absent => Check::note("instance_lock", "absent"),
        LockStatus::Held { slot } => {
            Check::note("instance_lock", "held").with(format!("slot_{slot}"))
        }
        LockStatus::Free { released: true, .. } => {
            Check::note("instance_lock", "free_after_clean_release")
        }
        LockStatus::Free { slot, .. } => Check::note("instance_lock", "free_after_unclean_exit")
            .with(slot.map_or_else(|| "slot_unknown".to_owned(), |slot| format!("slot_{slot}"))),
        // Not a fatal state, but nothing will start against it until the
        // operator removes the file, so it is a failure of the check.
        LockStatus::Unreadable => Check::fail("instance_lock", "unreadable_record"),
    }
}

/// Inspects the artifact root's fixed layout without opening or repairing it.
///
/// `ArtifactRoot::open` *repairs* modes on adoption, which a diagnosis must
/// never do, so this walks the three layout directories with the same
/// read-only audit the state-root directories get. Quarantine additionally
/// reports how many names it holds: set-aside orphans are evidence an
/// operator should see even when nothing is running.
fn artifact_layout_checks(root: &StateRoot, owner_uid: Option<u32>) -> Vec<Check> {
    let Some(uid) = owner_uid else {
        return Vec::new();
    };
    let artifact_root = root.artifact_root();
    if !artifact_root.exists() {
        return Vec::new();
    }

    let mut checks = Vec::new();
    for (dir, name) in [
        (governor_artifacts::OBJECTS_DIR, "artifact_objects_layout"),
        (governor_artifacts::INCOMING_DIR, "artifact_incoming_layout"),
        (
            governor_artifacts::QUARANTINE_DIR,
            "artifact_quarantine_layout",
        ),
    ] {
        let path = artifact_root.join(dir);
        checks.push(match layout::audit_dir(&path, uid) {
            Ok(()) => Check::ok(name),
            Err(PathDefect::Uncreatable) if !path.exists() => Check::note(name, "absent"),
            Err(defect) => Check::fail(name, defect_code(defect)),
        });
    }

    if let Ok(entries) = std::fs::read_dir(artifact_root.join(governor_artifacts::QUARANTINE_DIR)) {
        let held = entries.filter_map(Result::ok).count();
        if held > 0 {
            checks.push(
                Check::note("artifact_quarantine", "holds_evidence")
                    .with(format!("entries_{held}")),
            );
        }
    }
    checks
}

/// Checks that the durable authority's files are owner-only.
///
/// A daemon forces this on every start; `doctor` reports it, because a state
/// root whose database another principal can read is a finding even when
/// nothing is currently running.
fn database_file_checks(root: &StateRoot, owner_uid: u32) -> Vec<Check> {
    root.database_files()
        .into_iter()
        .filter(|path| path.exists())
        .filter_map(|path| {
            layout::audit_file(&path, owner_uid)
                .err()
                .map(|defect| Check::fail("database_file_private", defect_code(defect)))
        })
        .collect()
}

/// Reads the database read-only and turns what it found into checks.
fn inspect_store(database_path: &Path) -> (Vec<Check>, Vec<String>) {
    if !database_path.exists() {
        return (vec![Check::note("database", "absent")], Vec::new());
    }

    let diagnosis = match governor_store_sqlite::inspect::read_only(database_path) {
        Ok(diagnosis) => diagnosis,
        Err(error) => {
            return (
                vec![Check::fail("database", store_error_code(&error))],
                Vec::new(),
            );
        }
    };
    (checks_for(&diagnosis), summary_for(&diagnosis))
}

fn checks_for(diagnosis: &ReadOnlyDiagnosis) -> Vec<Check> {
    let mut checks = Vec::new();

    if !diagnosis.schema_present {
        checks.push(Check::note("database", "no_schema_yet"));
        return checks;
    }
    checks.push(Check::ok("database"));

    checks.push(if diagnosis.schema_too_new() {
        Check::fail("schema_epoch", "newer_than_this_binary").with(format!(
            "found_{}_supported_{}",
            diagnosis.schema_epoch.unwrap_or_default(),
            diagnosis.supported_schema_epoch
        ))
    } else {
        Check::ok("schema_epoch").with(format!(
            "epoch_{}",
            diagnosis.schema_epoch.unwrap_or_default()
        ))
    });

    // Not a verification: the watermark is what a *previous* authoritative open
    // proved, and saying otherwise would be claiming a check this process is
    // structurally unable to perform.
    let head = diagnosis.ledger_head.map_or(0, |seq| seq.get());
    let verified = diagnosis.verified_through.map_or(0, |seq| seq.get());
    checks.push(
        Check::note(
            "projection_replay",
            if diagnosis.replay_behind() {
                "not_verified_since_last_events"
            } else {
                "verified_through_ledger_head"
            },
        )
        .with(format!("verified_{verified}_of_{head}")),
    );

    checks.push(
        Check::ok("result_artifacts").with(format!("committed_{}", diagnosis.committed_artifacts)),
    );

    let pinned_missing = diagnosis
        .open_conditions
        .iter()
        .filter(|condition| {
            condition.kind == governor_core::health::HealthConditionKind::ResultArtifactMissing
        })
        .count();
    if pinned_missing > 0 {
        checks.push(
            Check::fail("pinned_artifacts", "result_artifact_missing")
                .with(format!("obligations_{pinned_missing}")),
        );
    }

    checks
}

fn summary_for(diagnosis: &ReadOnlyDiagnosis) -> Vec<String> {
    let mut lines = vec![
        format!("store.daemon_epoch={}", diagnosis.daemon_epoch.get()),
        format!(
            "store.ledger_head={}",
            diagnosis.ledger_head.map_or(0, |seq| seq.get())
        ),
    ];
    lines.extend(startup::summarise(
        &diagnosis.open_obligations,
        &diagnosis.open_conditions,
    ));
    lines.extend(report::obligation_lines(
        &diagnosis.open_obligations,
        SystemClock.now(),
    ));
    lines
}

const fn class_check_name(class: PathClass) -> &'static str {
    match class {
        PathClass::StateRoot => "state_root_layout",
        PathClass::Artifacts => "artifact_root_layout",
        PathClass::Ipc => "ipc_directory_layout",
        PathClass::Logs => "log_directory_layout",
    }
}

const fn defect_code(defect: PathDefect) -> &'static str {
    match defect {
        PathDefect::Uncreatable => "missing_or_uncreatable",
        PathDefect::NotADirectory => "not_a_directory",
        PathDefect::Symlink => "symlink",
        PathDefect::ForeignOwner => "foreign_owner",
        PathDefect::GroupOrOtherAccessible => "group_or_other_accessible",
        PathDefect::Unreadable => "metadata_unreadable",
    }
}

const fn store_error_code(error: &StoreError) -> &'static str {
    match error {
        StoreError::Conflict(_) => "conflict",
        StoreError::SchemaEpochTooNew { .. } => "schema_epoch_too_new",
        StoreError::MigrationChecksumMismatch { .. } => "migration_checksum_mismatch",
        StoreError::UnknownAppliedMigration { .. } => "unknown_applied_migration",
        StoreError::ConnectionPolicy(_) => "connection_policy",
        StoreError::Corrupt(_) => "corrupt_value",
        StoreError::RepairNeeded(_) => "repair_needed",
        StoreError::QuarantineIncomplete { .. } => "quarantine_incomplete",
        StoreError::WriterGone => "writer_gone",
        // Includes the read-only open of a database whose write-ahead log has
        // not been checkpointed: the honest answer is that its owner must
        // recover it, not that this diagnostic should.
        StoreError::Sqlite(_) => "unreadable_needs_owner_recovery",
        _ => "unclassified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_directory_is_diagnosed_rather_than_initialised() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = StateRoot::new(dir.path());
        let diagnosis = diagnose(&root);
        let lines = diagnosis.lines();

        assert!(lines.contains(&"doctor.daemon_running=false".to_owned()));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("name=database result=absent"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("name=instance_lock result=absent")),
            "{lines:#?}"
        );
        // The diagnosis must not have created the layout it was asked about.
        assert!(!root.database_path().exists());
        assert!(!root.artifact_root().exists());
        assert!(!root.lock_path().exists());
    }

    #[test]
    fn a_nonexistent_state_root_fails_legibly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = StateRoot::new(dir.path().join("nowhere"));
        let diagnosis = diagnose(&root);
        assert!(!diagnosis.healthy());
        assert!(
            diagnosis
                .lines()
                .contains(&"doctor.result=unhealthy".to_owned())
        );
    }

    #[test]
    fn a_corrupt_database_file_is_reported_rather_than_repaired() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = StateRoot::new(dir.path());
        std::fs::write(root.database_path(), b"not a database at all").expect("staged");

        let diagnosis = diagnose(&root);
        assert!(!diagnosis.healthy());
        assert!(
            diagnosis
                .lines()
                .iter()
                .any(|line| line.starts_with("check name=database result=")
                    && !line.contains("result=ok")),
            "{:#?}",
            diagnosis.lines()
        );
        assert_eq!(
            std::fs::read(root.database_path()).expect("still there"),
            b"not a database at all"
        );
    }

    #[test]
    fn the_trust_model_is_always_reported_and_never_claims_containment() {
        let dir = tempfile::tempdir().expect("temp dir");
        let lines = diagnose(&StateRoot::new(dir.path())).lines();
        assert!(lines.contains(&format!("trust.model={TRUST_MODEL}")));
        assert!(lines.contains(&"trust.same_user_containment=false".to_owned()));
        assert!(lines.contains(&"trust.protects_from_other_os_users=true".to_owned()));
        assert!(lines.contains(&"trust.ipc_peer_credential_check=false".to_owned()));
    }
}
