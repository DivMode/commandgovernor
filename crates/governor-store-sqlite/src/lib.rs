//! SQLite persistence authority for Command Governor.
//!
//! # Phase 1 role
//!
//! `governor-store-sqlite` is the single source of durable truth: the append-only
//! source/domain event log, the replayable projections built from it, and the
//! immutable result-artifact metadata that pins those artifacts while an
//! obligation is open. Event order is the daemon-assigned SQLite sequence, and a
//! projection mismatch on startup fails closed.
//!
//! # Boundary
//!
//! All writes go through one daemon-owned writer actor; there is never a second
//! independent writer. The database runs with WAL, foreign keys, a bounded busy
//! timeout, and `synchronous=FULL`, under explicit deterministic migrations with
//! a schema epoch/version check. There is no ORM, and no external I/O is
//! performed while a transaction is held.
//!
//! Result artifacts are published file-before-database: write an owner-private
//! temp file, sync it, atomically rename to its immutable key, sync the
//! containing directory, and only then commit the metadata, terminal event, and
//! `completed_unprocessed` obligation in a single transaction.
