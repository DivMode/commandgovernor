# V1 durable data model

This document specifies the initial SQLite authority and private result-artifact
boundary. The SQL is intentionally close to a future migration, but remains an
architecture schema until the Rust store implementation and migrations are
reviewed together.

## Principles

1. **Events first, projections second.** Normalized source/domain events are
   immutable; current-state tables are replayable materialized projections.
2. **One writer.** One daemon-owned `rusqlite` actor serializes state changes.
3. **No transcript database.** Prompt text, raw tool arguments, shell commands,
   cwd, transcript paths, terminal transcript, browser cookies/tokens, GitHub
   credentials, and arbitrary provider payloads are forbidden from the ledger.
4. **Fences are explicit data.** Session incarnation, turn, source event,
   obligation version, binding generation, wake revision, and foreman claim are
   represented directly.
5. **External I/O is outside SQLite transactions.** Transactions establish durable
   intent/ambiguity fences before I/O and persist evidence after it.
6. **The actual final result survives runtime death.** A bounded worker result
   lives in a separate owner-private immutable artifact store referenced by
   SQLite.

## SQLite policy

Initial connection policy:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
-- application sets a bounded busy timeout
```

Use bundled SQLite through `rusqlite`. Transactions that compare a projection and
mutate it acquire the write lock before the state read they depend on. No browser,
network, worker, GitHub, or runtime I/O occurs while a SQLite transaction is held.

Schema compatibility is a monotonic application epoch. A binary fails closed on
an unknown newer epoch.

## IDs and ordering

Generated public IDs should be opaque; UUIDv7 is appropriate where time-ordering
is useful. Deterministic delivery identities use a domain-separated cryptographic
hash. Correctness never depends on parsing an ID.

The daemon assigns the authoritative SQLite event sequence. Wall-clock timestamps
are useful evidence/diagnostics, not cross-process ordering authority.

## Meta and migrations

```sql
CREATE TABLE meta (
    key                 TEXT PRIMARY KEY,
    value               TEXT NOT NULL
);

CREATE TABLE schema_migrations (
    version             INTEGER PRIMARY KEY,
    name                TEXT NOT NULL,
    checksum            TEXT NOT NULL,
    applied_at_ms       INTEGER NOT NULL
);
```

Meta may contain schema epoch, database instance ID, and last verified projection
sequence. It must not contain credentials or browser session material.

## Immutable event ledger

```sql
CREATE TABLE events (
    seq                     INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id                TEXT NOT NULL UNIQUE,
    kind                    TEXT NOT NULL,
    schema_version          INTEGER NOT NULL,
    observed_at_ms          INTEGER NOT NULL,
    occurred_at_ms          INTEGER,

    project_id              TEXT,
    task_id                 TEXT,
    session_id              TEXT,
    session_incarnation_id  TEXT,
    turn_id                 TEXT,
    obligation_id           TEXT,

    source_namespace        TEXT NOT NULL,
    source_event_id         TEXT NOT NULL,
    source_event_fence      TEXT NOT NULL,

    -- Event-kind-specific, typed, redaction-safe metadata only.
    safe_metadata_json      TEXT NOT NULL DEFAULT '{}',

    UNIQUE(source_namespace, source_event_id, source_event_fence)
);
```

Every accepted event has a non-null source identity. A provider that lacks one
must derive it deterministically from stable **non-secret** facts such as Command
Governor turn ID + provider-native sequence/tool-use ID + event class. Internal
Command Governor events use their own generated source IDs. Never hash prompt,
transcript, tool arguments, or result content merely to manufacture an event
identity.

`safe_metadata_json` is not a generic provider dump. Each event kind has an
explicit serializer and allowed bounded fields; unknown fields are discarded.

## Projects and tasks

```sql
CREATE TABLE projects (
    project_id              TEXT PRIMARY KEY,
    source_host             TEXT NOT NULL,
    source_repo_id          TEXT,
    source_repo_display     TEXT,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq)
);

CREATE TABLE tasks (
    task_id                 TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES projects(project_id),
    source_issue_ref        TEXT,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq)
);
```

These are source-host references/provenance, not copies of repository content.

## Sessions and incarnations

```sql
CREATE TABLE sessions (
    session_id              TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES projects(project_id),
    runtime_kind            TEXT NOT NULL,
    worker_kind             TEXT NOT NULL,
    display_name            TEXT,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq)
);

CREATE TABLE session_incarnations (
    session_incarnation_id  TEXT PRIMARY KEY,
    session_id              TEXT NOT NULL REFERENCES sessions(session_id),
    generation              INTEGER NOT NULL,
    runtime_instance_ref    TEXT,
    worker_session_ref      TEXT,
    started_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    ended_event_seq         INTEGER REFERENCES events(seq),
    UNIQUE(session_id, generation)
);
```

A session name is display metadata. Continuity that cannot be proven after
runtime/process replacement creates a new incarnation. Delayed old-incarnation
events may be recorded for history but cannot mutate the current incarnation.

## Turns

```sql
CREATE TABLE turns (
    turn_id                 TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES tasks(task_id),
    session_incarnation_id  TEXT NOT NULL REFERENCES session_incarnations(session_incarnation_id),
    worker_turn_ref         TEXT,
    turn_generation         INTEGER NOT NULL,
    lifecycle_state         TEXT NOT NULL,
    started_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    last_progress_at_ms     INTEGER,
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    UNIQUE(session_incarnation_id, turn_generation)
);
```

`lifecycle_state` is a projection of accepted worker/runtime events. It is not an
uninterpreted copy of Herdr/Claude status text.

## Private result artifacts

```sql
CREATE TABLE result_artifacts (
    result_artifact_id      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES tasks(task_id),
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    storage_ref             TEXT NOT NULL UNIQUE,
    sha256_hex              TEXT NOT NULL,
    byte_len                INTEGER NOT NULL,
    media_type              TEXT NOT NULL,
    created_at_ms           INTEGER NOT NULL,
    retention_state         TEXT NOT NULL,
    eligible_for_delete_at_ms INTEGER
);
```

The daemon allocates `storage_ref`; workers never supply filesystem paths.
Artifact opens are rooted/no-follow as supported by the platform and verify digest
and length before MCP delivery.

### Crash-safe result publication

Before creating `completed_unprocessed`:

1. create owner-only temp file under the private artifact root;
2. write only the bounded final worker result required for review;
3. fsync/fdatasync file;
4. atomically rename to immutable store key;
5. sync containing directory where required;
6. in one SQLite transaction append/dedupe terminal event, insert artifact
   metadata, finalize turn projection, and create/update exactly one obligation;
7. only after commit may wake scheduling observe completion.

A crash before step 6 may leave an unreferenced orphan file, which is safe to
quarantine/GC after a grace period. The forbidden outcome is a committed open
obligation pointing at an artifact that was never made durable.

ACK only makes an artifact retention-eligible; asynchronous GC deletes later.
Every open obligation that references an artifact pins it.

## Obligations

```sql
CREATE TABLE obligations (
    obligation_id           TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES tasks(task_id),
    turn_id                 TEXT REFERENCES turns(turn_id),
    result_artifact_id      TEXT REFERENCES result_artifacts(result_artifact_id),
    obligation_kind         TEXT NOT NULL,
    state                   TEXT NOT NULL,
    priority                INTEGER NOT NULL,
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    current_version         INTEGER NOT NULL,
    current_binding_generation INTEGER,
    current_claim_id        TEXT,
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    closed_event_seq        INTEGER REFERENCES events(seq)
);

CREATE TABLE obligation_events (
    seq                     INTEGER PRIMARY KEY AUTOINCREMENT,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    obligation_version      INTEGER NOT NULL,
    event_seq               INTEGER NOT NULL UNIQUE REFERENCES events(seq),
    from_state              TEXT,
    to_state                TEXT NOT NULL,
    disposition             TEXT,
    actor_class             TEXT NOT NULL,
    binding_generation      INTEGER,
    claim_id                TEXT,
    UNIQUE(obligation_id, obligation_version)
);
```

`current_version` is compare-and-swap state. A terminal source event duplicate
hits the event-ledger unique fence and returns the existing result/obligation
rather than creating a second one.

## Foreman bindings

```sql
CREATE TABLE foreman_bindings (
    foreman_binding_id      TEXT PRIMARY KEY,
    provider                TEXT NOT NULL,
    canonical_conversation_id TEXT NOT NULL,
    canonical_conversation_url TEXT NOT NULL,
    browser_profile_id      TEXT NOT NULL,
    binding_generation      INTEGER NOT NULL UNIQUE,
    connector_abi           TEXT NOT NULL,
    capability_epoch        INTEGER NOT NULL,
    write_capability_state  TEXT NOT NULL,
    is_active               INTEGER NOT NULL CHECK(is_active IN (0, 1)),
    bound_event_seq         INTEGER NOT NULL REFERENCES events(seq),
    superseded_event_seq    INTEGER REFERENCES events(seq)
);

CREATE UNIQUE INDEX one_active_foreman_binding
    ON foreman_bindings(is_active)
    WHERE is_active = 1;
```

Rebind inserts a new generation and supersedes the old binding transactionally.
No cookie/token/local-storage value is stored here.

## Browser wake deliveries

A wake is targeted at the **exact obligation version/source fact that existed when
it was scheduled**.

```sql
CREATE TABLE browser_deliveries (
    delivery_id             TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    target_obligation_version INTEGER NOT NULL,
    target_source_event_seq INTEGER NOT NULL REFERENCES events(seq),
    foreman_binding_id      TEXT NOT NULL REFERENCES foreman_bindings(foreman_binding_id),
    binding_generation      INTEGER NOT NULL,
    delivery_revision       INTEGER NOT NULL,
    wake_protocol           TEXT NOT NULL,
    wake_payload_digest     TEXT NOT NULL,
    state                   TEXT NOT NULL,
    accepted_message_ref    TEXT,
    accepted_event_seq      INTEGER REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(obligation_id, binding_generation, delivery_revision)
);

CREATE TABLE delivery_attempts (
    delivery_attempt_id     TEXT PRIMARY KEY,
    delivery_id             TEXT NOT NULL REFERENCES browser_deliveries(delivery_id),
    attempt_no              INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    claimed_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    activation_armed_event_seq INTEGER REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    started_at_ms           INTEGER NOT NULL,
    finished_at_ms          INTEGER,
    failure_class           TEXT,
    evidence_class          TEXT,
    UNIQUE(delivery_id, attempt_no)
);
```

Immediately before any composer mutation and again before Send activation, the
adapter verifies the obligation is still open at
`target_obligation_version/target_source_event_seq` and binding generation is
current. If the obligation changed, this wake is stale and cannot Send.

The wake text is deterministic from opaque IDs. SQLite stores its digest, not
Claude output.

A delivery revision can have another attempt only after a prior attempt is
**proven failed before the Send ambiguity fence**. The aggregate delivery may
therefore transition `failed -> claimed` for a bounded safe retry. Once any attempt
is accepted or ambiguous, that revision is frozen forever. A later foreman resume
is a new `delivery_revision`.

On daemon startup, latest previous-process `claimed` or `activation_armed` without
a terminal result becomes `ambiguous` before browser recovery, even if the crash
probably happened before Send.

## Foreman physical turns

```sql
CREATE TABLE foreman_turns (
    foreman_turn_id         TEXT PRIMARY KEY,
    foreman_binding_id      TEXT NOT NULL REFERENCES foreman_bindings(foreman_binding_id),
    binding_generation      INTEGER NOT NULL,
    trigger_delivery_id     TEXT REFERENCES browser_deliveries(delivery_id),
    provider_turn_ref       TEXT,
    state                   TEXT NOT NULL,
    started_event_seq       INTEGER REFERENCES events(seq),
    settled_event_seq       INTEGER REFERENCES events(seq),
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq)
);
```

`settled` is observational. It has no FK/trigger or code path that closes an
obligation.

## Foreman claims

```sql
CREATE TABLE foreman_claims (
    claim_id                TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    obligation_version_at_claim INTEGER NOT NULL,
    binding_generation      INTEGER NOT NULL,
    wake_delivery_id        TEXT NOT NULL REFERENCES browser_deliveries(delivery_id),
    state                   TEXT NOT NULL,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    expires_at_ms           INTEGER NOT NULL,
    released_event_seq      INTEGER REFERENCES events(seq),
    closed_event_seq        INTEGER REFERENCES events(seq)
);
```

A claim is minted only from an accepted current-generation wake whose target
obligation version/source fact is still current. Claim expiry is an internal
coordination event: it may return an obligation to its prior attention state but
can never close it or release a required artifact.

## Input requests

Raw Claude/tool input arguments are **not** persisted.

```sql
CREATE TABLE input_requests (
    input_request_id        TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    native_input_ref        TEXT,
    request_kind            TEXT NOT NULL,
    authorization_class     TEXT NOT NULL,
    answer_shape            TEXT NOT NULL,
    request_revision        INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    answered_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(turn_id, source_event_seq, request_revision)
);
```

Safe durable fields describe **what kind of input is owed**, not the raw question
or tool arguments. When the foreman claims the request, the worker adapter obtains
current question/choice/permission detail ephemerally from the native Claude
session/provider if the current adapter supports it. If that detail cannot be
recovered after restart, the obligation remains `needs_input` and returns
`input_detail_unavailable`; Command Governor does not invent an answer.

`native_input_ref` is an opaque provider identity such as a documented tool-use
ID, never a transcript path or serialized arguments.

## Worker answer/resume delivery

Recording a foreman answer is not evidence the worker received it, so worker
continuations have their own durable delivery projection.

```sql
CREATE TABLE worker_commands (
    worker_command_id       TEXT PRIMARY KEY,
    input_request_id        TEXT REFERENCES input_requests(input_request_id),
    session_incarnation_id  TEXT NOT NULL REFERENCES session_incarnations(session_incarnation_id),
    answer_event_seq        INTEGER REFERENCES events(seq),
    command_kind            TEXT NOT NULL,
    command_revision        INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(input_request_id, command_revision)
);

CREATE TABLE worker_command_attempts (
    worker_command_attempt_id TEXT PRIMARY KEY,
    worker_command_id       TEXT NOT NULL REFERENCES worker_commands(worker_command_id),
    attempt_no              INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    claimed_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    ambiguity_armed_event_seq INTEGER,
    terminal_event_seq      INTEGER,
    UNIQUE(worker_command_id, attempt_no)
);
```

The answer itself is a bounded structured Command Governor event/artifact only to
the extent required by the explicit input protocol; raw original tool arguments
are never copied. Once worker resume/write may have happened, ambiguity forbids
blind replay. Matching native resumed-turn evidence reconciles the command and
moves the obligation back to `running`.

## Progress heartbeats

```sql
CREATE TABLE progress_heartbeats (
    progress_id             TEXT PRIMARY KEY,
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL UNIQUE REFERENCES events(seq),
    occurred_at_ms          INTEGER NOT NULL,
    safe_event_class        TEXT NOT NULL
);

CREATE INDEX progress_by_turn_time
    ON progress_heartbeats(turn_id, occurred_at_ms DESC);
```

Only identity/time/safe class is required. No tool args/results are stored. Very
chatty equivalent progress may be deterministically coalesced while retaining
enough evidence for the stall threshold.

## Health/reconciliation conditions

```sql
CREATE TABLE health_conditions (
    health_condition_id     TEXT PRIMARY KEY,
    kind                    TEXT NOT NULL,
    state                   TEXT NOT NULL,
    task_id                 TEXT,
    turn_id                 TEXT,
    obligation_id           TEXT,
    opened_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    resolved_event_seq      INTEGER REFERENCES events(seq)
);
```

Initial kinds include:

- `suspected_stall`
- `foreman_unreachable`
- `mcp_write_capability_missing`
- `browser_binding_displaced`
- `result_artifact_missing`
- `projection_mismatch`
- `runtime_state_conflict`
- `input_detail_unavailable`

A health condition never pretends to be worker completion.

## Critical transaction boundaries

### Worker terminal publication

One DB transaction after the artifact is durable:

- insert/dedupe terminal event;
- finalize `turns` projection;
- insert/reference result artifact;
- create/transition exactly one obligation to `completed_unprocessed` (or
  `failed` for authoritative failure);
- append obligation transition event.

### Create/claim browser delivery

One DB transaction:

- verify current binding;
- verify exact obligation version/source event is still current;
- insert/find deterministic delivery revision with target snapshot;
- insert attempt `claimed`;
- append event;
- commit **before any browser I/O**.

### Arm browser Send

Immediately before exact Send action, one DB transaction re-verifies binding and
target obligation snapshot, then commits `activation_armed`. A crash after commit
is recovered as ambiguous.

### `foreman_resume`

One transaction verifies accepted wake + target snapshot + current generation,
creates one current claim, updates obligation projection/version, and appends the
claim event. Artifact reading happens after the transaction; inability to read it
does not close the claim/obligation.

### ACK

One transaction verifies obligation version, source event, binding generation,
claim, and disposition; appends explicit disposition event; closes projection; and
marks pinned artifacts retention-eligible only as policy permits. No external I/O
occurs inside ACK.

## Forbidden-persistence fixture

Tests inject unique sentinels into:

- cwd;
- prompt;
- raw tool arguments;
- shell command;
- transcript path;
- terminal transcript;
- browser cookies/tokens/headers/bodies;
- GitHub auth.

After lifecycle/browser/MCP/crash scenarios, byte-scan SQLite DB/WAL/SHM, hook
inbox, structured logs, safe diagnostics, crash state, and configuration. Expected:
**zero sentinel matches** outside an explicitly designated sensitive result
artifact when the test intentionally places final worker result content there.
