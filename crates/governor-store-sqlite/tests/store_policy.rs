//! Connection policy, migrations and the schema-epoch gate.
//!
//! | Test | Requirement |
//! | --- | --- |
//! | [`the_required_pragmas_are_actually_in_force`] | `docs/data-model.md` "SQLite policy" |
//! | [`a_fresh_database_migrates_and_reopens_unchanged`] | deterministic migrations |
//! | [`a_drifted_migration_definition_fails_closed`] | checksum contract |
//! | [`an_unknown_newer_schema_epoch_fails_closed`] | `docs/testing.md` DB-003 |
//! | [`an_interrupted_migration_rolls_back_and_reapplies`] | DB-004 basics |
//! | [`the_daemon_epoch_advances_once_per_open`] | epoch fencing |

mod support;

use governor_store_sqlite::{Failpoint, StoreError};
use rusqlite::params;
use support::{Harness, count};

#[test]
fn the_required_pragmas_are_actually_in_force() {
    let harness = Harness::new();
    let store = harness.open().expect("opening a fresh state root");
    let policy = &store.startup().policy;

    // Read back from the engine, not merely issued: `journal_mode` silently
    // refuses to change in conditions the application would never notice.
    assert!(policy.foreign_keys, "foreign keys must be enforced");
    assert!(
        policy.journal_mode.eq_ignore_ascii_case("wal"),
        "journal mode is {}",
        policy.journal_mode
    );
    assert_eq!(policy.synchronous, 2, "synchronous must be FULL");
    assert!(
        policy.busy_timeout_ms > 0,
        "the busy timeout must be bounded, not infinite"
    );

    // And the engine agrees on a second, independent connection.
    let conn = harness.inspect();
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("reading journal mode back");
    assert!(journal.eq_ignore_ascii_case("wal"));
}

#[test]
fn foreign_keys_actually_reject_a_dangling_reference() {
    // The pragma being *on* is only interesting if the engine acts on it.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let _turn = support::open_turn(&store);
    drop(store);

    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enabling foreign keys");
    let refused = conn.execute(
        "INSERT INTO tasks (task_id, project_id, created_event_seq, latest_event_seq)
         VALUES ('00000000-0000-0000-0000-0000000000ff', 'no-such-project', 1, 1)",
        [],
    );
    assert!(
        refused.is_err(),
        "a task pointing at no project must be refused"
    );
}

#[test]
fn a_fresh_database_migrates_and_reopens_unchanged() {
    let harness = Harness::new();
    let store = harness.open().expect("first open");
    assert_eq!(store.startup().migrations.applied, vec![1]);
    assert!(store.startup().migrations.verified.is_empty());
    assert_eq!(store.startup().migrations.epoch, 1);
    let instance = store.startup().migrations.database_instance_id;
    drop(store);

    let store = harness.open().expect("second open");
    assert!(
        store.startup().migrations.applied.is_empty(),
        "a migrated database applies nothing on reopen"
    );
    assert_eq!(store.startup().migrations.verified, vec![1]);
    assert_eq!(
        store.startup().migrations.database_instance_id,
        instance,
        "the database keeps the identity it was created with"
    );
}

#[test]
fn a_drifted_migration_definition_fails_closed() {
    let harness = Harness::new();
    drop(harness.open().expect("first open"));

    // Simulate a database built by a different definition of version 1.
    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute(
        "UPDATE schema_migrations SET checksum = 'deadbeef' WHERE version = 1",
        [],
    )
    .expect("rewriting the recorded checksum");
    drop(conn);

    let error = harness.open().expect_err("a drifted migration is refused");
    assert!(matches!(
        error,
        StoreError::MigrationChecksumMismatch { version: 1, .. }
    ));
    assert!(error.is_fail_closed());
}

#[test]
fn an_unknown_newer_schema_epoch_fails_closed() {
    let harness = Harness::new();
    drop(harness.open().expect("first open"));

    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'schema_epoch'",
        params!["99"],
    )
    .expect("recording a newer epoch");
    let events_before = count(&conn, "events");
    drop(conn);

    let error = harness
        .open()
        .expect_err("an older binary must refuse a newer database");
    match error {
        StoreError::SchemaEpochTooNew { found, supported } => {
            assert_eq!(found, 99);
            assert_eq!(supported, governor_store_sqlite::SUPPORTED_SCHEMA_EPOCH);
        }
        other => panic!("expected a fail-closed epoch gate, got {other:?}"),
    }

    // No downgrade, and no mutation of any kind: the gate runs before the
    // store reads or writes anything else.
    let conn = harness.inspect();
    assert_eq!(count(&conn, "events"), events_before);
    let epoch: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_epoch'",
            [],
            |r| r.get(0),
        )
        .expect("epoch still readable");
    assert_eq!(epoch, "99", "the refused open must not rewrite the epoch");
}

#[test]
fn an_interrupted_migration_rolls_back_and_reapplies() {
    let harness = Harness::new();
    let hook = support::FireOnce::new("migrate", Failpoint::BeforeMigrationRecorded);
    let error = harness
        .open_with(Some(Box::new(hook)))
        .expect_err("the injected crash aborts the migration");
    assert!(matches!(error, StoreError::Sqlite(_)));

    // DDL is transactional in SQLite, so the whole migration rolled back and
    // the next open starts from a known state rather than a half-built schema.
    let store = harness.open().expect("reopening after the interruption");
    assert_eq!(
        store.startup().migrations.applied,
        vec![1],
        "the migration is applied cleanly on the next attempt"
    );
    assert!(store.startup().migrations.verified.is_empty());
}

#[test]
fn the_daemon_epoch_advances_once_per_open() {
    let harness = Harness::new();
    let first = harness.open().expect("first open").daemon_epoch();
    assert_eq!(first.get(), 1);

    let second = harness.open().expect("second open").daemon_epoch();
    assert_eq!(second.get(), 2, "each process lifetime gets its own epoch");

    let third = harness.open().expect("third open");
    assert_eq!(third.daemon_epoch().get(), 3);
    assert!(third.startup().previously_verified_through.is_none());
}
