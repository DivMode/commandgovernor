//! Safe diagnostics, and a shape that cannot carry anything else.
//!
//! # The rule
//!
//! `docs/threat-model.md`, "Threat: diagnostics become exfiltration": safe
//! diagnostics may include *opaque IDs, state/event classes, counts, durations,
//! versions/commits, redacted route class, and boolean evidence flags*, and
//! must not include prompt or result content, repository content, a working
//! directory, a transcript path, a provider stream, browser headers, cookies or
//! tokens, credentials, or arbitrary environment variables.
//!
//! # Why this is a type rather than a convention
//!
//! A logger that took a formatted string would satisfy the rule only for as
//! long as everybody remembered it. [`Fields`] instead accepts a closed set of
//! shapes, none of which can carry free text:
//!
//! - [`Fields::class`] takes `&'static str`, so the value is a literal in this
//!   binary and cannot be a runtime string;
//! - [`Fields::int`], [`Fields::count`] and [`Fields::flag`] are numbers and
//!   booleans;
//! - [`Fields::id`] takes an opaque domain identity;
//! - [`Fields::token`] takes a [`SafeToken`], whose charset already refuses
//!   whitespace, quotes and `/`, and so cannot hold prose, a command, or a
//!   path.
//!
//! There is deliberately no `&str` accessor. Adding one would be the change
//! that reopens the hole, and it would be visible in review.
//!
//! The log lives at `<state-root>/logs/daemon.log`, inside the layout the
//! SEC-001 sentinel sweep already walks.

use core::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;

use governor_core::fence::SafeToken;
use governor_core::id::{Id, IdKind};
use governor_core::time::Timestamp;

use crate::layout::PRIVATE_FILE_MODE;

/// Name of the daemon's diagnostics file.
const LOG_FILE: &str = "daemon.log";
/// Name the previous generation is rotated to.
const ROTATED_FILE: &str = "daemon.log.1";
/// Size at which the log is rotated, in bytes.
///
/// One generation, one megabyte. A daemon that writes more than this in safe
/// diagnostics has a reporting bug, and unbounded growth inside the state root
/// is a defect in its own right.
const ROTATE_AT_BYTES: u64 = 1 << 20;

/// How serious a record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Level {
    /// Normal progress.
    Info,
    /// Something the operator should look at; the daemon continued.
    Warn,
    /// A refusal. The daemon did not become ready, or is stopping.
    Error,
}

impl Level {
    const fn code(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// A record's structured payload: classes, counters, flags and opaque IDs.
#[derive(Debug, Clone, Default)]
pub struct Fields {
    rendered: String,
}

impl Fields {
    /// An empty payload.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a class or code that is a literal in this binary.
    #[must_use]
    pub fn class(mut self, key: &'static str, value: &'static str) -> Self {
        self.push(key, value);
        self
    }

    /// Adds a signed counter or duration in milliseconds.
    #[must_use]
    pub fn int(mut self, key: &'static str, value: i64) -> Self {
        let mut rendered = String::new();
        let _ = write!(rendered, "{value}");
        self.push(key, &rendered);
        self
    }

    /// Adds a cardinality.
    #[must_use]
    pub fn count(self, key: &'static str, value: usize) -> Self {
        self.int(key, i64::try_from(value).unwrap_or(i64::MAX))
    }

    /// Adds a boolean evidence flag.
    #[must_use]
    pub fn flag(mut self, key: &'static str, value: bool) -> Self {
        self.push(key, if value { "true" } else { "false" });
        self
    }

    /// Adds an opaque domain identity.
    #[must_use]
    pub fn id<K: IdKind>(mut self, key: &'static str, value: Id<K>) -> Self {
        let rendered = value.to_string();
        self.push(key, &rendered);
        self
    }

    /// Adds a redaction-safe token.
    #[must_use]
    pub fn token(mut self, key: &'static str, value: &SafeToken) -> Self {
        self.push(key, value.as_str());
        self
    }

    fn push(&mut self, key: &'static str, value: &str) {
        if !self.rendered.is_empty() {
            self.rendered.push(' ');
        }
        self.rendered.push_str(key);
        self.rendered.push('=');
        self.rendered.push_str(value);
    }

    /// The payload as it will appear in the log.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.rendered
    }
}

/// The daemon's append-only diagnostics file.
#[derive(Debug)]
pub struct SafeLog {
    file: Option<Mutex<File>>,
}

impl SafeLog {
    /// Opens `<dir>/daemon.log`, rotating one generation if it has grown.
    ///
    /// # Errors
    ///
    /// Returns the underlying failure when the file cannot be opened.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt as _;

        let path = dir.join(LOG_FILE);
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() >= ROTATE_AT_BYTES) {
            let _ = std::fs::rename(&path, dir.join(ROTATED_FILE));
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(PRIVATE_FILE_MODE)
            .open(&path)?;
        Ok(Self {
            file: Some(Mutex::new(file)),
        })
    }

    /// A log that discards everything, for tests and for `doctor`.
    #[must_use]
    pub const fn discarding() -> Self {
        Self { file: None }
    }

    /// Appends one record.
    ///
    /// Writing a diagnostic must never take the process down, so an I/O failure
    /// here is dropped. The facts that matter are durable in SQLite; this file
    /// is an operator convenience.
    pub fn record(&self, at: Timestamp, level: Level, event: &'static str, fields: &Fields) {
        let Some(file) = &self.file else {
            return;
        };
        let mut line = String::with_capacity(96);
        let _ = write!(line, "{} {} {}", at.as_unix_millis(), level.code(), event);
        if !fields.as_str().is_empty() {
            line.push(' ');
            line.push_str(fields.as_str());
        }
        line.push('\n');

        if let Ok(mut handle) = file.lock() {
            let _ = handle.write_all(line.as_bytes());
            let _ = handle.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor_core::id::ObligationId;

    #[test]
    fn a_record_renders_classes_counts_flags_and_identities() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = SafeLog::open(dir.path()).expect("opened");
        let id = ObligationId::from_uuid(uuid::Uuid::from_u128(7));
        log.record(
            Timestamp::from_unix_millis(1_700_000_000_000),
            Level::Info,
            "daemon.ready",
            &Fields::new()
                .class("phase", "ready")
                .count("open_obligations", 3)
                .flag("reclaimed_lock", false)
                .id("obligation", id),
        );

        let text = std::fs::read_to_string(dir.path().join(LOG_FILE)).expect("read back");
        assert_eq!(
            text,
            format!(
                "1700000000000 info daemon.ready phase=ready open_obligations=3 reclaimed_lock=false obligation={id}\n"
            )
        );
    }

    #[test]
    fn the_log_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("temp dir");
        let _log = SafeLog::open(dir.path()).expect("opened");
        let mode = std::fs::metadata(dir.path().join(LOG_FILE))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, PRIVATE_FILE_MODE, "mode was {mode:o}");
    }

    #[test]
    fn an_oversized_log_is_rotated_once_rather_than_growing_without_bound() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join(LOG_FILE),
            vec![
                b'x';
                usize::try_from(ROTATE_AT_BYTES).expect("the rotation threshold fits in a usize")
            ],
        )
        .expect("oversized log");
        let _log = SafeLog::open(dir.path()).expect("opened");
        assert!(dir.path().join(ROTATED_FILE).exists());
        assert_eq!(
            std::fs::metadata(dir.path().join(LOG_FILE))
                .expect("metadata")
                .len(),
            0
        );
    }

    #[test]
    fn a_discarding_log_writes_nothing() {
        let log = SafeLog::discarding();
        log.record(
            Timestamp::from_unix_millis(0),
            Level::Error,
            "daemon.refused",
            &Fields::new().class("reason", "authority_held"),
        );
    }
}
