//! A whole-database fingerprint, for "zero rows changed".
//!
//! Several acceptance requirements are stated as *nothing was mutated*:
//! OBL-003, OBL-004, OBL-009, GPT-008, SEC-004. Counting rows in the one table
//! a test happens to think about proves much less than it looks like — a
//! rejected ACK that advanced a version, stamped a deletion instant, or touched
//! a claim row would pass that check.
//!
//! So the comparison is the whole database: every table, every column, every
//! row, rendered and sorted. Driven by SQLite's own introspection rather than a
//! hand-written list, so a table added later is compared automatically instead
//! of being forgotten.

use std::collections::BTreeMap;

use rusqlite::Connection;
use rusqlite::types::{Value, ValueRef};

/// Every row of every table, rendered and sorted.
pub type LedgerDump = BTreeMap<String, Vec<String>>;

/// Tables whose contents are process bookkeeping rather than domain state.
///
/// `meta` holds the daemon epoch and the projection watermark, both of which
/// move on every open and on every replay verification. Comparing them across
/// a restart would report a difference that is not a mutation of any work.
pub const PROCESS_TABLES: &[&str] = &["meta"];

/// Fingerprints the whole database.
///
/// # Panics
///
/// Panics when the schema cannot be introspected or a row cannot be read.
#[must_use]
pub fn dump(conn: &Connection) -> LedgerDump {
    dump_excluding(conn, &[])
}

/// Fingerprints the whole database except the named tables.
///
/// # Panics
///
/// Panics when the schema cannot be introspected or a row cannot be read.
#[must_use]
pub fn dump_excluding(conn: &Connection, skip: &[&str]) -> LedgerDump {
    let mut out = LedgerDump::new();
    for table in table_names(conn) {
        if skip.contains(&table.as_str()) {
            continue;
        }
        let columns = columns(conn, &table);
        let projection = columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = conn
            .prepare(&format!("SELECT {projection} FROM \"{table}\""))
            .expect("preparing a table scan");
        let rows = statement
            .query_map([], |row| {
                let mut rendered = String::new();
                for (index, column) in columns.iter().enumerate() {
                    let value = match row.get_ref(index)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(value) => Value::Integer(value),
                        ValueRef::Real(value) => Value::Real(value),
                        ValueRef::Text(bytes) => {
                            Value::Text(String::from_utf8_lossy(bytes).into_owned())
                        }
                        ValueRef::Blob(bytes) => Value::Blob(bytes.to_vec()),
                    };
                    rendered.push_str(&format!("{column}={value:?};"));
                }
                Ok(rendered)
            })
            .expect("scanning a table");
        let mut rendered: Vec<String> = rows.map(|row| row.expect("a row")).collect();
        // Sorted rather than ordered by `rowid`: some tables are `WITHOUT
        // ROWID`, and a set comparison is what "the same rows" means anyway.
        rendered.sort();
        out.insert(table, rendered);
    }
    out
}

/// Fingerprints the durable domain state, ignoring per-process bookkeeping.
///
/// # Panics
///
/// As [`dump`].
#[must_use]
pub fn dump_domain(conn: &Connection) -> LedgerDump {
    dump_excluding(conn, PROCESS_TABLES)
}

/// Asserts that two fingerprints are identical, naming the first difference.
///
/// # Panics
///
/// Panics with the differing table, and the rows only one side holds, when the
/// two disagree.
pub fn assert_unchanged(before: &LedgerDump, after: &LedgerDump, context: &str) {
    if before == after {
        return;
    }
    for (table, rows) in before {
        let other = after.get(table);
        assert_eq!(
            Some(rows),
            other,
            "{context}: table `{table}` changed\n  before: {rows:#?}\n  after:  {other:#?}"
        );
    }
    for table in after.keys() {
        assert!(
            before.contains_key(table),
            "{context}: table `{table}` appeared"
        );
    }
    unreachable!("{context}: the dumps differ but every table matched");
}

/// Every table name in the live schema, in a stable order.
///
/// # Panics
///
/// Panics when `sqlite_schema` cannot be read.
#[must_use]
pub fn table_names(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_schema
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .expect("listing tables");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("iterating tables");
    rows.map(|row| row.expect("a table name")).collect()
}

/// Every column of one table, in declaration order.
///
/// # Panics
///
/// Panics when the table cannot be introspected.
#[must_use]
pub fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .expect("reading column info");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("iterating columns");
    rows.map(|row| row.expect("a column name")).collect()
}

/// Counts the rows in one table.
///
/// # Panics
///
/// Panics when the table does not exist.
#[must_use]
pub fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
        row.get(0)
    })
    .expect("counting rows")
}

/// Reads one optional scalar as text.
///
/// # Panics
///
/// Panics when the statement is malformed.
#[must_use]
pub fn scalar(conn: &Connection, sql: &str) -> Option<String> {
    conn.query_row(sql, [], |row| row.get::<_, Option<String>>(0))
        .unwrap_or(None)
}

/// Every `(table, column)` whose text holds `needle`, in a stable order.
///
/// # Panics
///
/// Panics when the schema cannot be introspected.
#[must_use]
pub fn columns_containing(conn: &Connection, needle: &str) -> Vec<(String, String)> {
    let mut hits = Vec::new();
    for table in table_names(conn) {
        for column in columns(conn, &table) {
            let found: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM \"{table}\"
                          WHERE CAST(\"{column}\" AS TEXT) LIKE '%' || ?1 || '%'"
                    ),
                    rusqlite::params![needle],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if found > 0 {
                hits.push((table.clone(), column));
            }
        }
    }
    hits.sort();
    hits
}
