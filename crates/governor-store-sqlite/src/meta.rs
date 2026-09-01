//! The `meta` key/value table.
//!
//! `docs/data-model.md`: *meta may contain schema epoch, database instance ID,
//! and last verified projection sequence. It must not contain credentials or
//! browser session material.* The keys are therefore a closed set declared here
//! and there is no API that writes an arbitrary key.

use governor_core::fence::{DaemonEpoch, EventSeq};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::codec::{parse_u32, parse_u64, store_u64};
use crate::error::{CorruptReason, CorruptValue, StoreResult};

/// Monotonic application schema epoch.
pub(crate) const SCHEMA_EPOCH: &str = "schema_epoch";
/// Opaque identity of this database file, minted once at creation.
pub(crate) const DATABASE_INSTANCE_ID: &str = "database_instance_id";
/// Highest event sequence whose projections were verified by replay.
pub(crate) const LAST_VERIFIED_PROJECTION_SEQ: &str = "last_verified_projection_seq";
/// Lifetime counter of the owning daemon process.
///
/// Startup advances it once. Every mutation-command row, external-effect intent
/// and resource lease records the epoch it was written under, which is what
/// makes "this row is from a previous process" a fact rather than a guess.
pub(crate) const DAEMON_EPOCH: &str = "daemon_epoch";

/// Reads a meta value.
///
/// # Errors
///
/// Returns a SQLite error.
pub(crate) fn get(conn: &Connection, key: &str) -> StoreResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

/// Writes a meta value.
///
/// # Errors
///
/// Returns a SQLite error.
pub(crate) fn put(conn: &Connection, key: &str, value: &str) -> StoreResult<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Reads the recorded schema epoch, if the table exists and holds one.
///
/// # Errors
///
/// Returns a corrupt-row error when the stored epoch is not a number.
pub(crate) fn schema_epoch(conn: &Connection) -> StoreResult<Option<u32>> {
    let Some(text) = get(conn, SCHEMA_EPOCH)? else {
        return Ok(None);
    };
    let value: i64 = text
        .parse()
        .map_err(|_| CorruptValue::new("meta", "schema_epoch", CorruptReason::IntegerOutOfRange))?;
    Ok(Some(parse_u32(value, "meta", "schema_epoch")?))
}

/// Reads the last verified projection watermark.
///
/// # Errors
///
/// Returns a corrupt-row error when the stored value is not a sequence.
pub(crate) fn last_verified_projection_seq(conn: &Connection) -> StoreResult<Option<EventSeq>> {
    let Some(text) = get(conn, LAST_VERIFIED_PROJECTION_SEQ)? else {
        return Ok(None);
    };
    let value: i64 = text.parse().map_err(|_| {
        CorruptValue::new(
            "meta",
            "last_verified_projection_seq",
            CorruptReason::IntegerOutOfRange,
        )
    })?;
    Ok(Some(EventSeq::new(parse_u64(
        value,
        "meta",
        "last_verified_projection_seq",
    )?)))
}

/// Records the last verified projection watermark.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-value error for an unstorable sequence.
pub(crate) fn set_last_verified_projection_seq(
    conn: &Connection,
    seq: EventSeq,
) -> StoreResult<()> {
    let value = store_u64(seq.get(), "meta", "last_verified_projection_seq")?;
    put(conn, LAST_VERIFIED_PROJECTION_SEQ, &value.to_string())
}

/// Reads the daemon epoch the database was last opened under.
///
/// # Errors
///
/// Returns a corrupt-row error when the stored value is not a counter.
pub(crate) fn daemon_epoch(conn: &Connection) -> StoreResult<DaemonEpoch> {
    let Some(text) = get(conn, DAEMON_EPOCH)? else {
        // A database that has never been opened by a daemon is at the epoch
        // before the first one, so the first startup advances to `FIRST`.
        return Ok(DaemonEpoch::new(0));
    };
    let value: i64 = text
        .parse()
        .map_err(|_| CorruptValue::new("meta", "daemon_epoch", CorruptReason::IntegerOutOfRange))?;
    Ok(DaemonEpoch::new(parse_u64(value, "meta", "daemon_epoch")?))
}

/// Advances the daemon epoch and returns the new value.
///
/// # Errors
///
/// Returns a SQLite error, or a corrupt-row error for an unreadable epoch.
pub(crate) fn advance_daemon_epoch(conn: &Connection) -> StoreResult<DaemonEpoch> {
    let next = daemon_epoch(conn)?.next();
    let value = store_u64(next.get(), "meta", "daemon_epoch")?;
    put(conn, DAEMON_EPOCH, &value.to_string())?;
    Ok(next)
}

/// Reports whether the schema has been created at all.
///
/// # Errors
///
/// Returns a SQLite error.
pub(crate) fn schema_exists(conn: &Connection) -> StoreResult<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}
