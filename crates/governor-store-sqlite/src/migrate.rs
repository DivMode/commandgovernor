//! Explicit, deterministic, checksummed migrations.
//!
//! # What "deterministic" buys
//!
//! Each migration is a numbered SQL file compiled into the binary, recorded in
//! `schema_migrations` with the SHA-256 of the exact bytes that were applied.
//! On every open the recorded checksums are compared with the ones this binary
//! carries; a disagreement means the database was built by a different
//! definition of the same version, and the store fails closed rather than
//! layering a guess on top.
//!
//! # Crash recovery
//!
//! One migration is one `BEGIN IMMEDIATE` transaction that both applies the DDL
//! and writes its `schema_migrations` row and the new epoch. SQLite makes DDL
//! transactional, so a crash anywhere inside it rolls the whole migration back
//! and reopening applies it again from a known state (`docs/testing.md`
//! DB-004). There is no window in which the schema has moved but the ledger of
//! migrations has not.
//!
//! # Epoch
//!
//! `docs/data-model.md`: *schema compatibility is a monotonic application
//! epoch. A binary fails closed on an unknown newer epoch.* The epoch is read
//! **before** anything is applied, so an older binary opening a newer database
//! refuses without touching it (`docs/testing.md` DB-003).

use governor_core::time::Timestamp;
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::codec::{hex32, store_time};
use crate::error::{StoreError, StoreResult};
use crate::meta;
use crate::tx::{Failpoint, FailpointHook, Tx};

/// Highest schema epoch this binary implements.
pub const SUPPORTED_SCHEMA_EPOCH: u32 = 2;

/// One numbered migration.
struct Migration {
    version: u32,
    name: &'static str,
    /// Epoch the database is at once this migration has been applied.
    epoch: u32,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        epoch: 1,
        sql: include_str!("migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "session_lineage_and_loadouts",
        epoch: 2,
        sql: include_str!("migrations/0002_session_lineage_and_loadouts.sql"),
    },
];

impl Migration {
    fn checksum(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.sql.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        hex32(&digest)
    }
}

/// What opening the store did to the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Versions applied during this open, in order.
    pub applied: Vec<u32>,
    /// Versions that were already applied and whose checksums verified.
    pub verified: Vec<u32>,
    /// Schema epoch after this open.
    pub epoch: u32,
    /// Opaque identity of this database file.
    pub database_instance_id: Uuid,
}

/// Brings the database up to this binary's schema, or fails closed.
///
/// # Errors
///
/// - [`StoreError::SchemaEpochTooNew`] when the database is newer than this
///   binary (nothing is read or written beyond the epoch itself);
/// - [`StoreError::MigrationChecksumMismatch`] when an applied migration's
///   definition drifted;
/// - [`StoreError::UnknownAppliedMigration`] when the database records a
///   version this binary does not carry;
/// - a SQLite error from applying a migration.
pub(crate) fn migrate(
    conn: &mut Connection,
    now: Timestamp,
    instance_id: Uuid,
    hook: Option<&dyn FailpointHook>,
) -> StoreResult<MigrationReport> {
    // The epoch gate runs first and reads nothing else, so an older binary
    // cannot so much as inspect a newer database's tables.
    if meta::schema_exists(conn)?
        && let Some(found) = meta::schema_epoch(conn)?
        && found > SUPPORTED_SCHEMA_EPOCH
    {
        return Err(StoreError::SchemaEpochTooNew {
            found,
            supported: SUPPORTED_SCHEMA_EPOCH,
        });
    }

    let recorded = if meta::schema_exists(conn)? {
        read_recorded(conn)?
    } else {
        Vec::new()
    };

    let mut verified = Vec::new();
    for (version, name, checksum) in &recorded {
        let known = MIGRATIONS
            .iter()
            .find(|m| m.version == *version)
            .ok_or(StoreError::UnknownAppliedMigration { version: *version })?;
        if &known.checksum() != checksum {
            return Err(StoreError::MigrationChecksumMismatch {
                version: *version,
                name: name.clone(),
            });
        }
        verified.push(*version);
    }

    let mut applied = Vec::new();
    for migration in MIGRATIONS {
        if verified.contains(&migration.version) {
            continue;
        }
        apply(conn, migration, now, hook)?;
        applied.push(migration.version);
    }

    let epoch = meta::schema_epoch(conn)?.unwrap_or(SUPPORTED_SCHEMA_EPOCH);
    let database_instance_id = ensure_instance_id(conn, instance_id)?;

    Ok(MigrationReport {
        applied,
        verified,
        epoch,
        database_instance_id,
    })
}

fn read_recorded(conn: &Connection) -> StoreResult<Vec<(u32, String, String)>> {
    let mut statement =
        conn.prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (version, name, checksum) = row?;
        out.push((
            crate::codec::parse_u32(version, "schema_migrations", "version")?,
            name,
            checksum,
        ));
    }
    Ok(out)
}

fn apply(
    conn: &mut Connection,
    migration: &Migration,
    now: Timestamp,
    hook: Option<&dyn FailpointHook>,
) -> StoreResult<()> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let tx = Tx::new(&transaction, hook, "migrate");
        tx.conn().execute_batch(migration.sql)?;
        tx.reach(Failpoint::BeforeMigrationRecorded)?;
        tx.conn().execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                i64::from(migration.version),
                migration.name,
                migration.checksum(),
                store_time(now),
            ],
        )?;
        meta::put(tx.conn(), meta::SCHEMA_EPOCH, &migration.epoch.to_string())?;
        tx.reach(Failpoint::BeforeCommit)?;
    }
    transaction.commit()?;
    Ok(())
}

fn ensure_instance_id(conn: &Connection, candidate: Uuid) -> StoreResult<Uuid> {
    if let Some(existing) = meta::get(conn, meta::DATABASE_INSTANCE_ID)? {
        return Uuid::parse_str(&existing).map_err(|_| {
            crate::error::CorruptValue::new(
                "meta",
                "database_instance_id",
                crate::error::CorruptReason::MalformedIdentity,
            )
            .into()
        });
    }
    meta::put(
        conn,
        meta::DATABASE_INSTANCE_ID,
        &candidate.hyphenated().to_string(),
    )?;
    Ok(candidate)
}
