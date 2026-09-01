//! Forbidden persistence, and the absence of ambient I/O.
//!
//! `docs/data-model.md` "Forbidden-persistence fixture" and research test 12:
//! the control ledger must never hold a prompt, raw tool arguments or results,
//! a shell command, a cwd, a transcript path, a terminal transcript, browser
//! cookies/tokens/headers/bodies, or credentials.
//!
//! This is the store-side smoke test. The exhaustive lifecycle fixture — hook
//! inbox, managed-run staging, structured logs, crash state — arrives with the
//! testkit; what is proven here is the database half, in two ways:
//!
//! 1. **Nothing forbidden can be constructed.** Every value the store accepts
//!    is a [`SafeToken`], a closed enum, an opaque identity or a bounded
//!    integer. The charset refuses whitespace, quotes and path separators, so
//!    the forbidden shapes fail before any I/O happens.
//! 2. **Nothing leaks sideways.** A sentinel placed in one legitimate opaque
//!    field is searched for in *every text column of every table*, and must
//!    appear only where it was put.

mod support;

use governor_core::fence::SafeToken;
use rusqlite::Connection;
use support::{
    Harness, accept_wake, bind, open_turn, publish_result, schedule_wake, source, start_worker,
};

/// Forbidden content the [`SafeToken`] charset itself refuses.
///
/// Prose, paths, shell commands, JSON documents and transcripts all contain a
/// space, a quote, a newline or a `/`, so they cannot be built at all — the
/// refusal happens before any store API is reached.
const UNREPRESENTABLE: &[(&str, &str)] = &[
    ("cwd", "/Volumes/Data/Developer/commandgovernor"),
    ("prompt", "please review the diff and ACK"),
    ("raw tool arguments", r#"{"command":"ls -la","cwd":"/tmp"}"#),
    ("raw tool result", "total 48\ndrwxr-xr-x 12 peter staff"),
    ("shell command", "rm -rf /tmp/cg-state"),
    ("transcript path", "/Users/peter/.claude/transcript.jsonl"),
    ("terminal transcript", "$ cargo test\n   Compiling ..."),
    (
        "provider stream record",
        r#"{"type":"tool_use","id":"toolu_01"}"#,
    ),
    ("authorization header", "Bearer sk-proj-0000000000"),
];

/// Forbidden content that is *token-shaped*, and rests on a different rule.
///
/// A cookie value and an opaque provider identity are indistinguishable as
/// strings, so no charset can separate them and [`SafeToken`] does not try.
/// What protects these is the schema: there is nowhere to put them. That is
/// what [`the_schema_has_no_column_for_forbidden_content`] locks down, and what
/// [`a_full_lifecycle_leaves_no_forbidden_bytes_in_the_database`] scans for.
const NO_COLUMN_FOR: &[(&str, &str)] = &[
    ("browser cookie", "__Secure-next-auth.session-token=abc.def"),
    ("provider api key", "sk-proj-0000000000"),
    (
        "github credential",
        "ghp_0000000000000000000000000000000000",
    ),
];

#[test]
fn prose_paths_and_documents_are_refused_before_any_io() {
    // `SafeToken` is the only string-shaped value any public store request
    // carries, so refusing these here means no store API can accept them. A
    // future field taking a bare `String` would have to be added to this list,
    // and would fail it.
    for (label, value) in UNREPRESENTABLE {
        assert!(
            SafeToken::new(value).is_err(),
            "{label} must be unrepresentable, but {value:?} was accepted"
        );
    }

    // And the honest converse: a token-shaped secret *is* representable. It is
    // kept out by having no column, not by the charset, and saying otherwise
    // would be claiming a guarantee this crate does not have.
    for (label, value) in NO_COLUMN_FOR {
        assert!(
            SafeToken::new(value).is_ok(),
            "{label} is token-shaped; if that changes, move it to the other list"
        );
    }
}

#[test]
fn a_full_lifecycle_leaves_no_forbidden_bytes_in_the_database() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    accept_wake(&store, &claimed, generation, "msg-1");
    store
        .cancel_obligation(governor_store_sqlite::CancelObligationRequest {
            obligation: turn.obligation,
            source: source("cg.cli", "cancel-1", "user"),
        })
        .expect("closing the work");
    drop(store);

    // Database, write-ahead log and shared-memory index together: a value that
    // was written and later overwritten still leaves its bytes in the WAL, so
    // scanning only the main file would be too weak.
    let bytes = harness.raw_bytes();
    assert!(!bytes.is_empty(), "the scan must have something to scan");
    for (label, value) in UNREPRESENTABLE.iter().chain(NO_COLUMN_FOR) {
        assert!(
            !contains(&bytes, value.as_bytes()),
            "{label} was found in the durable state root"
        );
    }
}

#[test]
fn safe_metadata_never_holds_a_provider_shaped_document() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    drop(store);

    let conn = harness.inspect();
    let mut statement = conn
        .prepare("SELECT kind, safe_metadata_json FROM events")
        .expect("reading the ledger");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("iterating the ledger");

    for row in rows {
        let (kind, document) = row.expect("a ledger row");
        // The writer emits exactly one shape: a flat object of strings,
        // integers and booleans. Nesting, arrays, floats and nulls are how a
        // raw provider record arrives, and none of them can be written.
        assert!(
            document.starts_with('{') && document.ends_with('}'),
            "{kind}"
        );
        let inner = &document[1..document.len() - 1];
        for forbidden in ['{', '[', ']'] {
            assert!(
                !inner.contains(forbidden),
                "{kind} metadata contains a nested structure: {document}"
            );
        }
        assert!(!inner.contains("null"), "{kind} metadata contains a null");
    }
}

#[test]
fn a_sentinel_appears_only_in_the_column_it_was_written_to() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");

    // Three sentinels, each pushed through a *legitimate* bounded opaque field.
    // The question is not whether they are stored — they must be — but whether
    // anything copies them somewhere else.
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    store
        .publish_worker_result(governor_store_sqlite::PublishWorkerResultRequest {
            obligation: turn.obligation,
            source: source("claude.result", "run-1", "final"),
            incarnation: governor_core::fence::IncarnationGeneration::FIRST,
            receipts: support::completion_receipts("CGSENTINELRUN"),
            artifact: support::durable_artifact("CGSENTINELSTORAGE"),
        })
        .expect("publication");

    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    accept_wake(&store, &claimed, generation, "CGSENTINELMESSAGE");
    drop(store);

    let conn = harness.inspect();

    // `run_ref` is allowlisted safe metadata on the terminal event, and the
    // artifact digest input — nothing else.
    assert_eq!(
        columns_containing(&conn, "CGSENTINELRUN"),
        vec![("events".to_owned(), "safe_metadata_json".to_owned())]
    );

    // The daemon-allocated storage key belongs to the artifact row alone. A
    // worker never supplies a path, and nothing mirrors the key elsewhere.
    assert_eq!(
        columns_containing(&conn, "CGSENTINELSTORAGE"),
        vec![("result_artifacts".to_owned(), "storage_ref".to_owned())]
    );

    // The provider message identity is acceptance evidence: the delivery row
    // and the accepting event, and no third place.
    assert_eq!(
        columns_containing(&conn, "CGSENTINELMESSAGE"),
        vec![
            (
                "browser_deliveries".to_owned(),
                "accepted_message_ref".to_owned()
            ),
            ("events".to_owned(), "safe_metadata_json".to_owned()),
        ]
    );
}

#[test]
fn the_schema_has_no_column_for_forbidden_content() {
    // A lock, not a description. The privacy contract is ultimately "there is
    // nowhere to put it", and that claim is only as good as the column list —
    // so the list is pinned here. Adding a column makes this fail, which is the
    // point: every new place a value could live gets one deliberate review.
    //
    // Regenerate with care, and check the new name against
    // `docs/data-model.md` principle 3 before accepting it.
    const SCHEMA: &[(&str, &str)] = &[
        ("browser_deliveries", "accepted_event_seq"),
        ("browser_deliveries", "accepted_message_ref"),
        ("browser_deliveries", "attempt_budget"),
        ("browser_deliveries", "binding_generation"),
        ("browser_deliveries", "delivery_id"),
        ("browser_deliveries", "delivery_key"),
        ("browser_deliveries", "delivery_revision"),
        ("browser_deliveries", "foreman_binding_id"),
        ("browser_deliveries", "obligation_id"),
        ("browser_deliveries", "state"),
        ("browser_deliveries", "target_obligation_version"),
        ("browser_deliveries", "target_source_event_seq"),
        ("browser_deliveries", "terminal_event_seq"),
        ("browser_deliveries", "wake_payload_digest"),
        ("browser_deliveries", "wake_protocol"),
        ("delivery_attempts", "activation_armed_event_seq"),
        ("delivery_attempts", "attempt_no"),
        ("delivery_attempts", "claimed_event_seq"),
        ("delivery_attempts", "delivery_attempt_id"),
        ("delivery_attempts", "delivery_id"),
        ("delivery_attempts", "evidence_class"),
        ("delivery_attempts", "failure_class"),
        ("delivery_attempts", "finished_at_ms"),
        ("delivery_attempts", "started_at_ms"),
        ("delivery_attempts", "state"),
        ("delivery_attempts", "terminal_event_seq"),
        ("events", "event_id"),
        ("events", "kind"),
        ("events", "obligation_id"),
        ("events", "observed_at_ms"),
        ("events", "occurred_at_ms"),
        ("events", "project_id"),
        ("events", "safe_metadata_json"),
        ("events", "schema_version"),
        ("events", "seq"),
        ("events", "session_id"),
        ("events", "session_incarnation_id"),
        ("events", "source_event_fence"),
        ("events", "source_event_id"),
        ("events", "source_namespace"),
        ("events", "task_id"),
        ("events", "turn_id"),
        ("external_attempts", "ambiguity_reason"),
        ("external_attempts", "completion_ref"),
        ("external_attempts", "daemon_epoch"),
        ("external_attempts", "destination_endpoint"),
        ("external_attempts", "destination_fence"),
        ("external_attempts", "destination_namespace"),
        ("external_attempts", "dispatched"),
        ("external_attempts", "dispatched_at_ms"),
        ("external_attempts", "effect_class"),
        ("external_attempts", "external_attempt_id"),
        ("external_attempts", "finished_at_ms"),
        ("external_attempts", "idempotency_contract"),
        ("external_attempts", "idempotency_key"),
        ("external_attempts", "idempotency_window_ms"),
        ("external_attempts", "no_effect_class"),
        ("external_attempts", "recorded_at_ms"),
        ("external_attempts", "source_event_fence"),
        ("external_attempts", "source_event_id"),
        ("external_attempts", "source_namespace"),
        ("external_attempts", "state"),
        ("foreman_bindings", "binding_generation"),
        ("foreman_bindings", "bound_event_seq"),
        ("foreman_bindings", "browser_profile_id"),
        ("foreman_bindings", "canonical_conversation_id"),
        ("foreman_bindings", "canonical_conversation_url"),
        ("foreman_bindings", "capability_epoch"),
        ("foreman_bindings", "connector_abi"),
        ("foreman_bindings", "foreman_binding_id"),
        ("foreman_bindings", "is_active"),
        ("foreman_bindings", "provider"),
        ("foreman_bindings", "superseded_event_seq"),
        ("foreman_bindings", "write_capability_state"),
        ("foreman_claims", "binding_generation"),
        ("foreman_claims", "claim_id"),
        ("foreman_claims", "closed_event_seq"),
        ("foreman_claims", "created_event_seq"),
        ("foreman_claims", "expires_at_ms"),
        ("foreman_claims", "obligation_id"),
        ("foreman_claims", "obligation_version_at_claim"),
        ("foreman_claims", "released_event_seq"),
        ("foreman_claims", "state"),
        ("foreman_claims", "wake_delivery_id"),
        ("foreman_turns", "binding_generation"),
        ("foreman_turns", "foreman_binding_id"),
        ("foreman_turns", "foreman_turn_id"),
        ("foreman_turns", "latest_event_seq"),
        ("foreman_turns", "provider_turn_ref"),
        ("foreman_turns", "settled_event_seq"),
        ("foreman_turns", "started_event_seq"),
        ("foreman_turns", "state"),
        ("foreman_turns", "trigger_delivery_id"),
        ("health_conditions", "external_attempt_id"),
        ("health_conditions", "health_condition_id"),
        ("health_conditions", "kind"),
        ("health_conditions", "obligation_id"),
        ("health_conditions", "opened_event_seq"),
        ("health_conditions", "resolved_event_seq"),
        ("health_conditions", "state"),
        ("health_conditions", "task_id"),
        ("health_conditions", "turn_id"),
        ("input_requests", "answer_shape"),
        ("input_requests", "answered_event_seq"),
        ("input_requests", "authorization_class"),
        ("input_requests", "input_request_id"),
        ("input_requests", "native_input_ref"),
        ("input_requests", "obligation_id"),
        ("input_requests", "request_kind"),
        ("input_requests", "request_revision"),
        ("input_requests", "source_event_seq"),
        ("input_requests", "state"),
        ("input_requests", "turn_id"),
        ("meta", "key"),
        ("meta", "value"),
        ("mutation_commands", "acked_at_ms"),
        ("mutation_commands", "actor_id"),
        ("mutation_commands", "command_id"),
        ("mutation_commands", "command_kind"),
        ("mutation_commands", "completed_at_ms"),
        ("mutation_commands", "created_at_ms"),
        ("mutation_commands", "daemon_epoch"),
        ("mutation_commands", "fingerprint"),
        ("mutation_commands", "safe_result_conflict"),
        ("mutation_commands", "safe_result_kind"),
        ("mutation_commands", "safe_result_ref"),
        ("mutation_commands", "status"),
        ("mutation_commands", "uncertain_at_ms"),
        ("obligation_events", "actor_class"),
        ("obligation_events", "binding_generation"),
        ("obligation_events", "claim_id"),
        ("obligation_events", "disposition"),
        ("obligation_events", "event_seq"),
        ("obligation_events", "from_state"),
        ("obligation_events", "obligation_id"),
        ("obligation_events", "obligation_version"),
        ("obligation_events", "seq"),
        ("obligation_events", "to_state"),
        ("obligations", "closed_event_seq"),
        ("obligations", "created_event_seq"),
        ("obligations", "current_binding_generation"),
        ("obligations", "current_claim_id"),
        ("obligations", "current_version"),
        ("obligations", "incarnation_generation"),
        ("obligations", "input_request_id"),
        ("obligations", "latest_event_seq"),
        ("obligations", "obligation_id"),
        ("obligations", "obligation_kind"),
        ("obligations", "priority"),
        ("obligations", "result_artifact_id"),
        ("obligations", "source_event_seq"),
        ("obligations", "state"),
        ("obligations", "task_id"),
        ("obligations", "turn_id"),
        ("progress_heartbeats", "occurred_at_ms"),
        ("progress_heartbeats", "progress_id"),
        ("progress_heartbeats", "safe_event_class"),
        ("progress_heartbeats", "source_event_seq"),
        ("progress_heartbeats", "turn_id"),
        ("projects", "created_event_seq"),
        ("projects", "project_id"),
        ("projects", "source_host"),
        ("projects", "source_repo_display"),
        ("projects", "source_repo_id"),
        ("resource_leases", "acquired_at_ms"),
        ("resource_leases", "daemon_epoch"),
        ("resource_leases", "expires_at_ms"),
        ("resource_leases", "holder_actor_id"),
        ("resource_leases", "lease_token"),
        ("resource_leases", "process_slot"),
        ("resource_leases", "process_start_ref"),
        ("resource_leases", "released_at_ms"),
        ("resource_leases", "renewed_at_ms"),
        ("resource_leases", "resource_digest"),
        ("resource_leases", "resource_lease_id"),
        ("resource_leases", "resource_namespace"),
        ("resource_leases", "state"),
        ("result_artifacts", "byte_len"),
        ("result_artifacts", "created_at_ms"),
        ("result_artifacts", "eligible_for_delete_at_ms"),
        ("result_artifacts", "media_type"),
        ("result_artifacts", "result_artifact_id"),
        ("result_artifacts", "retention_state"),
        ("result_artifacts", "sha256_hex"),
        ("result_artifacts", "source_event_seq"),
        ("result_artifacts", "storage_ref"),
        ("result_artifacts", "task_id"),
        ("result_artifacts", "turn_id"),
        ("schema_migrations", "applied_at_ms"),
        ("schema_migrations", "checksum"),
        ("schema_migrations", "name"),
        ("schema_migrations", "version"),
        ("session_incarnations", "ended_event_seq"),
        ("session_incarnations", "generation"),
        ("session_incarnations", "runtime_instance_ref"),
        ("session_incarnations", "session_id"),
        ("session_incarnations", "session_incarnation_id"),
        ("session_incarnations", "started_event_seq"),
        ("session_incarnations", "worker_session_ref"),
        ("sessions", "created_event_seq"),
        ("sessions", "display_name"),
        ("sessions", "project_id"),
        ("sessions", "runtime_kind"),
        ("sessions", "session_id"),
        ("sessions", "worker_kind"),
        ("tasks", "created_event_seq"),
        ("tasks", "latest_event_seq"),
        ("tasks", "project_id"),
        ("tasks", "source_issue_ref"),
        ("tasks", "task_id"),
        ("turns", "last_progress_at_ms"),
        ("turns", "latest_event_seq"),
        ("turns", "lifecycle_state"),
        ("turns", "session_incarnation_id"),
        ("turns", "started_event_seq"),
        ("turns", "task_id"),
        ("turns", "terminal_event_seq"),
        ("turns", "turn_generation"),
        ("turns", "turn_id"),
        ("turns", "worker_turn_ref"),
        ("worker_command_attempts", "ambiguity_armed_event_seq"),
        ("worker_command_attempts", "attempt_no"),
        ("worker_command_attempts", "claimed_event_seq"),
        ("worker_command_attempts", "evidence_class"),
        ("worker_command_attempts", "failure_class"),
        ("worker_command_attempts", "finished_at_ms"),
        ("worker_command_attempts", "started_at_ms"),
        ("worker_command_attempts", "state"),
        ("worker_command_attempts", "terminal_event_seq"),
        ("worker_command_attempts", "worker_command_attempt_id"),
        ("worker_command_attempts", "worker_command_id"),
        ("worker_commands", "answer_event_seq"),
        ("worker_commands", "attempt_budget"),
        ("worker_commands", "command_kind"),
        ("worker_commands", "command_revision"),
        ("worker_commands", "created_event_seq"),
        ("worker_commands", "input_request_id"),
        ("worker_commands", "session_incarnation_id"),
        ("worker_commands", "state"),
        ("worker_commands", "terminal_event_seq"),
        ("worker_commands", "worker_command_id"),
    ];

    let harness = Harness::new();
    let _store = harness.open().expect("opening");
    let conn = harness.inspect();
    assert_eq!(
        all_columns(&conn),
        SCHEMA
            .iter()
            .map(|(table, column)| ((*table).to_owned(), (*column).to_owned()))
            .collect::<Vec<_>>(),
        "the schema gained or lost a column"
    );

    // And no column is named for something the data model forbids outright.
    for (table, column) in all_columns(&conn) {
        for forbidden in [
            "prompt",
            "transcript",
            "cookie",
            "token_value",
            "credential",
            "secret",
            "password",
            "header",
            "cwd",
            "argv",
            "stdout",
            "stderr",
            "payload_body",
            "tool_input",
            "tool_result",
        ] {
            assert!(
                !column.contains(forbidden),
                "{table}.{column} names forbidden content"
            );
        }
    }
}

/// Every `(table, column)` the live schema holds, in a stable order.
fn all_columns(conn: &Connection) -> Vec<(String, String)> {
    let mut tables = Vec::new();
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_schema
              WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .expect("listing tables");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("iterating tables");
    for row in rows {
        tables.push(row.expect("a table name"));
    }

    let mut out = Vec::new();
    for table in tables {
        let mut info = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("reading column info");
        let rows = info
            .query_map([], |row| row.get::<_, String>(1))
            .expect("iterating columns");
        for row in rows {
            out.push((table.clone(), row.expect("a column name")));
        }
    }
    out.sort();
    out
}

/// Every `(table, column)` whose text holds `needle`, in a stable order.
///
/// Driven by SQLite's own introspection rather than a hand-written list, so a
/// column added later is searched automatically instead of being forgotten.
fn columns_containing(conn: &Connection, needle: &str) -> Vec<(String, String)> {
    let mut tables = Vec::new();
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .expect("listing tables");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("iterating tables");
    for row in rows {
        tables.push(row.expect("a table name"));
    }

    let mut hits = Vec::new();
    for table in tables {
        let mut columns = Vec::new();
        let mut info = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("reading column info");
        let rows = info
            .query_map([], |row| row.get::<_, String>(1))
            .expect("iterating columns");
        for row in rows {
            columns.push(row.expect("a column name"));
        }
        for column in columns {
            let found: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM {table}
                          WHERE CAST({column} AS TEXT) LIKE '%' || ?1 || '%'"
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn the_store_reaches_no_ambient_capability() {
    // The rule this guards is `crate::ports`: everything ambient goes through
    // `StorePorts`, which a transaction body cannot see. A direct call to the
    // filesystem, the network, a process or the system clock would make that a
    // convention again, so the source is scanned for one.
    //
    // `rusqlite` owns the only file handle in the crate, and it is a
    // dependency, not a call site here.
    const FORBIDDEN_CALLS: &[&str] = &[
        "std::fs",
        "std::net",
        "std::process",
        "SystemTime",
        "Instant::now",
        "thread::sleep",
        "getrandom",
        "rand::",
        "reqwest",
        "tokio",
    ];

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offences = Vec::new();
    visit(&root, &mut |path, text| {
        for needle in FORBIDDEN_CALLS {
            if text.contains(needle) {
                offences.push(format!("{} mentions {needle}", path.display()));
            }
        }
    });
    assert!(
        offences.is_empty(),
        "the store must reach nothing ambient: {offences:#?}"
    );
}

fn visit(dir: &std::path::Path, seen: &mut impl FnMut(&std::path::Path, &str)) {
    let entries = std::fs::read_dir(dir).expect("reading the source tree");
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            visit(&path, seen);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("reading a source file");
            seen(&path, &text);
        }
    }
}
