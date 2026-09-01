-- Command Governor session lineage and worker loadouts (epoch 2).
--
-- Follows docs/adr/0007-session-lineage-memory-and-analytics.md §4–§7 and
-- docs/data-model.md. Every deviation from those documents is marked
-- `DEVIATION:` with the reason.
--
-- Conventions are 0001's: STRICT tables, canonical UUID text identities, Unix
-- millisecond instants, `*_event_seq INTEGER REFERENCES events(seq)`, and a
-- CHECK against the closed label set on every state column.
--
-- DEVIATION: digests are stored as lowercase hex TEXT with CHECK(length = 64),
-- matching `result_artifacts.sha256_hex` and the existing `codec::hex32` /
-- `codec::parse_hex32` helpers. `resource_leases.lease_token` is the schema's
-- only BLOB, and only because it is a possession secret with deliberately no
-- text form. A digest is not a secret.

-- ---------------------------------------------------------------------------
-- Health conditions: widen the kind set and add the session scope
--
-- SQLite cannot ALTER a CHECK constraint, so the table is rebuilt. Both indexes
-- are recreated below; losing `health_conditions_one_open_per_scope` would
-- silently remove the one-open-per-(kind, scope) guarantee that the durable
-- half of `HealthLedger::raise` rests on.
--
-- DEVIATION: added scope column `session_id`, matching the new `HealthScope`
-- field. A session's launch loadout, managed configuration and lineage are
-- facts about the session; without its own column two sessions' conditions
-- would collapse onto one, because scope is part of a condition's identity.
-- ---------------------------------------------------------------------------

CREATE TABLE health_conditions_rebuilt (
    health_condition_id     TEXT PRIMARY KEY,
    kind                    TEXT NOT NULL CHECK(kind IN (
                                'suspected_stall', 'foreman_unreachable',
                                'mcp_write_capability_missing', 'browser_binding_displaced',
                                'result_artifact_missing', 'projection_mismatch',
                                'runtime_state_conflict', 'input_detail_unavailable',
                                'worker_defer_shape_unsupported',
                                'reconciliation_required',
                                -- Added at epoch 2, all session-scoped.
                                'loadout_unverifiable', 'managed_config_missing',
                                'lineage_broken')),
    state                   TEXT NOT NULL CHECK(state IN ('open', 'resolved')),
    task_id                 TEXT REFERENCES tasks(task_id),
    session_id              TEXT REFERENCES sessions(session_id),
    turn_id                 TEXT REFERENCES turns(turn_id),
    obligation_id           TEXT REFERENCES obligations(obligation_id),
    external_attempt_id     TEXT REFERENCES external_attempts(external_attempt_id),
    opened_event_seq        INTEGER NOT NULL REFERENCES events(seq),
    resolved_event_seq      INTEGER REFERENCES events(seq)
) STRICT;

-- Every epoch-1 condition carries a NULL session scope: none of the epoch-1
-- kinds is about a session, so this is a widening and never a reinterpretation.
INSERT INTO health_conditions_rebuilt (
        health_condition_id, kind, state, task_id, session_id, turn_id,
        obligation_id, external_attempt_id, opened_event_seq, resolved_event_seq)
SELECT  health_condition_id, kind, state, task_id, NULL, turn_id,
        obligation_id, external_attempt_id, opened_event_seq, resolved_event_seq
  FROM health_conditions;

DROP TABLE health_conditions;
ALTER TABLE health_conditions_rebuilt RENAME TO health_conditions;

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
        COALESCE(session_id, ''),
        COALESCE(turn_id, ''),
        COALESCE(obligation_id, ''),
        COALESCE(external_attempt_id, '')
    )
    WHERE state = 'open';

-- ---------------------------------------------------------------------------
-- Immutable capability profiles (whitelist-only; an empty profile grants
-- nothing, which is why "omitted" cannot silently mean "all")
--
-- The contents digest is part of the primary key: the same logical profile id
-- with different contents is a DIFFERENT immutable snapshot, never an update.
-- There is no UPDATE statement for this table anywhere in the crate, so an
-- edited role file produces a second row and every loadout that embedded the
-- first one keeps pointing at the first one.
-- ---------------------------------------------------------------------------

CREATE TABLE capability_profiles (
    capability_profile_id   TEXT NOT NULL,
    digest_hex              TEXT NOT NULL CHECK(length(digest_hex) = 64),
    capability_count        INTEGER NOT NULL CHECK(capability_count >= 0),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    PRIMARY KEY(capability_profile_id, digest_hex)
) STRICT;

-- One row per explicitly granted name. An empty profile has zero rows.
CREATE TABLE capability_profile_entries (
    capability_profile_id   TEXT NOT NULL,
    digest_hex              TEXT NOT NULL,
    capability_name         TEXT NOT NULL,
    PRIMARY KEY(capability_profile_id, digest_hex, capability_name),
    FOREIGN KEY(capability_profile_id, digest_hex)
        REFERENCES capability_profiles(capability_profile_id, digest_hex)
) STRICT;

-- ---------------------------------------------------------------------------
-- Immutable recursive-delegation policies
--
-- Same shape and same reasoning: an empty policy permits no child role, and a
-- widened policy is a new snapshot rather than an edit.
-- ---------------------------------------------------------------------------

CREATE TABLE delegation_policies (
    delegation_policy_id    TEXT NOT NULL,
    digest_hex              TEXT NOT NULL CHECK(length(digest_hex) = 64),
    allowed_role_count      INTEGER NOT NULL CHECK(allowed_role_count >= 0),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    PRIMARY KEY(delegation_policy_id, digest_hex)
) STRICT;

CREATE TABLE delegation_policy_entries (
    delegation_policy_id    TEXT NOT NULL,
    digest_hex              TEXT NOT NULL,
    allowed_role            TEXT NOT NULL,
    PRIMARY KEY(delegation_policy_id, digest_hex, allowed_role),
    FOREIGN KEY(delegation_policy_id, digest_hex)
        REFERENCES delegation_policies(delegation_policy_id, digest_hex)
) STRICT;

-- ---------------------------------------------------------------------------
-- Immutable model policies
--
-- DEVIATION: identity and digest only, with no contents table. A model-policy
-- body is adapter configuration resolved outside this crate, exactly as
-- `ModelPolicyRef` models it; inventing a contents shape here would create a
-- second place for it to be defined and therefore a second place to disagree.
-- ---------------------------------------------------------------------------

CREATE TABLE model_policies (
    model_policy_id         TEXT NOT NULL,
    digest_hex              TEXT NOT NULL CHECK(length(digest_hex) = 64),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    PRIMARY KEY(model_policy_id, digest_hex)
) STRICT;

-- ---------------------------------------------------------------------------
-- Private immutable managed-configuration artifacts
--
-- Shaped exactly like `result_artifacts`: the bytes live in the artifact root,
-- this row is metadata only, and `storage_ref` is the daemon-allocated opaque
-- storage key. A worker never supplies a path and none is representable.
--
-- DEVIATION: `retention_state` is present for shape-compatibility with
-- `result_artifacts` but is pinned unconditionally at epoch 2. A managed
-- configuration is pinned by any loadout any session was ever launched under,
-- and Slice 2 defines no releaser for that relation, so releasing one would be
-- a guess. `eligible_for_delete_at_ms` is therefore always NULL, and
-- `replay::compare_config_retention` asserts exactly that rather than leaving
-- it as an undocumented habit. Reclaiming configurations is a bounded
-- follow-up, and it will arrive as a migration that defines the releaser.
-- ---------------------------------------------------------------------------

CREATE TABLE managed_config_artifacts (
    managed_config_artifact_id TEXT PRIMARY KEY,
    storage_ref             TEXT NOT NULL UNIQUE,
    sha256_hex              TEXT NOT NULL CHECK(length(sha256_hex) = 64),
    byte_len                INTEGER NOT NULL CHECK(byte_len >= 0),
    media_type              TEXT NOT NULL,
    hook_contract_epoch     INTEGER NOT NULL CHECK(hook_contract_epoch >= 0),
    created_at_ms           INTEGER NOT NULL,
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),
    retention_state         TEXT NOT NULL
        CHECK(retention_state IN ('pinned', 'eligible')),
    eligible_for_delete_at_ms INTEGER,
    -- The epoch-2 retention rule, restated so a row that bypassed the writer
    -- still cannot exist.
    CHECK(retention_state = 'pinned'),
    CHECK(eligible_for_delete_at_ms IS NULL)
) STRICT;

-- ---------------------------------------------------------------------------
-- Immutable resolved worker loadouts
-- ---------------------------------------------------------------------------

CREATE TABLE worker_loadouts (
    worker_loadout_id       TEXT NOT NULL,
    -- The canonical digest from `WorkerLoadout::resolve`, in the primary key
    -- for the same reason as the profiles above: an edited loadout is a new
    -- snapshot, never an UPDATE.
    digest_hex              TEXT NOT NULL CHECK(length(digest_hex) = 64),

    worker_kind             TEXT NOT NULL,
    runtime_kind            TEXT NOT NULL,
    role                    TEXT NOT NULL,

    model_policy_id         TEXT NOT NULL,
    model_policy_digest_hex TEXT NOT NULL CHECK(length(model_policy_digest_hex) = 64),
    capability_profile_id   TEXT NOT NULL,
    capability_profile_digest_hex TEXT NOT NULL
        CHECK(length(capability_profile_digest_hex) = 64),
    delegation_policy_id    TEXT NOT NULL,
    delegation_policy_digest_hex TEXT NOT NULL
        CHECK(length(delegation_policy_digest_hex) = 64),
    managed_config_artifact_id TEXT NOT NULL
        REFERENCES managed_config_artifacts(managed_config_artifact_id),
    managed_config_digest_hex TEXT NOT NULL CHECK(length(managed_config_digest_hex) = 64),
    managed_config_byte_len INTEGER NOT NULL CHECK(managed_config_byte_len >= 0),

    hook_contract_epoch     INTEGER NOT NULL CHECK(hook_contract_epoch >= 0),
    resume_policy           TEXT NOT NULL CHECK(resume_policy IN ('exact_loadout')),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),

    PRIMARY KEY(worker_loadout_id, digest_hex),
    -- The composite foreign keys are what make "the digest a loadout embeds
    -- must name a profile snapshot that actually exists" a database fact rather
    -- than a code convention. A widened profile written under the same id has a
    -- different digest and therefore does not satisfy the old loadout's key.
    FOREIGN KEY(capability_profile_id, capability_profile_digest_hex)
        REFERENCES capability_profiles(capability_profile_id, digest_hex),
    FOREIGN KEY(delegation_policy_id, delegation_policy_digest_hex)
        REFERENCES delegation_policies(delegation_policy_id, digest_hex),
    FOREIGN KEY(model_policy_id, model_policy_digest_hex)
        REFERENCES model_policies(model_policy_id, digest_hex)
) STRICT;

-- ---------------------------------------------------------------------------
-- The loadout one logical session incarnation was launched under
--
-- DEVIATION from putting these columns on `sessions`: the binding is per
-- (session, incarnation). A resume that legitimately produces a new loadout
-- revision starts a new incarnation rather than mutating a row, which is what
-- makes "resume cannot widen the sandbox" a schema property rather than a
-- code one. A separate table also keeps `sessions` unchanged, so migration
-- 0002 stays additive for every epoch-1 table but `health_conditions`.
-- ---------------------------------------------------------------------------

CREATE TABLE session_loadouts (
    session_id              TEXT NOT NULL REFERENCES sessions(session_id),
    session_incarnation_id  TEXT NOT NULL
        REFERENCES session_incarnations(session_incarnation_id),
    worker_loadout_id       TEXT NOT NULL,
    digest_hex              TEXT NOT NULL,
    bound_event_seq         INTEGER NOT NULL REFERENCES events(seq),
    -- One loadout per incarnation, forever.
    PRIMARY KEY(session_incarnation_id),
    UNIQUE(session_id, session_incarnation_id),
    FOREIGN KEY(worker_loadout_id, digest_hex)
        REFERENCES worker_loadouts(worker_loadout_id, digest_hex)
) STRICT;

CREATE INDEX session_loadouts_by_session
    ON session_loadouts(session_id, bound_event_seq);

-- ---------------------------------------------------------------------------
-- Durable logical session lineage
--
-- DEVIATION: ADR 0007 §5 sketches the relation without a key; this makes the
-- child the primary key, so a logical session has exactly one logical parent.
-- Every `SessionRelation` variant, `provider_fork` included, is single-parent,
-- and one parent per child is what reduces cycle detection from a search of a
-- general graph to a bounded upward walk of a chain. A future multi-parent DAG
-- is therefore a deliberate migration rather than a silent widening.
-- ---------------------------------------------------------------------------

CREATE TABLE session_edges (
    parent_session_id       TEXT NOT NULL REFERENCES sessions(session_id),
    child_session_id        TEXT NOT NULL REFERENCES sessions(session_id),
    -- Necessary but not sufficient for ownership: `turns` records the
    -- incarnation, not the session, so proving the turn belongs to the parent
    -- session is a two-hop join the store performs inside the insert
    -- transaction. See `ops::session::require_parent_turn_ownership`.
    parent_turn_id          TEXT NOT NULL REFERENCES turns(turn_id),
    relation_kind           TEXT NOT NULL CHECK(relation_kind IN (
                                'delegated_worker', 'scout', 'researcher',
                                'reviewer', 'observer', 'consolidator',
                                'provider_fork')),
    created_event_seq       INTEGER NOT NULL REFERENCES events(seq),

    -- The self-parent half of `SessionEdge::new`, restated durably so a row
    -- that bypassed the constructor still cannot exist. The multi-hop half
    -- cannot be a CHECK and lives in the insert transaction instead.
    CHECK(parent_session_id != child_session_id),

    PRIMARY KEY(child_session_id)
) STRICT;

CREATE INDEX session_edges_by_parent
    ON session_edges(parent_session_id, created_event_seq);

CREATE INDEX session_edges_by_parent_turn
    ON session_edges(parent_turn_id);
