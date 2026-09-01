//! Session lineage and worker-loadout acceptance tests: SES-001 … SES-006.
//!
//! # Coverage
//!
//! | Test | Issue #4 acceptance item | Status |
//! | --- | --- | --- |
//! | [`ses_001_resume_requires_the_exact_launch_loadout`] | 1 — resume uses the exact validated loadout | covered here, across a restart, including the in-transaction fence re-check |
//! | [`ses_002_missing_or_corrupt_managed_config_refuses_resume`] | 2 — a missing or corrupt config fails closed | covered here, both arms, with the durable condition and zero mutation |
//! | [`ses_003_widened_role_definition_cannot_broaden_a_resumed_child`] | 3 — a changed role definition cannot widen a resumed child | covered here, at the schema level: two snapshots under one identity |
//! | [`ses_004_lineage_survives_daemon_restart`] | 4 — lineage survives daemon and runtime restart | covered here, over 100 restarts, with `compare_lineage` clean each time |
//! | [`ses_005_multi_hop_lineage_cycle_fails_closed`] | 4 (integrity) | covered here, including the one-hop case the pure constructor owns |
//! | [`ses_006_parent_turn_must_belong_to_the_parent_session`] | 4 (integrity) | covered here, proving the foreign key alone is insufficient |
//!
//! Two of these are about what the store *refuses*, and both use
//! [`assert_unchanged`] rather than a count in the one table the test happened
//! to think about: a refusal that advanced a version, stamped a retention
//! instant or wrote an event would pass a narrower check.

use std::collections::BTreeSet;

use governor_core::artifact::ArtifactDigest;
use governor_core::error::ConflictKind;
use governor_core::id::SessionId;
use governor_core::session::{
    CommittedLoadout, ManagedConfigDigest, ManagedConfigVerified, SessionRelation,
};
use governor_daemon::worker::ResumeRefusal;
use governor_store_sqlite::SessionHealthRequest;
use governor_testkit::dump::{assert_unchanged, count, dump_domain, scalar};
use governor_testkit::harness::Harness;
use governor_testkit::restart::restart_loop;
use governor_testkit::scenario::{
    MANAGED_CONFIG, authorize_resume, bind_loadout, capabilities_of, open_named_turn, open_turn,
    profile_digest_hex, publish_managed_config, record_lineage, resolve_loadout, spawn_child,
};

// --- SES-001: resume requires the exact validated launch loadout --------------

#[test]
fn ses_001_resume_requires_the_exact_launch_loadout() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    let parent = open_turn(&store);
    let child = spawn_child(
        &store,
        &mut artifacts,
        &parent,
        SessionRelation::Scout,
        1,
        &["read"],
    );
    let launched = child.fence();
    drop(store);

    // A new process: the launch snapshot has to come back out of the database,
    // not out of anything the first process kept in memory.
    let store = harness.open().expect("reopen");
    let artifacts = harness.open_artifacts();
    let record = store
        .read_session_loadout(child.session.incarnation)
        .expect("reading the binding")
        .expect("the incarnation is bound");
    assert_eq!(record.session, child.session.session);
    assert_eq!(record.persisted.digest, launched.digest());

    // `rehydrate` is the only path from persisted parts to a value that can
    // admit a resume, and it re-derives the digest rather than trusting it.
    let committed = CommittedLoadout::rehydrate(record.persisted.clone())
        .expect("the launch row proves itself");
    assert_eq!(committed.fence(), launched);

    let before = dump_domain(&harness.inspect());
    let authorized =
        authorize_resume(&store, &artifacts, &child, launched).expect("the exact fence resumes");
    assert_eq!(authorized.fence(), launched);

    // The permits exist, and they are the only way to reach an adapter.
    let spawned = authorized.spawn_with(|_permit, resume| resume.fence());
    assert_eq!(spawned, launched);

    // Resuming recorded a durable intent and changed nothing about the loadout
    // itself: no second snapshot, no rebinding.
    let after = dump_domain(&harness.inspect());
    assert_ne!(before, after, "a resume commits a durable spawn intent");
    let conn = harness.inspect();
    assert_eq!(count(&conn, "worker_loadouts"), 1);
    assert_eq!(count(&conn, "session_loadouts"), 1);
    assert_eq!(count(&conn, "external_attempts"), 1);
    assert_eq!(
        scalar(&conn, "SELECT state FROM external_attempts").as_deref(),
        Some("intent_recorded"),
        "the intent is durable before the adapter is handed anything"
    );
    assert_eq!(
        scalar(&conn, "SELECT effect_class FROM external_attempts").as_deref(),
        Some("non_idempotent_write"),
        "a spawn is never automatically retried"
    );
    assert_eq!(
        scalar(&conn, "SELECT idempotency_contract FROM external_attempts"),
        None,
        "and carries no idempotency contract that could authorise one"
    );
    store.verify_projections().expect("replay after the resume");
}

#[test]
fn ses_001_a_binding_that_moved_under_the_verification_is_refused() {
    // The in-transaction fence re-check, on its own. Steps 2 to 4 of the resume
    // happen outside the write lock; what makes that sound is step 6 re-reading
    // the same `(loadout_id, digest_hex)` pair under the lock and refusing on
    // any difference. Presenting a fence the binding does not name is exactly
    // the state a superseding write between the two would leave behind.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    let parent = open_turn(&store);
    let child = spawn_child(
        &store,
        &mut artifacts,
        &parent,
        SessionRelation::Scout,
        1,
        &["read"],
    );

    // A second, *different* snapshot under the same logical loadout identity.
    let widened = resolve_loadout(&store, &child.fixture, &["read", "write"], &["scout"])
        .expect("a widened revision resolves");
    assert_eq!(widened.loadout, child.loadout.loadout);
    assert_ne!(widened.digest, child.loadout.digest);

    let before = dump_domain(&harness.inspect());
    let refusal = store
        .authorize_worker_spawn(governor_store_sqlite::AuthorizeWorkerSpawnRequest {
            session: child.session.session,
            incarnation: child.session.incarnation,
            verified_loadout: widened.fence(),
            verified_config: child.fixture.config,
            destination: governor_core::effect::DestinationRef::new(
                governor_testkit::scenario::token("herdr"),
                governor_testkit::scenario::token("spawn"),
                governor_testkit::scenario::token("pane.v1"),
            ),
            source: governor_testkit::scenario::source("cg.internal", "resume-2", "spawn"),
            daemon_epoch: store.daemon_epoch(),
        })
        .expect_err("the binding does not name the widened snapshot");
    assert_eq!(
        refusal.conflict_code(),
        Some(ConflictKind::LoadoutDigestMismatch.code())
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-001: a refused spawn authorization commits no intent",
    );
}

// --- SES-002: a missing or corrupt managed config fails closed ----------------

#[test]
fn ses_002_missing_or_corrupt_managed_config_refuses_resume() {
    for arm in ["missing", "corrupt"] {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();

        let parent = open_turn(&store);
        let child = spawn_child(
            &store,
            &mut artifacts,
            &parent,
            SessionRelation::Researcher,
            2,
            &["read"],
        );
        let launched = child.fence();
        drop(store);

        let object = harness
            .artifact_root()
            .join("objects")
            .join(child.config.key().as_str());
        match arm {
            "missing" => std::fs::remove_file(&object).expect("losing the configuration"),
            _ => std::fs::write(&object, b"rewritten configuration\n")
                .expect("rewriting the configuration in place"),
        }

        // The ledger itself is intact, so the store still opens. A missing file
        // is not a corrupt projection.
        let store = harness.open().expect("the ledger is intact");
        let artifacts = harness.open_artifacts();
        let before = dump_domain(&harness.inspect());

        let refusal = authorize_resume(&store, &artifacts, &child, launched)
            .expect_err("an unprovable configuration refuses the resume");
        assert!(
            matches!(refusal, ResumeRefusal::ManagedConfigUnreadable(_)),
            "{arm}: {refusal:?}"
        );

        // The refusal is durable attention, scoped to the session.
        let conditions = store.open_health_conditions().expect("reading conditions");
        assert_eq!(conditions.len(), 1, "{arm}");
        assert_eq!(
            conditions[0].kind,
            governor_core::health::HealthConditionKind::ManagedConfigMissing,
            "{arm}"
        );
        assert_eq!(
            conditions[0].scope,
            governor_core::health::HealthScope::session(child.session.session),
            "{arm}"
        );

        // No permit, no intent, and nothing about the launch snapshot moved.
        let conn = harness.inspect();
        assert_eq!(count(&conn, "external_attempts"), 0, "{arm}");
        assert_eq!(count(&conn, "worker_loadouts"), 1, "{arm}");
        assert_eq!(count(&conn, "session_loadouts"), 1, "{arm}");
        assert_eq!(count(&conn, "session_edges"), 1, "{arm}");
        assert_eq!(
            count(&conn, "capability_profiles"),
            1,
            "{arm}: the launch snapshot is not replaced by a current on-disk one"
        );
        drop(conn);

        // The one row difference is the condition and its two events; the
        // durable session state is otherwise byte-identical.
        let after = dump_domain(&harness.inspect());
        assert_ne!(before, after, "{arm}: the condition is durable");
        for table in ["worker_loadouts", "session_loadouts", "session_edges"] {
            assert_eq!(
                before.get(table),
                after.get(table),
                "{arm}: `{table}` must not change when a resume is refused"
            );
        }

        // And the bytes themselves are never handed back.
        let record = store
            .read_session_loadout(child.session.incarnation)
            .expect("reading the binding")
            .expect("still bound");
        let key = governor_artifacts::StorageKey::new(record.config.storage_ref.clone())
            .expect("a valid key");
        let error = artifacts
            .read_verified(
                &key,
                ArtifactDigest::from_bytes(*record.config.digest.as_bytes()),
                record.config.byte_len,
            )
            .expect_err("{arm}: no bytes at all");
        match arm {
            "missing" => assert!(
                matches!(error, governor_artifacts::ArtifactError::Missing { .. }),
                "{error:?}"
            ),
            _ => assert!(
                matches!(error, governor_artifacts::ArtifactError::Integrity { .. }),
                "{error:?}"
            ),
        }

        store
            .verify_projections()
            .expect("replay after the refusal");
    }
}

#[test]
fn ses_002_a_witness_for_a_rewritten_config_cannot_be_minted() {
    // The falsification the corrupt arm above rests on, stated directly: the
    // *row* is unchanged when the file is rewritten, so a metadata comparison
    // passes. What refuses is the freshly computed digest, which is why the
    // resume path reads the bytes rather than the row.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (published, reference) = publish_managed_config(&store, &mut artifacts, MANAGED_CONFIG);

    // The recorded metadata still describes the original bytes.
    assert_eq!(reference.byte_len(), published.byte_len());
    let recorded_digest = ManagedConfigDigest::from_persisted(*published.digest().as_bytes());
    ManagedConfigVerified::verify(reference, recorded_digest, reference.byte_len())
        .expect("the original observation verifies");

    // A different observation of the same length does not.
    let rewritten = ManagedConfigDigest::from_persisted([0xEE; 32]);
    assert!(
        ManagedConfigVerified::verify(reference, rewritten, reference.byte_len()).is_err(),
        "a rewritten configuration must not mint a witness"
    );
}

// --- SES-003: a widened role definition cannot broaden a resumed child --------

#[test]
fn ses_003_widened_role_definition_cannot_broaden_a_resumed_child() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    let parent = open_turn(&store);
    let child = spawn_child(
        &store,
        &mut artifacts,
        &parent,
        SessionRelation::DelegatedWorker,
        3,
        &["read"],
    );
    let launched = child.fence();

    // The role file is edited: same capability-profile identity, wider
    // contents. This is an *insert*, never an update.
    let widened = resolve_loadout(&store, &child.fixture, &["read", "write"], &["scout"])
        .expect("the widened definition resolves");
    assert_eq!(widened.loadout, launched.id(), "same logical loadout");
    assert_ne!(widened.digest, launched.digest(), "different contents");

    let conn = harness.inspect();
    assert_eq!(
        count(&conn, "capability_profiles"),
        2,
        "SES-003: the digest is in the primary key, so this is a second snapshot"
    );
    assert_eq!(count(&conn, "worker_loadouts"), 2);
    drop(conn);

    // The original snapshot still grants exactly what it granted.
    let record = store
        .read_session_loadout(child.session.incarnation)
        .expect("reading the binding")
        .expect("bound");
    let original_profile = record.persisted.spec.capability_profile;
    let conn = harness.inspect();
    assert_eq!(
        capabilities_of(
            &conn,
            original_profile.id(),
            &profile_digest_hex(original_profile.digest())
        ),
        vec!["read".to_owned()],
        "SES-003: the launch snapshot is not rewritten by today's role file"
    );
    drop(conn);

    // Presenting the widened fence is refused, and no permit exists.
    let before = dump_domain(&harness.inspect());
    let artifacts_ro = harness.open_artifacts();
    let refusal = authorize_resume(&store, &artifacts_ro, &child, widened.fence())
        .expect_err("a widened profile is not the launch snapshot");
    match refusal {
        ResumeRefusal::Refused(conflict) => {
            assert_eq!(conflict.kind(), ConflictKind::LoadoutDigestMismatch);
        }
        other => panic!("SES-003: {other:?}"),
    }
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-003: a refused resume commits nothing",
    );

    // The original fence still resumes, and still under `{read}`.
    let authorized = authorize_resume(&store, &artifacts_ro, &child, launched)
        .expect("the launch fence resumes");
    let resumed = authorized.spawn_with(|_permit, resume| resume.fence());
    assert_eq!(resumed, launched);
    let conn = harness.inspect();
    assert_eq!(
        capabilities_of(
            &conn,
            original_profile.id(),
            &profile_digest_hex(original_profile.digest())
        ),
        vec!["read".to_owned()],
        "SES-003: resuming did not widen the sandbox"
    );
    drop(conn);
    store.verify_projections().expect("replay");
}

#[test]
fn ses_003_a_live_incarnation_cannot_be_rebound_to_another_snapshot() {
    // The other half of the same rule. One loadout per incarnation, forever: a
    // new revision needs a new incarnation, so rebinding is refused rather than
    // being an update.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let parent = open_turn(&store);
    let child = spawn_child(
        &store,
        &mut artifacts,
        &parent,
        SessionRelation::Reviewer,
        4,
        &["read"],
    );
    let widened = resolve_loadout(&store, &child.fixture, &["read", "write"], &["scout"])
        .expect("a widened revision");

    // Rebinding the *same* snapshot converges.
    assert!(
        bind_loadout(&store, &child.session, child.fence())
            .expect("a repeat is convergence")
            .duplicate
    );

    let before = dump_domain(&harness.inspect());
    let refusal = bind_loadout(&store, &child.session, widened.fence())
        .expect_err("rebinding to a different snapshot is refused");
    assert_eq!(
        refusal.conflict_code(),
        Some(ConflictKind::SessionIncarnationAlreadyBound.code())
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-003: a refused rebinding changes nothing",
    );
}

// --- SES-004: lineage survives a daemon restart -------------------------------

#[test]
fn ses_004_lineage_survives_daemon_restart() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    // A -> B (scout) -> C (researcher).
    let a = open_named_turn(&store, "session-a");
    let b = spawn_child(
        &store,
        &mut artifacts,
        &a,
        SessionRelation::Scout,
        10,
        &["read"],
    );
    let c = spawn_child(
        &store,
        &mut artifacts,
        &b.session,
        SessionRelation::Researcher,
        11,
        &["read"],
    );

    let verified = store.verify_projections().expect("replay rebuilds lineage");
    assert_eq!(
        verified.lineage_edges, 2,
        "SES-004: both edges rebuilt from `session_lineage_recorded` alone"
    );
    assert_eq!(verified.loadouts, 2);

    // Re-issuing the identical record is idempotent: same source identity, so
    // the ledger converges and no second edge appears.
    let before = dump_domain(&harness.inspect());
    assert!(
        record_lineage(&store, &a, b.session.session, SessionRelation::Scout)
            .expect("a repeat converges")
            .duplicate
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-004: a repeated lineage record writes nothing",
    );
    drop(store);

    // A hundred restarts. Every one replays, and every one still sees the same
    // two edges with the same parents, turns and relations.
    let expected: BTreeSet<(SessionId, SessionId, SessionRelation)> = [
        (a.session, b.session.session, SessionRelation::Scout),
        (
            b.session.session,
            c.session.session,
            SessionRelation::Researcher,
        ),
    ]
    .into_iter()
    .collect();

    restart_loop(&harness, 100, |round, store| {
        let verified = store
            .verify_projections()
            .unwrap_or_else(|error| panic!("restart {round}: {error}"));
        assert_eq!(verified.lineage_edges, 2, "restart {round}");
        let conn = harness.inspect();
        let mut statement = conn
            .prepare(
                "SELECT parent_session_id, child_session_id, parent_turn_id, relation_kind
                   FROM session_edges ORDER BY child_session_id",
            )
            .expect("reading edges");
        let rows: Vec<(String, String, String, String)> = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("iterating edges")
            .map(|row| row.expect("an edge"))
            .collect();
        assert_eq!(rows.len(), 2, "restart {round}");
        let found: BTreeSet<(SessionId, SessionId, SessionRelation)> = rows
            .iter()
            .map(|(parent, child, _, relation)| {
                (
                    SessionId::parse(parent).expect("a parent identity"),
                    SessionId::parse(child).expect("a child identity"),
                    match relation.as_str() {
                        "scout" => SessionRelation::Scout,
                        "researcher" => SessionRelation::Researcher,
                        other => panic!("restart {round}: unexpected relation {other}"),
                    },
                )
            })
            .collect();
        assert_eq!(found, expected, "restart {round}");
        // The parent turn is the *delegating* turn, and it survives too.
        assert_eq!(
            rows.iter()
                .find(|(_, child, _, _)| child == &b.session.session.to_string())
                .map(|(_, _, turn, _)| turn.clone()),
            Some(a.turn.to_string()),
            "restart {round}"
        );
    });
}

// --- SES-005: a multi-hop lineage cycle fails closed ---------------------------

#[test]
fn ses_005_multi_hop_lineage_cycle_fails_closed() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    let a = open_named_turn(&store, "cycle-a");
    let b = spawn_child(
        &store,
        &mut artifacts,
        &a,
        SessionRelation::Scout,
        20,
        &["read"],
    );
    let c = spawn_child(
        &store,
        &mut artifacts,
        &b.session,
        SessionRelation::Reviewer,
        21,
        &["read"],
    );

    // A -> B -> C is three legal constructor calls. Making C the parent of A
    // closes the cycle, and only a walk of the whole chain can see it.
    let before = dump_domain(&harness.inspect());
    let refusal = record_lineage(&store, &c.session, a.session, SessionRelation::Observer)
        .expect_err("SES-005: a three-hop cycle is refused");
    assert_eq!(
        refusal.conflict_code(),
        Some(ConflictKind::SessionLineageCycle.code())
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-005: a refused edge writes no row and no event",
    );

    // The one-hop case is a different guard, in a different layer: the pure
    // constructor refuses it before any query runs. Both must hold
    // independently, which is why the store calls the constructor as well.
    let refusal = record_lineage(&store, &a, a.session, SessionRelation::ProviderFork)
        .expect_err("SES-005: a session cannot be its own parent");
    assert_eq!(
        refusal.conflict_code(),
        Some(ConflictKind::SessionLineageCycle.code())
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-005: nor does a self-parent",
    );

    store
        .verify_projections()
        .expect("replay after the refusals");
}

#[test]
fn ses_005_a_cycle_a_restore_created_is_reported_rather_than_walked_forever() {
    // The depth bound's reason for existing. No store operation can produce a
    // cyclic graph, so this one is written directly through a second
    // connection — which is exactly what a restore from a mangled backup looks
    // like to the next process.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let a = open_named_turn(&store, "restore-a");
    let b = spawn_child(
        &store,
        &mut artifacts,
        &a,
        SessionRelation::Scout,
        30,
        &["read"],
    );
    assert!(
        store
            .list_broken_lineage()
            .expect("a healthy walk")
            .is_empty(),
        "a graph this store built always terminates"
    );
    drop(store);

    // Both halves, so the ledger and the projection still agree and the reopen
    // is not refused for a different reason. That is what a mangled backup
    // looks like: internally consistent, and impossible for any live operation
    // to have produced.
    let conn = rusqlite::Connection::open(harness.database_path()).expect("writable connection");
    conn.execute(
        "INSERT INTO events (event_id, kind, schema_version, observed_at_ms, session_id,
                source_namespace, source_event_id, source_event_fence, safe_metadata_json)
         VALUES (?1, 'session_lineage_recorded', 1, 1, ?2, 'cg.restore', ?3,
                 'session_lineage_recorded', ?4)",
        rusqlite::params![
            governor_testkit::scenario::id::<governor_core::id::kind::Event>(0xBAD1).to_string(),
            a.session.to_string(),
            a.session.to_string(),
            format!(
                r#"{{"parent_session":"{}","parent_turn":"{}","relation":"observer"}}"#,
                b.session.session, b.session.turn
            ),
        ],
    )
    .expect("a restore that reintroduced a lineage event");
    let seq: i64 = conn
        .query_row("SELECT MAX(seq) FROM events", [], |row| row.get(0))
        .expect("the injected sequence");
    conn.execute(
        "INSERT INTO session_edges (parent_session_id, child_session_id, parent_turn_id,
                relation_kind, created_event_seq)
         VALUES (?1, ?2, ?3, 'observer', ?4)",
        rusqlite::params![
            b.session.session.to_string(),
            a.session.to_string(),
            b.session.turn.to_string(),
            seq,
        ],
    )
    .expect("and the edge it describes");
    drop(conn);

    // The walk terminates — at the bound — and names the sessions it could not
    // resolve, instead of hanging.
    let store = harness.open().expect("reopen");
    let broken = store.list_broken_lineage().expect("a bounded walk");
    assert_eq!(
        broken.len(),
        2,
        "both sessions in the cycle are unresolvable"
    );
    assert!(broken.contains(&a.session) && broken.contains(&b.session.session));

    // And the durable attention it produces is session-scoped.
    for session in broken {
        store
            .raise_lineage_broken(SessionHealthRequest { session })
            .expect("attention on an unwalkable lineage");
    }
    let conditions = store.open_health_conditions().expect("reading conditions");
    assert_eq!(conditions.len(), 2);
    assert!(conditions.iter().all(|c| c.kind
        == governor_core::health::HealthConditionKind::LineageBroken
        && c.scope.session.is_some()));
}

// --- SES-006: the parent turn must belong to the parent session ---------------

#[test]
fn ses_006_parent_turn_must_belong_to_the_parent_session() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    let a = open_named_turn(&store, "owner-a");
    let z = open_named_turn(&store, "unrelated-z");
    let child = spawn_child(
        &store,
        &mut artifacts,
        &a,
        SessionRelation::Scout,
        40,
        &["read"],
    );
    let second = open_named_turn(&store, "second-child");

    // Session A as the parent, but with a turn drawn from unrelated session Z.
    // The foreign key on `parent_turn_id` is satisfied — Z's turn exists — so
    // the schema alone permits this row. The two-hop join is what refuses it.
    let before = dump_domain(&harness.inspect());
    let refusal = store
        .record_session_lineage(governor_store_sqlite::RecordSessionLineageRequest {
            parent_session: a.session,
            child_session: second.session,
            parent_turn: z.turn,
            relation: SessionRelation::Scout,
        })
        .expect_err("SES-006: a turn of another session is not this session's turn");
    assert_eq!(
        refusal.conflict_code(),
        Some(ConflictKind::ParentTurnNotOwnedByParentSession.code())
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "SES-006: a refused edge writes nothing",
    );

    // An unknown turn identity is reported the same way, deliberately: probing
    // turn identities against a session must reveal nothing.
    let refusal = store
        .record_session_lineage(governor_store_sqlite::RecordSessionLineageRequest {
            parent_session: a.session,
            child_session: second.session,
            parent_turn: governor_testkit::scenario::id(0xDEAD),
            relation: SessionRelation::Scout,
        })
        .expect_err("SES-006: an unknown turn is refused the same way");
    assert_eq!(
        refusal.conflict_code(),
        Some(ConflictKind::ParentTurnNotOwnedByParentSession.code())
    );

    // The parent's *own* turn is accepted, so the guard is not simply refusing
    // everything.
    record_lineage(&store, &a, second.session, SessionRelation::Scout)
        .expect("SES-006: the parent's own turn is accepted");
    assert_eq!(count(&harness.inspect(), "session_edges"), 2);
    assert_eq!(child.relation, SessionRelation::Scout);
    store.verify_projections().expect("replay");
}

// --- The refusal surface, as codes -------------------------------------------

#[test]
fn every_session_refusal_has_a_stable_code() {
    // These codes reach the CLI and the acceptance suite, so they are contract.
    for (kind, code) in [
        (ConflictKind::SessionLineageCycle, "session_lineage_cycle"),
        (
            ConflictKind::SessionLineageTooDeep,
            "session_lineage_too_deep",
        ),
        (
            ConflictKind::ParentTurnNotOwnedByParentSession,
            "parent_turn_not_owned_by_parent_session",
        ),
        (
            ConflictKind::SessionIncarnationAlreadyBound,
            "session_incarnation_already_bound",
        ),
        (ConflictKind::NoSessionLoadout, "no_session_loadout"),
    ] {
        assert_eq!(kind.code(), code);
    }
}
