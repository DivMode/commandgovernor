//! Opening the database, and proving the connection policy is actually on.
//!
//! `docs/data-model.md` "SQLite policy" and `docs/architecture.md` "SQLite"
//! require foreign keys, WAL, `synchronous=FULL`, and a bounded busy timeout.
//! Issuing the `PRAGMA` statements is not the same as having them in force —
//! `journal_mode` silently refuses to change for an in-memory database or one
//! held open by another connection in a transaction — so every pragma is read
//! back from the engine and a disagreement is a fail-closed
//! [`StoreError::ConnectionPolicy`].

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::{PolicyViolation, StoreResult};

/// Default bounded busy timeout.
///
/// The store has exactly one writer, so contention can only come from a second
/// process against the same state root. Waiting a bounded time and then failing
/// is right; waiting forever would hide a two-daemon situation the daemon lock
/// is supposed to catch.
pub const DEFAULT_BUSY_TIMEOUT_MS: u32 = 5_000;

/// How to open the durable store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    /// Path to the SQLite database file.
    ///
    /// A file, never `:memory:`: WAL and `synchronous=FULL` are meaningless
    /// without one, and the crash suites need to reopen the same bytes.
    pub database_path: PathBuf,
    /// Bounded busy timeout in milliseconds.
    pub busy_timeout_ms: u32,
}

impl StoreConfig {
    /// Configures a store at `database_path` with the default busy timeout.
    #[must_use]
    pub fn new(database_path: impl AsRef<Path>) -> Self {
        Self {
            database_path: database_path.as_ref().to_path_buf(),
            busy_timeout_ms: DEFAULT_BUSY_TIMEOUT_MS,
        }
    }

    /// Overrides the bounded busy timeout.
    #[must_use]
    pub const fn with_busy_timeout_ms(mut self, millis: u32) -> Self {
        self.busy_timeout_ms = millis;
        self
    }
}

/// The connection policy as the engine reports it.
///
/// Returned so `command-governor doctor` and the store suite can show the
/// values rather than assert on a boolean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyReport {
    /// `PRAGMA foreign_keys`.
    pub foreign_keys: bool,
    /// `PRAGMA journal_mode`.
    pub journal_mode: String,
    /// `PRAGMA synchronous`, as the engine's numeric level.
    pub synchronous: i64,
    /// `PRAGMA busy_timeout`, in milliseconds.
    pub busy_timeout_ms: i64,
}

/// `synchronous=FULL` is level 2.
const SYNCHRONOUS_FULL: i64 = 2;

/// Opens the database and applies the required connection policy.
///
/// # Errors
///
/// Returns [`crate::error::StoreError::Sqlite`] when the file cannot be opened,
/// or [`crate::error::StoreError::ConnectionPolicy`] when a required pragma is
/// not in force afterwards.
pub(crate) fn open(config: &StoreConfig) -> StoreResult<(Connection, PolicyReport)> {
    let conn = Connection::open_with_flags(
        &config.database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    // Order matters. `journal_mode` must be set before any transaction is
    // opened, and `foreign_keys` is a no-op inside one.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(
        config.busy_timeout_ms,
    )))?;

    let report = read_policy(&conn)?;
    verify(&report, config)?;
    Ok((conn, report))
}

/// Reads the policy back from the engine.
///
/// # Errors
///
/// Returns a SQLite error when a pragma cannot be queried.
pub(crate) fn read_policy(conn: &Connection) -> StoreResult<PolicyReport> {
    let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    let journal_mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let busy_timeout_ms: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    Ok(PolicyReport {
        foreign_keys: foreign_keys != 0,
        journal_mode,
        synchronous,
        busy_timeout_ms,
    })
}

fn verify(report: &PolicyReport, config: &StoreConfig) -> StoreResult<()> {
    if !report.foreign_keys {
        return Err(PolicyViolation {
            pragma: "foreign_keys",
            observed: "off".to_owned(),
            required: "on",
        }
        .into());
    }
    if !report.journal_mode.eq_ignore_ascii_case("wal") {
        return Err(PolicyViolation {
            pragma: "journal_mode",
            observed: report.journal_mode.clone(),
            required: "wal",
        }
        .into());
    }
    if report.synchronous != SYNCHRONOUS_FULL {
        return Err(PolicyViolation {
            pragma: "synchronous",
            observed: report.synchronous.to_string(),
            required: "2 (FULL)",
        }
        .into());
    }
    if report.busy_timeout_ms != i64::from(config.busy_timeout_ms) {
        return Err(PolicyViolation {
            pragma: "busy_timeout",
            observed: report.busy_timeout_ms.to_string(),
            required: "the configured bounded timeout",
        }
        .into());
    }
    Ok(())
}
