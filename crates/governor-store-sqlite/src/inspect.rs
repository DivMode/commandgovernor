//! Read-only diagnosis of a database that another process may own.
//!
//! # Why this exists separately from [`crate::store`]
//!
//! [`crate::OpenStore::start`] is the *authoritative* open: it migrates, it
//! advances the daemon epoch, it writes the replay watermark, and it
//! quarantines orphaned effects. Every one of those is a write, and a write is
//! exactly what `command-governor doctor` must not do — a diagnostic that takes
//! write authority over a state root is a second daemon by another name
//! (`docs/testing.md` DB-005).
//!
//! So this module opens the same file with `SQLITE_OPEN_READ_ONLY` and reads
//! what it can prove without changing anything: the recorded schema epoch, the
//! recorded daemon epoch, the replay watermark against the ledger head, the
//! open obligations, and the open health conditions.
//!
//! # What it deliberately cannot tell you
//!
//! Projection replay equivalence is **not** checked here. [`crate::replay`]
//! records its watermark as part of proving equivalence, so running it would be
//! a write. What a read-only caller gets instead is the honest weaker fact —
//! the watermark the last authoritative open reached, and the current ledger
//! head — from which a caller can say "verified through N of M" rather than
//! claim a verification it did not perform.
//!
//! A database whose write-ahead log has not been checkpointed cannot always be
//! opened read-only at all, because the shared-memory index may need creating.
//! That is reported as a plain SQLite failure rather than papered over: the
//! honest answer is "this root needs its owner to recover it".

use std::path::Path;

use governor_core::fence::{DaemonEpoch, EventSeq};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::error::StoreResult;
use crate::load::{OpenCondition, OpenObligation};
use crate::migrate::SUPPORTED_SCHEMA_EPOCH;
use crate::tx::Tx;

/// What a read-only look at a state root's database found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadOnlyDiagnosis {
    /// Whether the schema has been created at all.
    pub schema_present: bool,
    /// Schema epoch recorded in the database, when there is one.
    pub schema_epoch: Option<u32>,
    /// Highest schema epoch this binary implements.
    pub supported_schema_epoch: u32,
    /// Daemon epoch the last authoritative open advanced to.
    pub daemon_epoch: DaemonEpoch,
    /// Highest ledger sequence a previous process proved projections through.
    pub verified_through: Option<EventSeq>,
    /// Highest ledger sequence currently recorded.
    pub ledger_head: Option<EventSeq>,
    /// Obligations that still owe somebody something.
    pub open_obligations: Vec<OpenObligation>,
    /// Conditions currently demanding attention.
    pub open_conditions: Vec<OpenCondition>,
    /// Committed result-artifact rows.
    pub committed_artifacts: usize,
}

impl ReadOnlyDiagnosis {
    /// Reports whether the database is newer than this binary understands.
    ///
    /// `docs/testing.md` DB-003: an older binary must refuse rather than guess.
    #[must_use]
    pub const fn schema_too_new(&self) -> bool {
        match self.schema_epoch {
            Some(found) => found > self.supported_schema_epoch,
            None => false,
        }
    }

    /// Reports whether the ledger has advanced past the last proven replay.
    ///
    /// True after any process stopped before it could re-verify, which is
    /// normal for a crash and is a fact worth showing rather than a fault.
    #[must_use]
    pub fn replay_behind(&self) -> bool {
        match (self.ledger_head, self.verified_through) {
            (Some(head), Some(verified)) => head.get() > verified.get(),
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

/// Reads everything a diagnostic may know without taking write authority.
///
/// # Errors
///
/// - a SQLite error when the file cannot be opened read-only, which includes
///   the case of a write-ahead log that still needs recovering;
/// - a corrupt-row error when a stored value cannot be rehydrated.
pub fn read_only(database_path: &Path) -> StoreResult<ReadOnlyDiagnosis> {
    let mut conn = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let schema_present = crate::meta::schema_exists(&conn)?;
    if !schema_present {
        return Ok(ReadOnlyDiagnosis {
            schema_present: false,
            schema_epoch: None,
            supported_schema_epoch: SUPPORTED_SCHEMA_EPOCH,
            daemon_epoch: DaemonEpoch::new(0),
            verified_through: None,
            ledger_head: None,
            open_obligations: Vec::new(),
            open_conditions: Vec::new(),
            committed_artifacts: 0,
        });
    }

    let schema_epoch = crate::meta::schema_epoch(&conn)?;
    // The epoch gate, exactly as `crate::migrate` applies it: a database from a
    // newer binary is described, never interpreted. Reading its tables with
    // this binary's expectations is what fails closed here.
    if schema_epoch.is_some_and(|found| found > SUPPORTED_SCHEMA_EPOCH) {
        return Ok(ReadOnlyDiagnosis {
            schema_present: true,
            schema_epoch,
            supported_schema_epoch: SUPPORTED_SCHEMA_EPOCH,
            daemon_epoch: DaemonEpoch::new(0),
            verified_through: None,
            ledger_head: None,
            open_obligations: Vec::new(),
            open_conditions: Vec::new(),
            committed_artifacts: 0,
        });
    }

    let daemon_epoch = crate::meta::daemon_epoch(&conn)?;
    let verified_through = crate::meta::last_verified_projection_seq(&conn)?;

    // Deferred, and on a read-only connection: this takes a shared lock and
    // cannot promote to a write one even by accident.
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let tx = Tx::new(&transaction, None, "read_only_diagnosis");
    let ledger_head = crate::event::highest_seq(&tx)?;
    let open_obligations = crate::load::open_obligations(&tx)?;
    let open_conditions = crate::load::open_conditions(&tx)?;
    let committed_artifacts = crate::load::committed_artifacts(&tx)?.len();

    Ok(ReadOnlyDiagnosis {
        schema_present: true,
        schema_epoch,
        supported_schema_epoch: SUPPORTED_SCHEMA_EPOCH,
        daemon_epoch,
        verified_through,
        ledger_head,
        open_obligations,
        open_conditions,
        committed_artifacts,
    })
}
