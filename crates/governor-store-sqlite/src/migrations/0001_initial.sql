-- Command Governor initial schema (epoch 1).
--
-- Follows docs/data-model.md. Every deviation from the SQL in that document is
-- marked `DEVIATION:` with the reason, and the document is updated in the same
-- pull request.
--
-- Conventions:
--   * opaque identities are canonical UUID text; correctness never parses them;
--   * instants are Unix milliseconds, evidence only, never ordering authority;
--   * `*_event_seq` columns point at the daemon-assigned ledger sequence, which
--     is the only ordering authority;
--   * every state column carries a CHECK against its closed label set, so a
--     value the code cannot decode cannot be written in the first place.
--     DEVIATION: docs/data-model.md leaves state columns as bare TEXT.

-- ---------------------------------------------------------------------------
-- Meta and migrations
-- ---------------------------------------------------------------------------

CREATE TABLE meta (
    key                     TEXT PRIMARY KEY,
    value                   TEXT NOT NULL
) STRICT;

CREATE TABLE schema_migrations (
    version                 INTEGER PRIMARY KEY,
    name                    TEXT NOT NULL,
    checksum                TEXT NOT NULL,
    applied_at_ms           INTEGER NOT NULL
) STRICT;

-- ---------------------------------------------------------------------------
-- Immutable event ledger
-- ---------------------------------------------------------------------------

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

    -- Event-kind-specific, typed, redaction-safe metadata only. Written solely
    -- by the per-kind serializers in `safe_metadata.rs`; there is no generic
    -- "persist this JSON" path anywhere in the crate.
    safe_metadata_json      TEXT NOT NULL DEFAULT '{}',

    UNIQUE(source_namespace, source_event_id, source_event_fence)
) STRICT;

-- DEVIATION: additive index. Obligation replay reads one obligation's ledger
-- slice; without this it is a full scan of `events`.
CREATE INDEX events_by_obligation_seq ON events(obligation_id, seq);

-- ---------------------------------------------------------------------------
-- Projects and tasks (source-host provenance, never repository content)
-- ---------------------------------------------------------------------------

CREATE TABLE projects (
    project_id              TEXT PRIMARY KEY,
    source_host             TEXT NOT NULL,
    source_repo_id          TEXT,
    source_repo_display     TEXT,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq)
) STRICT;

CREATE TABLE tasks (
    task_id                 TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES projects(project_id),
    source_issue_ref        TEXT,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq)
) STRICT;

-- ---------------------------------------------------------------------------
-- Sessions and incarnations
-- ---------------------------------------------------------------------------

CREATE TABLE sessions (
    session_id              TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES projects(project_id),
    runtime_kind            TEXT NOT NULL,
    worker_kind             TEXT NOT NULL,
    display_name            TEXT,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq)
) STRICT;

CREATE TABLE session_incarnations (
    session_incarnation_id  TEXT PRIMARY KEY,
    session_id              TEXT NOT NULL REFERENCES sessions(session_id),
    generation              INTEGER NOT NULL,
    runtime_instance_ref    TEXT,
    worker_session_ref      TEXT,
    started_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    ended_event_seq         INTEGER REFERENCES events(seq),
    UNIQUE(session_id, generation)
) STRICT;

-- ---------------------------------------------------------------------------
-- Turns
-- ---------------------------------------------------------------------------

CREATE TABLE turns (
    turn_id                 TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES tasks(task_id),
    session_incarnation_id  TEXT NOT NULL REFERENCES session_incarnations(session_incarnation_id),
    worker_turn_ref         TEXT,
    turn_generation         INTEGER NOT NULL,
    lifecycle_state         TEXT NOT NULL
        CHECK(lifecycle_state IN ('running', 'completed', 'failed')),
    started_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    last_progress_at_ms     INTEGER,
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    UNIQUE(session_incarnation_id, turn_generation)
) STRICT;

-- ---------------------------------------------------------------------------
-- Private result artifacts (metadata only; the bytes live outside SQLite)
-- ---------------------------------------------------------------------------

CREATE TABLE result_artifacts (
    result_artifact_id      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES tasks(task_id),
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    storage_ref             TEXT NOT NULL UNIQUE,
    sha256_hex              TEXT NOT NULL,
    byte_len                INTEGER NOT NULL CHECK(byte_len >= 0),
    media_type              TEXT NOT NULL,
    created_at_ms           INTEGER NOT NULL,
    retention_state         TEXT NOT NULL
        CHECK(retention_state IN ('pinned', 'eligible')),
    eligible_for_delete_at_ms INTEGER
) STRICT;

-- ---------------------------------------------------------------------------
-- Obligations
-- ---------------------------------------------------------------------------

CREATE TABLE obligations (
    obligation_id           TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES tasks(task_id),
    turn_id                 TEXT REFERENCES turns(turn_id),
    result_artifact_id      TEXT REFERENCES result_artifacts(result_artifact_id),
    obligation_kind         TEXT NOT NULL CHECK(obligation_kind IN ('worker_turn')),
    state                   TEXT NOT NULL CHECK(state IN (
                                'created', 'running', 'needs_input', 'failed',
                                'completed_unprocessed', 'claimed_by_foreman',
                                'processing', 'acknowledged', 'cancelled_by_user',
                                'superseded')),
    priority                INTEGER NOT NULL,

    -- DEVIATION: split from the document's single `source_event_seq`.
    -- `created_event_seq` is the event that created the obligation;
    -- `source_event_seq` is the event carrying the source fact the obligation
    -- currently stands on, which is what a wake snapshot and an ACK fence
    -- compare against and which advances with each accepted worker event.
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),

    current_version         INTEGER NOT NULL,
    current_binding_generation INTEGER,
    current_claim_id        TEXT,

    -- DEVIATION: two more projected fields the document's table cannot hold.
    -- Both are read back by callers and compared by replay verification; the
    -- obligation's *fenced* state is never read from this row at all, it is
    -- folded from the ledger (see `crate::load`).
    incarnation_generation  INTEGER NOT NULL,
    -- No foreign key: `input_requests.obligation_id` already points here, and
    -- SQLite cannot resolve the resulting cycle without deferred constraints.
    input_request_id        TEXT,

    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    closed_event_seq        INTEGER REFERENCES events(seq)
) STRICT;

CREATE INDEX obligations_open_by_task
    ON obligations(task_id)
    WHERE closed_event_seq IS NULL;

CREATE TABLE obligation_events (
    seq                     INTEGER PRIMARY KEY AUTOINCREMENT,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    -- The version the transition produced. Replay checks it, so a projection
    -- that drifted from its ledger is detected rather than trusted.
    obligation_version      INTEGER NOT NULL,
    event_seq               INTEGER NOT NULL UNIQUE REFERENCES events(seq),
    from_state              TEXT,
    to_state                TEXT NOT NULL,
    disposition             TEXT,
    actor_class             TEXT NOT NULL
        CHECK(actor_class IN ('worker', 'foreman', 'user', 'daemon')),
    binding_generation      INTEGER,
    claim_id                TEXT,
    UNIQUE(obligation_id, obligation_version)
) STRICT;

-- ---------------------------------------------------------------------------
-- Foreman bindings
-- ---------------------------------------------------------------------------

CREATE TABLE foreman_bindings (
    foreman_binding_id      TEXT PRIMARY KEY,
    provider                TEXT NOT NULL,
    canonical_conversation_id TEXT NOT NULL,
    canonical_conversation_url TEXT NOT NULL,
    browser_profile_id      TEXT NOT NULL,
    binding_generation      INTEGER NOT NULL UNIQUE,
    connector_abi           TEXT NOT NULL,
    capability_epoch        INTEGER NOT NULL,
    write_capability_state  TEXT NOT NULL CHECK(write_capability_state IN (
                                'unknown', 'proven', 'read_fetch_only_unsupported',
                                'lost', 'blocked_by_confirmation')),
    is_active               INTEGER NOT NULL CHECK(is_active IN (0, 1)),
    bound_event_seq         INTEGER NOT NULL REFERENCES events(seq),
    superseded_event_seq    INTEGER REFERENCES events(seq)
) STRICT;

CREATE UNIQUE INDEX one_active_foreman_binding
    ON foreman_bindings(is_active)
    WHERE is_active = 1;

-- ---------------------------------------------------------------------------
-- Browser wake deliveries
-- ---------------------------------------------------------------------------

CREATE TABLE browser_deliveries (
    delivery_id             TEXT PRIMARY KEY,
    delivery_key            TEXT NOT NULL UNIQUE,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    target_obligation_version INTEGER NOT NULL,
    target_source_event_seq INTEGER NOT NULL REFERENCES events(seq),
    foreman_binding_id      TEXT NOT NULL REFERENCES foreman_bindings(foreman_binding_id),
    binding_generation      INTEGER NOT NULL,
    delivery_revision       INTEGER NOT NULL,
    -- DEVIATION: the bounded retry budget is part of the delivery's durable
    -- identity. Without it the attempt machine cannot be rebuilt: replay would
    -- have to guess the budget the revision was created with.
    attempt_budget          INTEGER NOT NULL CHECK(attempt_budget > 0),
    wake_protocol           TEXT NOT NULL,
    wake_payload_digest     TEXT NOT NULL,
    state                   TEXT NOT NULL CHECK(state IN (
                                'pending', 'claimed', 'accepted', 'failed', 'ambiguous')),
    accepted_message_ref    TEXT,
    accepted_event_seq      INTEGER REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(obligation_id, binding_generation, delivery_revision)
) STRICT;

CREATE TABLE delivery_attempts (
    delivery_attempt_id     TEXT PRIMARY KEY,
    delivery_id             TEXT NOT NULL REFERENCES browser_deliveries(delivery_id),
    attempt_no              INTEGER NOT NULL,
    state                   TEXT NOT NULL CHECK(state IN (
                                'claimed', 'activation_armed', 'accepted',
                                'failed', 'ambiguous')),
    claimed_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    -- Non-null means this attempt crossed the Send ambiguity fence, which stays
    -- true after the attempt reaches a terminal state.
    activation_armed_event_seq INTEGER REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    started_at_ms           INTEGER NOT NULL,
    finished_at_ms          INTEGER,
    failure_class           TEXT,
    evidence_class          TEXT,
    UNIQUE(delivery_id, attempt_no)
) STRICT;

-- ---------------------------------------------------------------------------
-- Foreman physical turns (observational: nothing here closes an obligation)
-- ---------------------------------------------------------------------------

CREATE TABLE foreman_turns (
    foreman_turn_id         TEXT PRIMARY KEY,
    foreman_binding_id      TEXT NOT NULL REFERENCES foreman_bindings(foreman_binding_id),
    binding_generation      INTEGER NOT NULL,
    trigger_delivery_id     TEXT REFERENCES browser_deliveries(delivery_id),
    provider_turn_ref       TEXT,
    state                   TEXT NOT NULL CHECK(state IN (
                                'idle_unknown', 'starting', 'active', 'settled',
                                'observation_lost')),
    started_event_seq       INTEGER REFERENCES events(seq),
    settled_event_seq       INTEGER REFERENCES events(seq),
    latest_event_seq        INTEGER NOT NULL REFERENCES events(seq)
) STRICT;

-- ---------------------------------------------------------------------------
-- Foreman claims
-- ---------------------------------------------------------------------------

CREATE TABLE foreman_claims (
    claim_id                TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    obligation_version_at_claim INTEGER NOT NULL,
    binding_generation      INTEGER NOT NULL,
    wake_delivery_id        TEXT NOT NULL REFERENCES browser_deliveries(delivery_id),
    state                   TEXT NOT NULL CHECK(state IN ('live', 'expired', 'closed')),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    expires_at_ms           INTEGER NOT NULL,
    released_event_seq      INTEGER REFERENCES events(seq),
    closed_event_seq        INTEGER REFERENCES events(seq)
) STRICT;

-- ---------------------------------------------------------------------------
-- Input requests (what kind of input is owed, never the question itself)
-- ---------------------------------------------------------------------------

CREATE TABLE input_requests (
    input_request_id        TEXT PRIMARY KEY,
    obligation_id           TEXT NOT NULL REFERENCES obligations(obligation_id),
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    native_input_ref        TEXT,
    request_kind            TEXT NOT NULL CHECK(request_kind IN (
                                'engineering_question', 'user_owned_decision',
                                'runtime_input', 'provider_elicitation')),
    authorization_class     TEXT NOT NULL CHECK(authorization_class IN (
                                'delegated_engineering', 'user_owned')),
    answer_shape            TEXT NOT NULL,
    request_revision        INTEGER NOT NULL,
    state                   TEXT NOT NULL CHECK(state IN (
                                'pending', 'answered', 'resolved', 'cancelled')),
    answered_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(turn_id, source_event_seq, request_revision)
) STRICT;

-- ---------------------------------------------------------------------------
-- Worker answer/resume delivery
-- ---------------------------------------------------------------------------

CREATE TABLE worker_commands (
    worker_command_id       TEXT PRIMARY KEY,
    input_request_id        TEXT REFERENCES input_requests(input_request_id),
    session_incarnation_id  TEXT NOT NULL REFERENCES session_incarnations(session_incarnation_id),
    answer_event_seq        INTEGER REFERENCES events(seq),
    command_kind            TEXT NOT NULL CHECK(command_kind IN ('answer_input', 'resume')),
    command_revision        INTEGER NOT NULL,
    attempt_budget          INTEGER NOT NULL CHECK(attempt_budget > 0),
    state                   TEXT NOT NULL CHECK(state IN (
                                'pending', 'claimed', 'accepted', 'failed', 'ambiguous')),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    UNIQUE(input_request_id, command_revision)
) STRICT;

CREATE TABLE worker_command_attempts (
    worker_command_attempt_id TEXT PRIMARY KEY,
    worker_command_id       TEXT NOT NULL REFERENCES worker_commands(worker_command_id),
    attempt_no              INTEGER NOT NULL,
    state                   TEXT NOT NULL CHECK(state IN (
                                'claimed', 'activation_armed', 'accepted',
                                'failed', 'ambiguous')),
    claimed_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    -- DEVIATION: the document leaves these two without a foreign key; every
    -- other `*_event_seq` in the schema has one, and a dangling sequence here
    -- would break replay silently.
    ambiguity_armed_event_seq INTEGER REFERENCES events(seq),
    terminal_event_seq      INTEGER REFERENCES events(seq),
    started_at_ms           INTEGER NOT NULL,
    finished_at_ms          INTEGER,
    failure_class           TEXT,
    evidence_class          TEXT,
    UNIQUE(worker_command_id, attempt_no)
) STRICT;

-- ---------------------------------------------------------------------------
-- Progress heartbeats (identity, time and safe class only)
-- ---------------------------------------------------------------------------

CREATE TABLE progress_heartbeats (
    progress_id             TEXT PRIMARY KEY,
    turn_id                 TEXT NOT NULL REFERENCES turns(turn_id),
    source_event_seq        INTEGER NOT NULL UNIQUE REFERENCES events(seq),
    occurred_at_ms          INTEGER NOT NULL,
    safe_event_class        TEXT NOT NULL
) STRICT;

CREATE INDEX progress_by_turn_time
    ON progress_heartbeats(turn_id, occurred_at_ms DESC);

-- ---------------------------------------------------------------------------
-- Mutation-command journal
--
-- DEVIATION: whole table. It is the SQLite form of the Prime-Agent-style
-- command journal in docs/research/2026-08-31-durable-orchestration-pattern-review.md
-- ("`governor-store-sqlite`"), which docs/data-model.md predates. One authority:
-- the journal lives here, never in a second log file.
-- ---------------------------------------------------------------------------

CREATE TABLE mutation_commands (
    actor_id                TEXT NOT NULL,
    command_id              TEXT NOT NULL,

    -- DEVIATION beyond the research doc's conceptual table. "Exact retry" has
    -- to mean exact: without a fingerprint, a client reusing a command id for a
    -- *different* operation would silently receive the first operation's
    -- recorded result. A mismatch is typed `mutation_command_mismatch`, never a
    -- replayed result. It is a digest of the fenced parameters, never the
    -- parameters.
    fingerprint             TEXT NOT NULL,
    command_kind            TEXT NOT NULL,
    status                  TEXT NOT NULL CHECK(status IN (
                                'received', 'completed', 'uncertain', 'acked')),

    -- DEVIATION: the research doc's `safe_result_blob_or_ref` is split and
    -- narrowed. There is no blob: a bounded result is one of three shapes, and
    -- the only variable parts are one opaque token or one stable conflict code.
    -- A column that could hold an arbitrary response body would be a place for
    -- prompts, tool output and credentials to accumulate.
    safe_result_kind        TEXT CHECK(safe_result_kind IS NULL OR safe_result_kind IN (
                                'applied', 'already_satisfied', 'refused')),
    safe_result_ref         TEXT,
    safe_result_conflict    TEXT,

    daemon_epoch            INTEGER NOT NULL,
    created_at_ms           INTEGER NOT NULL,
    completed_at_ms         INTEGER,
    -- DEVIATION: the pure `MutationCommand` records when it was found uncertain,
    -- and replay must reproduce that field exactly.
    uncertain_at_ms         INTEGER,
    acked_at_ms             INTEGER,

    -- A committed result and its status cannot disagree.
    CHECK((status IN ('completed', 'acked')) = (safe_result_kind IS NOT NULL)),
    CHECK((safe_result_kind = 'refused') = (safe_result_conflict IS NOT NULL)),
    CHECK(safe_result_ref IS NULL OR safe_result_kind = 'applied'),
    CHECK((status = 'acked') = (acked_at_ms IS NOT NULL)),

    PRIMARY KEY(actor_id, command_id)
) STRICT;

-- Startup recovery asks exactly one question: which rows from an older daemon
-- epoch are still `received`?
CREATE INDEX mutation_commands_unresolved
    ON mutation_commands(daemon_epoch)
    WHERE status = 'received';

-- Compaction asks the other one: which acked rows are old enough to drop?
CREATE INDEX mutation_commands_compactable
    ON mutation_commands(acked_at_ms)
    WHERE status = 'acked';

-- ---------------------------------------------------------------------------
-- Consequential external effects
--
-- DEVIATION: whole table, from the same research doc. "External attempts remain
-- a separate domain table because command delivery and the external side effect
-- are different facts."
--
-- The source fence is the `SourceRef` triple rather than an `events(seq)`: an
-- intent is committed in its own transaction *before* any I/O, and the fact that
-- justified it is not necessarily an event this ledger holds.
-- ---------------------------------------------------------------------------

CREATE TABLE external_attempts (
    external_attempt_id     TEXT PRIMARY KEY,

    effect_class            TEXT NOT NULL CHECK(effect_class IN (
                                'read', 'idempotent_write', 'non_idempotent_write')),
    -- The destination's documented mechanism plus the exact key it keys on.
    -- Both are recorded or neither is: a retry may only rest on a recorded
    -- contract, never on a label somebody attached at the call site.
    idempotency_contract    TEXT CHECK(idempotency_contract IS NULL OR idempotency_contract IN (
                                'deduplicated_by_key', 'conditional_on_destination_fence')),
    idempotency_window_ms   INTEGER,
    idempotency_key         TEXT,

    destination_namespace   TEXT NOT NULL,
    destination_endpoint    TEXT NOT NULL,
    destination_fence       TEXT NOT NULL,

    source_namespace        TEXT NOT NULL,
    source_event_id         TEXT NOT NULL,
    source_event_fence      TEXT NOT NULL,

    daemon_epoch            INTEGER NOT NULL,
    state                   TEXT NOT NULL CHECK(state IN (
                                'intent_recorded', 'completed',
                                'failed_before_effect', 'ambiguous')),
    -- The dispatch fence, committed immediately before the adapter issues the
    -- call. It stays true after the attempt reaches a terminal state.
    dispatched              INTEGER NOT NULL CHECK(dispatched IN (0, 1)),

    completion_ref          TEXT,
    no_effect_class         TEXT CHECK(no_effect_class IS NULL OR no_effect_class IN (
                                'not_attempted', 'rejected_before_dispatch',
                                'destination_refused_without_applying',
                                'precondition_rejected_at_destination')),
    ambiguity_reason        TEXT CHECK(ambiguity_reason IS NULL OR ambiguity_reason IN (
                                'orphaned_by_restart', 'response_lost',
                                'deadline_elapsed', 'evidence_inconclusive')),

    recorded_at_ms          INTEGER NOT NULL,
    dispatched_at_ms        INTEGER,
    finished_at_ms          INTEGER,

    CHECK((effect_class = 'idempotent_write') = (idempotency_contract IS NOT NULL)),
    CHECK((effect_class = 'idempotent_write') = (idempotency_key IS NOT NULL)),
    CHECK((idempotency_contract = 'deduplicated_by_key') = (idempotency_window_ms IS NOT NULL)),
    CHECK((state = 'failed_before_effect') = (no_effect_class IS NOT NULL)),
    CHECK((state = 'ambiguous') = (ambiguity_reason IS NOT NULL)),
    CHECK((state = 'intent_recorded') = (finished_at_ms IS NULL)),
    CHECK(dispatched = (dispatched_at_ms IS NOT NULL)),
    -- An effect that was never dispatched cannot have landed.
    CHECK(state != 'completed' OR dispatched = 1)
) STRICT;

-- Startup recovery reads exactly this slice: previous-epoch attempts with a
-- durable intent and no proven outcome.
CREATE INDEX external_attempts_unresolved
    ON external_attempts(daemon_epoch)
    WHERE state = 'intent_recorded';

-- ---------------------------------------------------------------------------
-- Resource leases
--
-- DEVIATION: whole table, from the research doc's "resource ownership". It is
-- deliberately small. The global daemon/state-root lock is *not* a lease
-- (docs/research: "for V1, the global daemon/state-root lock remains simpler
-- than a distributed lease"); this table exists only for resources where a
-- second process legitimately participates, and it holds exactly what
-- `ResourceOwnership` holds: one resource, and the most recent lease over it,
-- kept after release so a superseded holder can still be told why it lost.
-- ---------------------------------------------------------------------------

CREATE TABLE resource_leases (
    resource_namespace      TEXT NOT NULL,
    -- The canonical resource *name* is never stored: a path, a socket location
    -- or a profile directory is forbidden durable control-plane data, so the
    -- identity is a namespace plus the digest of that name.
    resource_digest         TEXT NOT NULL,

    resource_lease_id       TEXT NOT NULL UNIQUE,
    -- A possession fence, stored as raw bytes. There is deliberately no text
    -- form, so it cannot reach a log line by way of a formatter.
    lease_token             BLOB NOT NULL,
    holder_actor_id         TEXT NOT NULL,
    -- The process number alone is never an identity: a recycled number paired
    -- with a different start reference is a different incarnation.
    process_slot            INTEGER NOT NULL,
    process_start_ref       TEXT NOT NULL,
    daemon_epoch            INTEGER NOT NULL,

    state                   TEXT NOT NULL CHECK(state IN ('held', 'released')),
    acquired_at_ms          INTEGER NOT NULL,
    renewed_at_ms           INTEGER NOT NULL,
    expires_at_ms           INTEGER NOT NULL,
    released_at_ms          INTEGER,

    CHECK((state = 'released') = (released_at_ms IS NOT NULL)),
    CHECK(length(lease_token) = 32),

    PRIMARY KEY(resource_namespace, resource_digest)
) STRICT;

-- ---------------------------------------------------------------------------
-- Health / reconciliation conditions
-- ---------------------------------------------------------------------------

CREATE TABLE health_conditions (
    health_condition_id     TEXT PRIMARY KEY,
    kind                    TEXT NOT NULL CHECK(kind IN (
                                'suspected_stall', 'foreman_unreachable',
                                'mcp_write_capability_missing', 'browser_binding_displaced',
                                'result_artifact_missing', 'projection_mismatch',
                                'runtime_state_conflict', 'input_detail_unavailable',
                                'worker_defer_shape_unsupported',
                                -- DEVIATION: added kind. An external attempt with a
                                -- durable intent and no proven outcome needs a durable
                                -- attention record; see `external_attempts` below.
                                'reconciliation_required')),
    state                   TEXT NOT NULL CHECK(state IN ('open', 'resolved')),
    task_id                 TEXT REFERENCES tasks(task_id),
    turn_id                 TEXT REFERENCES turns(turn_id),
    obligation_id           TEXT REFERENCES obligations(obligation_id),
    -- DEVIATION: added scope column, matching the new `HealthScope` field. Scope
    -- is part of a condition's identity, so without it two ambiguous attempts
    -- would collapse onto one condition.
    external_attempt_id     TEXT REFERENCES external_attempts(external_attempt_id),
    opened_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    resolved_event_seq      INTEGER REFERENCES events(seq)
) STRICT;

CREATE INDEX health_conditions_open
    ON health_conditions(kind)
    WHERE state = 'open';

-- One open condition per (kind, scope): the durable half of the deduplication
-- `HealthLedger::raise` performs in memory. NULLs do not compare equal in a
-- SQLite unique index, so the scope columns are coalesced to a sentinel.
CREATE UNIQUE INDEX health_conditions_one_open_per_scope
    ON health_conditions(
        kind,
        COALESCE(task_id, ''),
        COALESCE(turn_id, ''),
        COALESCE(obligation_id, ''),
        COALESCE(external_attempt_id, '')
    )
    WHERE state = 'open';
