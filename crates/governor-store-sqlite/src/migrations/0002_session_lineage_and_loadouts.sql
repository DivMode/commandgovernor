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
