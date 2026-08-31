# V1 durable data model

This document specifies the initial SQLite authority and private result-artifact
boundary. Names are intentionally close to a future migration, but this is an
architecture schema, not yet committed application SQL.

## Principles

1. **Events first, projections second.** Normalized source/domain events are
   immutable. Projection rows make current-state queries cheap but must be
   replayable/validatable.
2. **One writer.** The daemon owns one `rusqlite` DB actor. Async components send
   typed commands to it; they do not share a pool of arbitrary write connections.
3. **No sensitive transcript database.** Prompts, cwd, transcript paths, tool
   arguments, shell commands, browser cookies/tokens, GitHub credentials, and
   terminal transcript text are forbidden from event payloads and tracing.
4. **Fences are data.** Session incarnation, turn, source event, obligation,
   binding generation, and delivery revision are explicit columns/foreign keys.
5. **External side effects never hide inside a DB transaction.** Transactions
   durably establish intent/fence state before external I/O; later evidence closes
   the attempt.
6. **Result content survives runtime death without polluting the ledger.** The
   final bounded result required by an open obligation lives in an owner-private
   artifact store referenced from SQLite.

## SQLite runtime policy

Initial settings:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
-- set a bounded application busy timeout
```

Use bundled SQLite via `rusqlite` so the application controls the SQLite version
and features across supported platforms. The DB actor serializes state-changing
commands. Transactions that perform compare-and-swap/uniqueness fencing should
acquire the write lock before reading the state they are about to mutate.

The schema has a monotonic application schema epoch. A binary refuses to run
against an unknown newer epoch. Migrations are transactional when SQLite permits;
filesystem migrations use explicit staged markers and crash tests.

## Identifier representation

Externally visible IDs should be opaque. UUIDv7 is appropriate for generated
entities that benefit from time ordering; deterministic delivery IDs are derived
from a domain-separated cryptographic hash. The database must not depend on the
lexical structure of an ID for correctness.

Timestamps are daemon-assigned UTC instants plus monotonic process timing where
needed for watchdog math. Cross-process ordering comes from SQLite event sequence,
not wall-clock comparison.

## Core event ledger

```sql
CREATE TABLE events (
    seq                 INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id            TEXT NOT NULL UNIQUE,
    kind                TEXT NOT NULL,
    schema_version      INTEGER NOT NULL,
    observed_at_ms      INTEGER NOT NULL,
    occurred_at_ms      INTEGER,

    project_id          TEXT,
    task_id             TEXT,
    session_id          TEXT,
    session_incarnation_id TEXT,
    turn_id             TEXT,
    obligation_id       TEXT,

    source_namespace    TEXT NOT NULL,
    source_event_id     TEXT,
    source_event_fence  TEXT,

    -- Strictly schema-validated, redaction-safe metadata only.
    safe_metadata_json  TEXT NOT NULL DEFAULT '{}',

    UNIQUE (source_namespace, source_event_id, source_event_fence)
);
```

The unique source tuple deduplicates replayed native lifecycle events. If a source
does not provide a stable event ID, its adapter must construct a deterministic
fence from stable non-secret identifiers such as session incarnation + turn +
event class + native sequence. It must not hash secret prompt/transcript content
as a substitute for identity.

`safe_metadata_json` is not an escape hatch. Every event kind has a typed
serializer and explicit allowed fields/lengths. Unknown provider payload fields
are discarded, not persisted opportunistically.

## Project and task identity

```sql
CREATE TABLE projects (
    project_id           TEXT PRIMARY KEY,
    source_host          TEXT NOT NULL,
    source_repo_id       TEXT,
    source_repo_name     TEXT,
    created_event_seq    INTEGER NOT NULL REFERENCES events(seq)
);

CREATE TABLE tasks (
    task_id              TEXT PRIMARY KEY,
    project_id           TEXT NOT NULL REFERENCES projects(project_id),
    source_issue_ref     TEXT,
    created_event_seq    INTEGER NOT NULL REFERENCES events(seq),
    latest_event_seq     INTEGER NOT NULL REFERENCES events(seq)
);
```

Repository refs are durable identifiers/URLs, not copies of repository contents.

## Sessions and incarnations

A display name is never an identity fence.

```sql
CREATE TABLE sessions (
    session_id           TEXT PRIMARY KEY,
    project_id           TEXT NOT NULL REFERENCES projects(project_id),
    runtime_kind         TEXT NOT NULL,
    worker_kind          TEXT NOT NULL,
    display_name         TEXT,
    created_event_seq    INTEGER NOT NULL REFERENCES events(seq)
);

CREATE TABLE session_incarnations (
    session_incarnation_id TEXT PRIMARY KEY,
    session_id             TEXT NOT NULL REFERENCES sessions(session_id),
    generation             INTEGER NOT NULL,
    runtime_instance_ref   TEXT,
    worker_session_ref     TEXT,
    started_event_seq      INTEGER NOT NULL REFERENCES events(seq),
    ended_event_seq        INTEGER REFERENCES events(seq),
    UNIQUE(session_id, generation)
);
```

A runtime restart/adoption that cannot prove continuity creates a new incarnation.
A stale event from an older incarnation cannot mutate the projection for a newer
one.

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

`lifecycle_state` is a projection. Validity comes from replaying accepted event
kinds under the worker lifecycle state machine.

## Result artifacts

SQLite holds metadata; sensitive content is in a private immutable file.

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

`storage_ref` is an opaque store key, not an arbitrary path supplied by a worker.
The artifact directory is controlled by the daemon. MCP retrieval validates the
reference, size, digest, and obligation fence before reading.

### Crash-safe artifact commit order

For a terminal worker result:

1. create a temp file inside the private artifact directory with owner-only mode;
2. write the bounded final result;
3. `fsync`/`fdatasync` the file;
4. atomically rename to its immutable store key;
5. sync the containing directory where the platform requires it for rename
   durability;
6. in one SQLite transaction, append the terminal lifecycle event, insert the
   artifact metadata, and create/update `completed_unprocessed` obligation state;
7. only after commit may the terminal result be announced to wake logic.

A crash between steps 5 and 6 can leave an unreferenced artifact, which startup
GC can safely quarantine/remove after a grace period. The forbidden ordering is a
committed obligation that points at an artifact that was never made durable.

ACK changes retention eligibility in SQLite first; file deletion is asynchronous
GC. An open obligation always pins the artifact.

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
    claimed_at_ms           INTEGER,
    claim_expires_at_ms     INTEGER,
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

`current_version` is incremented on every state change. Mutations use a
compare-and-swap fence over `(obligation_id, current_version, binding_generation,
claim_id as applicable)`.

A duplicate worker terminal event hits the source-event uniqueness constraint and
must return the existing obligation/result rather than creating a second one.

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
    bound_event_seq         INTEGER NOT NULL REFERENCES events(seq),
    superseded_event_seq    INTEGER REFERENCES events(seq),
    UNIQUE(provider, canonical_conversation_id, binding_generation)
);
```

There is one active binding projection in V1. Rebind inserts a new generation and
supersedes the prior one transactionally. Browser cookies, session tokens, account
secrets, and raw local-storage values are never stored here.

## Browser deliveries

```sql
CREATE TABLE browser_deliveries (
    delivery_id             TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
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

The wake payload itself is tiny and non-sensitive. Storing its digest is adequate
for identity/audit; the daemon can deterministically re-render it from the opaque
IDs for a definite pre-submit retry. The DB never stores Claude output in the
wake row.

A delivery revision may have multiple attempts **only while every prior attempt
is proven `failed` before the activation fence**. Once any attempt is
`activation_armed`, `accepted`, or `ambiguous`, that delivery revision is frozen.
A post-settlement foreman resume creates a new `delivery_revision`, never another
attempt on the uncertain/accepted delivery.

## Foreman physical-turn observations

Physical ChatGPT settlement is deliberately separate from obligation ACK.

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

A settled row has no authority to close `obligations`.

## Input requests and responses

```sql
CREATE TABLE input_requests (
    input_request_id        TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    request_kind            TEXT NOT NULL,
    request_revision        INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    safe_schema_json        TEXT NOT NULL,
    answered_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(turn_id, source_event_seq, request_revision)
);
```

`safe_schema_json` may contain only the minimum structured question schema safe to
show the foreman: opaque choice IDs, bounded labels when policy permits, risk
classification, and required-answer shape. It does not copy arbitrary tool args.
If the original question text is required for a meaningful answer, it belongs in
a private input artifact under the same sensitive artifact policy, not in the
ledger.

A foreman answer is an immutable event and creates a separate worker-command
obligation/delivery. ACKing the input question does not pretend the worker resume
was delivered. Worker command delivery gets the same external-I/O discipline:
intent before spawn/write, explicit accepted/failed/ambiguous evidence, and no
blind replay after an ambiguous resume.

## Progress and watchdog projection

Progress events persist only bounded safe metadata:

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

Adapters may rate-limit/coalesce extremely chatty equivalent progress sources,
but must preserve enough timestamps to prove the watchdog threshold was or was
not crossed. Coalescing policy itself is deterministic and tested.

## Health and reconciliation records

Do not encode every operational problem as a fake task state. Durable health
records can represent conditions such as:

- `suspected_stall`
- `foreman_unreachable`
- `mcp_write_capability_missing`
- `browser_binding_displaced`
- `result_artifact_missing`
- `projection_mismatch`
- `runtime_state_conflict`

They reference the event/obligation/turn they concern and can be resolved by a
later event without closing the underlying obligation.

## Meta and migrations

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE schema_migrations (
    version          INTEGER PRIMARY KEY,
    name             TEXT NOT NULL,
    checksum         TEXT NOT NULL,
    applied_at_ms    INTEGER NOT NULL
);
```

Important meta values include schema epoch, database instance ID, last verified
projection event sequence, and application compatibility epoch. No credential or
browser secret is meta data.

## Transaction boundaries that must be tested

### Terminal worker completion

One DB transaction:

- insert/dedupe normalized terminal event;
- finalize `turns` projection;
- reference already-durable result artifact;
- create or transition exactly one obligation to `completed_unprocessed`;
- append obligation transition event.

### Claim browser delivery

One DB transaction:

- verify obligation still open and binding generation current;
- insert/find deterministic delivery revision;
- insert attempt with `claimed`;
- append event;
- commit **before** navigation/composer mutation.

### Arm Send ambiguity fence

One DB transaction commits `activation_armed` immediately before issuing the
exact CDP/DOM Send action. A crash after commit but before the browser receives
the action is intentionally recovered as ambiguous.

### ACK

One DB transaction:

- verify current binding generation;
- verify obligation version/source event/claim ID;
- verify result/input object still matches the claim;
- append explicit foreman disposition event;
- close obligation projection;
- mark pinned artifacts retention-eligible only if policy allows.

No browser or worker I/O occurs inside the ACK transaction.

## Forbidden persistence regression fixture

Tests inject unique sentinel strings into:

- cwd;
- prompt text;
- tool arguments;
- shell commands;
- transcript path;
- terminal transcript;
- browser cookies/tokens;
- GitHub token/auth fields.

After the scenario, the SQLite database, WAL, structured logs, crash reports, and
non-artifact state directories are scanned byte-for-byte for every sentinel.
Expected result: **zero matches**. Sensitive result/input artifacts are tested
separately for ACL, bounded content, and retention behavior.
