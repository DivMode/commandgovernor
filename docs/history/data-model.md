# V1 durable data model

This document specifies the initial SQLite authority, managed-run staging, and
private result-artifact boundary.

The SQL below is now **implemented**, as migration `0001_initial` in
`crates/governor-store-sqlite/src/migrations/`. Where the implementation differs
from the architecture sketch this document originally carried, the difference is
recorded inline as **Deviation** and marked `DEVIATION:` in the migration itself.
Two conventions apply throughout and are not repeated per table:

- every table is `STRICT`, and every state column carries a `CHECK` against its
  closed label set, so a value the code cannot decode cannot be written;
- opaque identities are canonical UUID text, and correctness never parses one.

## Principles

1. **Events first, projections second.** Normalized source/domain events are
   immutable; current-state tables are replayable materialized projections.
2. **One writer.** One daemon-owned `rusqlite` actor serializes state changes.
3. **No transcript database or raw worker spool.** Prompt text, raw tool arguments
   or results, shell commands, cwd, transcript paths, terminal transcript, browser
   cookies/tokens, GitHub credentials, arbitrary provider payloads, and complete
   provider streams are forbidden from SQLite and durable worker-host staging.
4. **Fences are explicit data.** Session incarnation, turn, source event,
   obligation version, binding generation, wake revision, and foreman claim are
   represented directly.
5. **External I/O is outside SQLite transactions.** Transactions establish durable
   intent/ambiguity fences before I/O and persist evidence after it.
6. **The actual final result survives runtime death.** A bounded final worker result
   lives in a separate owner-private immutable artifact store referenced by
   SQLite.
7. **Idempotency identity is not possession identity.** Browser delivery has a
   deterministic non-secret `delivery_key` plus a separately generated random
   `delivery_id` used as the accepted-wake correlation fence.

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
is useful. Browser `delivery_key` values use a domain-separated cryptographic hash
for deterministic deduplication. Browser `delivery_id` values are CSPRNG-generated
opaque correlation IDs of at least 192 bits. Correctness never depends on parsing
an ID.

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

Meta may contain schema epoch, database instance ID, last verified projection
sequence, and the daemon epoch. It must not contain credentials or browser
session material. The keys are a closed set declared in the store; there is no
API that writes an arbitrary one.

`daemon_epoch` is the lifetime counter of the owning daemon process. Startup
advances it exactly once, and every mutation-command row, external-effect intent
and resource lease records the epoch it was written under — which is what makes
"this row is from a previous process" a fact rather than a guess.

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

**Deviation (additive):** the implementation adds
`CREATE INDEX events_by_obligation_seq ON events(obligation_id, seq)`. A fenced
compare-then-mutate folds one obligation's ledger slice inside the write
transaction; without the index that is a full scan of `events`.

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

## Managed-run filesystem staging

Managed Claude output is parsed online by `worker-host`; SQLite does not contain a
raw provider stream and the filesystem does not retain one either.

The worker-host may durably write only:

1. a **sanitized managed-run receipt** containing allowlisted opaque IDs, safe
   event/outcome classes, completeness flags, and timestamps;
2. a **bounded final-result candidate** containing only the final assistant result
   required for review, and only after a complete final result record is parsed;
3. a **sanitized child-exit receipt** containing the fenced child identity and
   safe exit outcome.

Intermediate stream-json records—including raw `tool_use`, `tool_result`, prompt,
command, cwd, transcript path, or arbitrary provider bodies—are processed in
memory and discarded. A confirmed deferred tool receipt stores only safe opaque
identity/classification and stop reason, not raw deferred tool input.

The staging root is owner-private against other OS users, opaque-keyed, bounded,
and excluded from diagnostics. V1 does not claim same-user hostile-process
containment from file modes.

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

1. validate the fenced final-result candidate + sanitized run/exit receipts;
2. create owner-only temp file under the private artifact root;
3. write only the bounded final worker result required for review;
4. fsync/fdatasync file;
5. atomically publish the immutable store key with `link(2)`, then `unlink(2)`
   the staging name;
6. sync containing directory where required;
7. in one SQLite transaction append/dedupe terminal event, insert artifact
   metadata, finalize turn projection, and create/update exactly one obligation;
8. only after commit may wake scheduling observe completion.

**Deviation (step 5).** This document said "atomically rename to immutable store
key". The implementation uses `link(2)` followed by `unlink(2)` of the staging
name instead. Both publish the name atomically; the difference is what happens
when the destination already exists. `rename(2)` **silently replaces** it —
verified empirically on this platform — so it cannot express "an artifact is
immutable and there is no overwrite path", while `link(2)` fails `EEXIST` and the
store reports an already-published key — a bug or an attack, never a silent
overwrite. Immutability becomes a property the filesystem enforces rather than
one the caller is trusted not to violate. The durability
ordering is unchanged: bytes are `fsync`ed before the immutable name exists, and
the directory is `fsync`ed before the durability proof is minted.

A crash before step 7 may leave an unreferenced orphan file, which is safe to
quarantine/GC after a grace period. A crash between the `link` and the `unlink`
leaves one inode under two names — the published one is correct and durable, and
the leftover staging name is swept as an orphan. The forbidden outcome is a
committed open obligation pointing at an artifact that was never made durable.

ACK only makes an artifact retention-eligible; asynchronous GC deletes later.
Every open obligation that references an artifact pins it.

### Who writes `eligible_for_delete_at_ms`

`retention_state` is derived, never set: it is recomputed from the obligations
that actually reference the artifact on every transition, so nothing can release
an artifact an open obligation still needs. `eligible_for_delete_at_ms` is the
"later" a sweep compares `now` against, and the ACK transaction writes it:

- the value is **ACK instant + a retention-grace policy input the caller
  supplies**. The store invents neither half, and the sweep does not re-add a
  locally configured grace on top — two policy authorities would eventually
  disagree about when bytes disappear;
- it is written with `COALESCE`, so the first release instant stands. A repeated
  or idempotent ACK cannot push an already-released artifact's deletion further
  into the future;
- it is guarded on `retention_state = 'eligible'`, so it stamps only what the
  recompute genuinely released; an artifact another open obligation still needs
  is untouched;
- a pinned artifact has it set back to `NULL` in the same statement that pins it.
  Pinned and "deletable at" are not both meaningful.

A released artifact with **no** recorded instant is kept forever. The grace
period cannot be evaluated, and failing closed there costs disk while guessing
costs a result.

**Phase 1 policy, worth naming because it is a live consequence.** User
cancellation closes an obligation and therefore releases the pin, but it carries
no retention policy and stamps no instant — so a cancelled task's artifact is
kept indefinitely. That is the fail-closed side of the rule above, not an
oversight, and it is the behaviour until cancellation is given its own retention
policy input.

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
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    current_version         INTEGER NOT NULL,
    current_binding_generation INTEGER,
    current_claim_id        TEXT,
    incarnation_generation  INTEGER NOT NULL,
    input_request_id        TEXT,
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    closed_event_seq        INTEGER REFERENCES events(seq)
);

CREATE INDEX obligations_open_by_task
    ON obligations(task_id)
    WHERE closed_event_seq IS NULL;

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

**Deviations.** The document's single `source_event_seq` is split: the event that
*created* the obligation and the event carrying the source fact it currently
stands on are different facts, and a wake snapshot and an ACK fence compare
against the latter. `incarnation_generation` and `input_request_id` are two more
fields the pure `Obligation` projection carries that the sketch had nowhere to
put. `obligations_open_by_task` is an additive index. `input_request_id` has no
foreign key: `input_requests.obligation_id` already points the other way, and
SQLite cannot resolve the cycle without deferred constraints.

`current_version` is compare-and-swap state. A terminal source event duplicate
hits the event-ledger unique fence and returns the existing result/obligation
rather than creating a second one.

`obligation_events.disposition` is non-null only on the closing transition, and
its closed label set is `accepted | rejected_needs_rework | failure_acknowledged
| abandoned`. Which of them may close a given obligation depends on the attention
state the claim came from — a success disposition cannot close a failure, a
failure disposition cannot close a result, and `needs_input` closes only via
`abandoned`. The matrix is in
[`adr/0004-foreman-mcp-and-binding.md`](adr/0004-foreman-mcp-and-binding.md),
"The disposition set", and the check lives in the pure obligation machine, so it
is one authority rather than a `CHECK` constraint restating it.

**How the row relates to the ledger.** This table is a *materialised* copy. A
fenced transition does not read its state from here: it folds the obligation's
ledger slice through the `governor-core` state machine inside the write
transaction, applies the next event to that value, and writes the result here in
the same transaction. Replay verification then proves the copy still agrees with
the ledger, which is `docs/testing.md` DB-001.

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

`write_capability_state` records the actual feature-tested surface state. As of the
architecture review, consumer ChatGPT Pro is expected to be
`read_fetch_only_unsupported`; candidate Business/Enterprise/Edu workspaces must
still prove mutation/confirmation behavior.

## Browser wake deliveries

A wake is targeted at the **exact obligation version/source fact that existed when
it was scheduled**.

```sql
CREATE TABLE browser_deliveries (
    delivery_id             TEXT PRIMARY KEY,
    delivery_key            TEXT NOT NULL UNIQUE,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    target_obligation_version INTEGER NOT NULL,
    target_source_event_seq INTEGER NOT NULL REFERENCES events(seq),
    foreman_binding_id      TEXT NOT NULL REFERENCES foreman_bindings(foreman_binding_id),
    binding_generation      INTEGER NOT NULL,
    delivery_revision       INTEGER NOT NULL,
    attempt_budget          INTEGER NOT NULL,
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

**Deviation.** `browser_deliveries.attempt_budget` is part of the revision's
durable identity. Without it the attempt machine cannot be rebuilt: replay would
have to guess the bounded budget the revision was created with.

`wake_payload_digest` is computed by the store from the already-created random
`delivery_id` plus the scheduling tuple and the protocol label. Computing it
rather than accepting it means no caller can put worker output in that column
even by accident.

Creation computes deterministic non-secret:

```text
delivery_key = H("command-governor/wake-key/v1",
                 obligation_id,
                 binding_generation,
                 delivery_revision)
```

and independently generates random:

```text
delivery_id = CSPRNG(>=192 bits)
```

The unique `delivery_key` makes duplicate scheduling converge on one row. The
random `delivery_id` is the value embedded in the browser wake and later required
by `foreman_resume`; bootstrap/status never expose it. It is not sole
authentication.

Immediately before any composer mutation and again before Send activation, the
adapter verifies the obligation is still open at
`target_obligation_version/target_source_event_seq` and binding generation is
current. If the obligation changed, this wake is stale and cannot Send.

The wake payload is deterministic given the already-created random delivery ID;
SQLite stores its digest, not worker output.

A delivery revision can have another attempt only after a prior attempt is
**proven failed before the Send ambiguity fence**. The aggregate delivery may
therefore transition `failed -> claimed` for a bounded safe retry. Once any attempt
is accepted or ambiguous, that revision is frozen forever. A later foreman resume
is a new `delivery_revision`, new `delivery_key`, and new random `delivery_id`.

`delivery_attempts.failure_class` is therefore two populations, and
`activation_armed_event_seq` is what separates them. A pre-fence class
(`target_not_found`, `stale_target`, `wrong_conversation`, `app_not_selected`,
`composer_not_ready`, `navigation_blocked`) is a retryable `failed`. After arming,
only `activation_refused` and `transport_rejected_before_send` are admissible as
proof of no submission at all; they record a terminal `failed` that is **not**
retryable on that revision, and every other post-arm outcome is `ambiguous`. A
claim against a revision whose last attempt has `activation_armed_event_seq` set
is a typed `retry_after_ambiguity_fence` conflict regardless of that attempt's
terminal state, so a post-fence `failed` row is not a weaker `ambiguous` — it is
an equally frozen revision that happens to carry a truthful outcome.
`docs/state-machines.md` §4 "Retry classification" is the same rule.

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

A claim is minted only from an accepted current-generation wake whose **random
`delivery_id` was presented by the caller** and whose target obligation
version/source fact is still current. Claim expiry is an internal coordination
event: it may return an obligation to its prior attention state but can never close
it or release a required artifact.

**Expiry as implemented, and why it touches `browser_deliveries`.** Expiry
appends its own event, so it bumps `obligations.current_version` exactly as the
claim before it did. The accepted wake's `target_obligation_version` was frozen at
scheduling time and is now two versions behind, which would make the only accepted
wake for that obligation permanently stale — work still owed, with no way to hand
it over. So the expiry transaction re-points that one wake:

```sql
UPDATE browser_deliveries
   SET target_obligation_version = :restored_version
 WHERE delivery_id = :wake
   AND target_source_event_seq = :current_source_event_seq;
```

The `target_source_event_seq` predicate is the whole guard. `obligations`' source
fact advances only on accepted worker events, so the re-point succeeds across a
claim/expiry round trip that changed nothing else, and fails across a genuine
worker event — which correctly leaves the wake stale, because it is now about
older work. This is what keeps `docs/testing.md` OBL-008's reclaim path alive
across the version bumps claim and expiry cause. The restored attention state is
not read from a column: the loader folds the obligation's ledger slice, so it
cannot be a stored value that drifted. Nothing here closes work or unpins an
artifact — the restored obligation is open, so `refresh_retention` recomputes the
pin as held.

**ACK checks the obligation's claim first.** Before the presented `claim_id` row
is rehydrated at all, ACK fences the claim the *obligation* currently records. An
obligation that expired and was reclaimed is held by a different claim, and the
displaced holder's honest answer is `stale_claim` — `docs/testing.md` OBL-004 —
rather than the `expired_claim` its own row would also support. The check is
skipped only when nothing holds the obligation, which is the already-closed case
where an exact repeat of the committed ACK must still return idempotent success.

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
or tool arguments. A clean deferred input request requires a provider-native exact
reference (for current Claude PreToolUse, a tool-use identity) plus confirmed
single-tool deferred outcome. Current `PermissionRequest` input lacks the same
`tool_use_id`; permission events may be recorded as safe decision evidence but do
not fabricate a precise resumable input identity.

When the foreman claims a true deferred request, the worker adapter obtains current
question/choice detail ephemerally from the native Claude session/provider if the
current adapter supports it. If that detail cannot be recovered after restart, the
obligation remains `needs_input` and returns `input_detail_unavailable`; Command
Governor does not invent an answer.

`native_input_ref` is an opaque provider identity, never a transcript path or
serialized arguments.

### The structured answer set

`answer_shape` is one of *single choice* carrying the provider's option **count**,
*boolean*, or *opaque token* — a count and a kind, never option text. The answer
recorded against it is exactly one of:

```text
Choice { index }      -- zero-based index into the provider's option list
Boolean { value }
OpaqueToken { token } -- an opaque provider-defined selection token
Declined              -- valid against any shape; the request stays owed
```

An answer that does not fit the declared shape — an index past the option count,
or a boolean against a choice — is a typed conflict, and a second, differing
answer to an already-answered request is `conflicting_input_answer`. The first
answer is immutable, because one continuation is already outstanding.

**There is deliberately no free-text variant.** Free prose from a foreman is
durable transcript-adjacent content, and a column that accepts a sentence is
where prompts, tool arguments and eventually credentials end up — the same reason
`mutation_commands` has no result blob. Recorded here as a fail-closed Phase 1
policy: widening the set is a deliberate, reviewed change to the answer type, not
a field somebody repurposes.

**Open question for the MCP-contract phase.** If a foreman genuinely must type a
sentence back to a worker — a clarification the option list does not cover — the
data model has no sanctioned place to put it today, and `OpaqueToken` is not that
place. Resolving it means either establishing that the input protocol never needs
prose, or defining a bounded, explicitly classified prose field with its own
retention and redaction rules. Until then the answer is `Declined` plus a
human-owned path outside Command Governor.

**Phase 1 status.** The answer type and its shape check live in `governor-core`;
`input_requests` has no store writer yet, for the same reason the worker-command
tables below have none. `answer_shape` carries no `CHECK` constraint until the
Phase 2 adapter fixes its durable labels — which is also the last moment at which
widening the answer set is cheap.

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
    attempt_budget          INTEGER NOT NULL,
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
    ambiguity_armed_event_seq INTEGER REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    started_at_ms           INTEGER NOT NULL,
    finished_at_ms          INTEGER,
    failure_class           TEXT,
    evidence_class          TEXT,
    UNIQUE(worker_command_id, attempt_no)
);
```

**Deviations.** `attempt_budget` for the same reason as the browser delivery;
foreign keys on the two `*_event_seq` columns the sketch left bare, because every
other sequence column has one and a dangling reference here would break replay
silently; and the same timing/outcome columns `delivery_attempts` carries, since
both tables project the same attempt machine.

**Phase 1 status.** These two tables exist and are locked down, but no store
operation writes them yet: worker continuation delivery arrives with the Phase 2
worker adapter. Startup quarantine consequently covers browser deliveries only.
`docs/testing.md` DB-006's worker-command half is therefore still open.

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
    external_attempt_id     TEXT REFERENCES external_attempts(external_attempt_id),
    opened_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    resolved_event_seq      INTEGER REFERENCES events(seq)
);

CREATE UNIQUE INDEX health_conditions_one_open_per_scope
    ON health_conditions(
        kind,
        COALESCE(task_id, ''),
        COALESCE(turn_id, ''),
        COALESCE(obligation_id, ''),
        COALESCE(external_attempt_id, '')
    )
    WHERE state = 'open';
```

**Deviations.** `external_attempt_id` is a fourth scope column, matching the new
`HealthScope` field: scope is part of a condition's identity, so without it two
ambiguous external attempts would collapse onto one condition. The partial unique
index is the durable half of that deduplication.

Initial kinds include:

- `suspected_stall`
- `foreman_unreachable`
- `mcp_write_capability_missing`
- `browser_binding_displaced`
- `result_artifact_missing`
- `projection_mismatch`
- `runtime_state_conflict`
- `input_detail_unavailable`
- `worker_defer_shape_unsupported`
- `reconciliation_required`

`reconciliation_required` is raised for an external attempt whose intent is
durable but whose outcome was never proven — the crash window `ambiguous`
describes. Like every other kind it is attention: it authorises no replay, and
resolving it is an explicit human or reconciliation decision.

A health condition never pretends to be worker completion.

## Mutation-command journal

**Deviation: whole table.** It is the SQLite form of the Prime-Agent-style
command journal in
`docs/research/2026-08-31-durable-orchestration-pattern-review.md`, which this
document predates. One authority: the journal lives here, never in a second log
file.

```sql
CREATE TABLE mutation_commands (
    actor_id                TEXT NOT NULL,
    command_id              TEXT NOT NULL,
    fingerprint             TEXT NOT NULL,
    command_kind            TEXT NOT NULL,
    status                  TEXT NOT NULL,
    safe_result_kind        TEXT,
    safe_result_ref         TEXT,
    safe_result_conflict    TEXT,
    daemon_epoch            INTEGER NOT NULL,
    created_at_ms           INTEGER NOT NULL,
    completed_at_ms         INTEGER,
    uncertain_at_ms         INTEGER,
    acked_at_ms             INTEGER,
    PRIMARY KEY(actor_id, command_id)
);
```

The transaction protocol is:

```text
BEGIN IMMEDIATE
  insert unique (actor_id, command_id, received)
COMMIT
-> only now may the caller dispatch consequential I/O
-> commit the completed safe result before replying
```

An exact retry of a `completed` identity returns the recorded result with zero
dispatch. A retry of `received` or `uncertain` returns typed
`mutation_result_uncertain` and is **never** redispatched; startup turns any
previous-epoch `received` row into `uncertain`. An `uncertain` row may still
reach `completed`, but only through late *proven* evidence that the mutation did
commit — that is a record, not a retry, and nothing dispatches.

`fingerprint` goes beyond the research doc's conceptual table. "Exact retry" has
to mean exact: without it, a client reusing a command id for a different
operation would silently receive the first operation's recorded result. A
mismatch is typed `mutation_command_mismatch`. It is a digest of the fenced
parameters, never the parameters.

The result is three narrow columns rather than one blob. A column that could
hold an arbitrary response body would be a place for prompts, tool output and
credentials to accumulate.

Receipt ACK is **layer 1 of three** and reaches nothing else: it marks a row
`acked`, which combined with policy age makes it eligible for compaction. It
cannot close an obligation.

## Consequential external effects

**Deviation: whole table**, from the same research doc: "external attempts remain
a separate domain table because command delivery and the external side effect are
different facts".

```sql
CREATE TABLE external_attempts (
    external_attempt_id     TEXT PRIMARY KEY,
    effect_class            TEXT NOT NULL,
    idempotency_contract    TEXT,
    idempotency_window_ms   INTEGER,
    idempotency_key         TEXT,
    destination_namespace   TEXT NOT NULL,
    destination_endpoint    TEXT NOT NULL,
    destination_fence       TEXT NOT NULL,
    source_namespace        TEXT NOT NULL,
    source_event_id         TEXT NOT NULL,
    source_event_fence      TEXT NOT NULL,
    daemon_epoch            INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    dispatched              INTEGER NOT NULL,
    completion_ref          TEXT,
    no_effect_class         TEXT,
    ambiguity_reason        TEXT,
    recorded_at_ms          INTEGER NOT NULL,
    dispatched_at_ms        INTEGER,
    finished_at_ms          INTEGER
);
```

The source fence is the `SourceRef` triple rather than an `events(seq)`: an
intent is committed in its own transaction *before* any I/O, and the fact that
justified it is not necessarily an event this ledger holds.

The protocol is `BEGIN IMMEDIATE` → insert unique intent row → `COMMIT` → and
only then may an execution permit exist. The primary key is what makes it unique:
the insert is a plain `INSERT`, so a crash-and-retry reusing an attempt identity
hits the constraint rather than minting a second permit for one logical
operation.

`dispatched` is committed immediately before the adapter issues its call. A crash
after it, with no proven outcome, is `ambiguous` — never success, never failure —
and startup opens a `reconciliation_required` condition scoped to the attempt.
There is no automatic escape: progress requires a *new* attempt, which the domain
admits only for a read, a proven-absent effect, or an idempotent write whose
recorded contract and exact key the new attempt reproduces.

Stated as Phase 1 policy, because the browser tables next door do have a
promotion and the contrast is easy to miss: a generic external-attempt
`ambiguous` is **strictly terminal**, with no promotion path at all. No evidence
turns this row into a success; the row keeps its ambiguity forever and any
progress is a different row. `browser_deliveries` keeps the separate
exact-evidence promotion it already had — `ambiguous -> accepted` on the
provider-native message identity in the bound conversation, performing no Send —
because a browser wake has an exact after-the-fact identity to check against and
a generic external destination does not.

## Resource leases

**Deviation: whole table**, from the research doc's "resource ownership". It is
deliberately small. The global daemon/state-root lock is *not* a lease — "for V1
the global daemon/state-root lock remains simpler than a distributed lease" — and
stays a file lock owned by the daemon. This table exists only for resources where
a second process legitimately participates.

```sql
CREATE TABLE resource_leases (
    resource_namespace      TEXT NOT NULL,
    resource_digest         TEXT NOT NULL,
    resource_lease_id       TEXT NOT NULL UNIQUE,
    lease_token             BLOB NOT NULL,
    holder_actor_id         TEXT NOT NULL,
    process_slot            INTEGER NOT NULL,
    process_start_ref       TEXT NOT NULL,
    daemon_epoch            INTEGER NOT NULL,
    state                   TEXT NOT NULL,
    acquired_at_ms          INTEGER NOT NULL,
    renewed_at_ms           INTEGER NOT NULL,
    expires_at_ms           INTEGER NOT NULL,
    released_at_ms          INTEGER,
    PRIMARY KEY(resource_namespace, resource_digest)
);
```

One row per resource, holding the most recent lease and keeping it after release
so a superseded holder can still be told precisely why it lost.

The canonical resource *name* is never stored: a path, a socket location or a
profile directory is forbidden durable control-plane data, so the identity is a
namespace plus the digest of that name. The possession token is raw bytes with no
text form, so it cannot reach a log line through a formatter. Renew and release
check all three fences — token, process incarnation and daemon epoch — so a
recycled process number, a superseded daemon lifetime, or a token from a lease
that was taken over all fail closed across a restart.

## What is derived from the ledger, and what is not

Obligations, browser deliveries and their attempts are **projections**: rebuilt by
folding `events` through the pure state machines, and proven equivalent by replay
verification.

The three tables above are **not** ledger-derived. Each is a self-contained row
whose own transaction protocol is the durability contract — an intent row must
commit alone and first, before any consequential I/O, so coupling it to an event
append would couple it to a fact the ledger may not hold. They are still
re-proved on every read: each loader folds the row's own recorded history through
the domain machine and refuses a row that no legal sequence of transitions can
reach.

## Critical transaction boundaries

### Worker terminal publication

One DB transaction after the final result artifact is durable:

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
- compute deterministic `delivery_key`;
- insert/find the delivery revision and generate/persist its random `delivery_id`
  exactly once;
- insert attempt `claimed`;
- append event;
- commit **before any browser I/O**.

If duplicate scheduling finds the same `delivery_key`, it returns the existing
row/random `delivery_id`; it never generates a second physical revision identity.

The steps above read as one indivisible "create and claim", which left it unclear
what happens when the row already exists and someone is already acting on it.
Resolved as implemented, in three parts:

- **The revision is found or created, and the caller is told which.** A duplicate
  `delivery_key` returns the existing row and its existing random `delivery_id`.
  A candidate `delivery_id` is drawn unconditionally before the transaction —
  the CSPRNG is a port and a transaction body cannot reach one — and discarded on
  the found path. It is never persisted, so a duplicate schedule cannot rotate a
  live wake's correlation ID.
- **The attempt claim is a separate decision and can fail on its own.** Finding
  the row does not entitle the caller to an attempt. If the revision's last
  attempt is still live (`claimed` or `activation_armed`), the claim is a typed
  conflict and the whole transaction rolls back — someone else already owns the
  external effect, and two claimed attempts on one revision is how one wake gets
  Sent twice. If the revision is frozen (`accepted` or `ambiguous`) the conflict
  is `delivery_revision_frozen`; past the fence it is
  `retry_after_ambiguity_fence`; past the durable budget it is
  `retry_budget_exhausted`.
- **A new attempt exists only after a prior attempt is proven failed pre-fence**,
  within the revision's `attempt_budget`.

The found path also re-verifies the wake's own snapshot against the obligation, so
a revision scheduled against a state the obligation has since left is refused as
a stale target rather than claimed.

**At most one revision per obligation may be able to act at a time.** The unique
`(obligation, generation, revision)` key makes revisions distinct; this rule
makes them exclusive *in time*, in two halves enforced inside the same
transaction:

- **Creation requires every earlier revision to be terminal.** A new revision is
  refused with `delivery_revision_still_live` while any revision for the
  obligation — across binding generations — is still `pending` or `claimed`,
  because two live wakes about one obligation are two chances at the same
  external effect.
- **A superseded revision may never act again.** A failed revision keeps its
  attempt budget, so a bounded retry on it is normally legal — but once a
  successor revision exists, claiming an attempt on the older one is refused
  with `delivery_revision_superseded`. Resurrection is how revision N and
  revision N+1 would otherwise both become live.

### Arm browser Send

Immediately before exact Send action, one DB transaction re-verifies binding and
target obligation snapshot, then commits `activation_armed`. A crash after commit
is recovered as ambiguous.

### `foreman_resume`

One transaction verifies accepted random wake `delivery_id` + target snapshot +
current generation, creates one current claim, updates obligation
projection/version, and appends the claim event. Artifact reading happens after the
transaction; inability to read it does not close the claim/obligation.

### ACK

One transaction verifies obligation version, source event, binding generation,
claim, and disposition; appends explicit disposition event; closes projection; and
marks pinned artifacts retention-eligible only as policy permits. No external I/O
occurs inside ACK.

### How "no external I/O inside a transaction" is enforced

Not by review. Everything ambient the store crate can reach — the clock, the
CSPRNG, identity minting — lives behind one `StorePorts` value, which is lent
only to the phase of a write that runs *before* `BEGIN IMMEDIATE`. The
transaction body's signature takes no ports, so inside a transaction there is
nothing to call: no clock, no entropy, no adapter. The crate itself performs no
filesystem, network or process I/O at all — `rusqlite` owns the only file handle
— and a test scans the crate's own source to keep it that way.

A third phase runs strictly after `COMMIT` returns. It exists for exactly one
thing: surrendering the durable-intent acceptance that authorises an external
execution permit. Putting that in a phase the runner reaches only after a
successful commit makes "intent before I/O" a property of the code's shape
rather than a rule somebody has to remember.

## Forbidden-persistence fixture

Tests inject unique sentinels into:

- cwd;
- prompt;
- raw tool arguments;
- raw tool results;
- shell command;
- transcript path;
- terminal transcript;
- intermediate Claude stream records;
- browser cookies/tokens/headers/bodies;
- GitHub auth.

After lifecycle/browser/MCP/crash scenarios, byte-scan SQLite DB/WAL/SHM, hook
inbox, managed-run staging/receipts, structured logs, safe diagnostics, crash
state, and configuration. Expected: **zero sentinel matches** outside an explicitly
designated bounded final-result candidate/result artifact when the test
intentionally places final worker result content there.
