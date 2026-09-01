//! Durable-orchestration review, "Acceptance tests to add before adapters" 1-12.
//!
//! Source: `docs/research/2026-08-31-durable-orchestration-pattern-review.md`.
//! `governor-store-sqlite`'s `store_durability` suite already proves one
//! representative case of most of them. What is added here is the exhaustive
//! version the foreman asked for: every effect class crossed with every kill
//! window, replay verified after every generated scenario, and the full
//! forbidden-data sweep of the journal and attempt tables.
//!
//! # Coverage
//!
//! | Test | Review test | Status |
//! | --- | --- | --- |
//! | [`research_01_no_adapter_before_a_durable_intent`] | 1 | representative case in `store_durability`; the dispatch-fence half covered here |
//! | [`research_02_kill_after_intent_before_io`] | 2 | exhaustive over effect class × kill window, covered here |
//! | [`research_03_kill_after_io_before_outcome`] | 3 | exhaustive over effect class × kill window, covered here |
//! | [`research_04_a_completed_mutation_replays_without_the_adapter`] | 4 | covered in `store_durability`; the "adapter untouched" half covered here |
//! | [`research_05_a_pending_mutation_never_dispatches`] | 5 | covered in `store_durability`; the "adapter untouched" half covered here |
//! | [`research_06_a_different_command_identity_is_new_work`] | 6 | covered in `store_durability`; restated here against the fake destination |
//! | [`research_07_a_recycled_process_slot_cannot_own_the_lease`] | 7 | covered in `governor-store-sqlite` `store_durability` and `governor-core` |
//! | [`research_08_a_stale_token_or_epoch_cannot_mutate_ownership`] | 8 | covered in `governor-store-sqlite` `store_durability` and `governor-core` |
//! | [`research_09_a_receipt_ack_permits_retention_only`] | 9 | covered in `store_durability`; restated across a restart here |
//! | [`research_10_every_transport_receipt_leaves_the_work_open`] | 10 | covered in `store_durability`; extended here with the browser layer |
//! | [`research_11_replay_equivalence_after_every_scenario`] | 11 | covered here (every generated scenario in this suite) |
//! | [`research_12_the_journal_holds_no_forbidden_data`] | 12 | covered here (full sweep) |
//!
//! Tests 7 and 8 are not re-implemented: they are proven twice already, in the
//! pure lease machine and against real SQLite, and a third copy would be
//! duplication rather than coverage. The tests below name them so the table is
//! auditable.

use governor_core::effect::{
    DestinationRef, EffectAmbiguityReason, EffectDecision, ExternalEffectClass,
    IdempotencyContract, IdempotencyKey, NoEffectClass,
};
use governor_core::health::HealthConditionKind;
use governor_core::mutation::{
    MutationCommandKind, MutationCommandStatus, MutationFingerprint, SafeMutationResult,
};
use governor_core::obligation::ObligationState;
use governor_core::time::DurationMs;
use governor_store_sqlite::{
    AckMutationReceiptRequest, AttemptEvidence, BeginMutationRequest, CompleteMutationRequest,
    ExternalOutcome, Failpoint, MarkExternalDispatchedRequest, MutationAdmission,
    RecordExternalIntentRequest, RecordExternalOutcomeRequest, Store, StoreError,
};
use governor_testkit::dump::{assert_unchanged, count, dump_domain};
use governor_testkit::effect::FakeExternalDestination;
use governor_testkit::failpoints::StoreCrash;
use governor_testkit::harness::Harness;
use governor_testkit::scenario::{
    FINAL_RESULT, LIVE_CLAIM, accepted_work, id, snapshot, source, token,
};
use governor_testkit::sentinels::{FORBIDDEN, assert_no_forbidden_bytes, sweep};

/// Every effect class the durable-intent protocol distinguishes.
fn effect_classes() -> Vec<(&'static str, ExternalEffectClass)> {
    vec![
        ("read", ExternalEffectClass::Read),
        (
            "idempotent write, deduplicated by key",
            ExternalEffectClass::IdempotentWrite {
                contract: IdempotencyContract::DeduplicatedByKey {
                    window: DurationMs::from_millis(60_000),
                },
                key: IdempotencyKey::new(token("k-1")),
            },
        ),
        (
            "idempotent write, conditional on a destination fence",
            ExternalEffectClass::IdempotentWrite {
                contract: IdempotencyContract::ConditionalOnDestinationFence,
                key: IdempotencyKey::new(token("k-2")),
            },
        ),
        (
            "non-idempotent write",
            ExternalEffectClass::NonIdempotentWrite,
        ),
    ]
}

fn destination() -> DestinationRef {
    DestinationRef::new(token("worker-host"), token("turn-7"), token("gen-1"))
}

fn intent(store: &Store, class: ExternalEffectClass) -> RecordExternalIntentRequest {
    RecordExternalIntentRequest {
        class,
        destination: destination(),
        source: source("worker.resume", "cmd-1", "rev-1"),
        daemon_epoch: store.daemon_epoch(),
    }
}

#[test]
fn research_01_no_adapter_before_a_durable_intent() {
    for (label, class) in effect_classes() {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut adapter = FakeExternalDestination::attach(&harness.database_path());

        let granted = store
            .record_external_intent(intent(&store, class))
            .unwrap_or_else(|error| panic!("{label}: {error}"));

        // The adapter looks for itself, through its own connection: the intent
        // must already be committed, and the dispatch fence must already be in
        // place before the call is made.
        adapter.probe_intent(&granted.permit);
        store
            .mark_external_dispatched(MarkExternalDispatchedRequest {
                attempt: granted.attempt,
            })
            .expect("the dispatch fence, immediately before the call");
        let evidence = adapter.deliver(&granted.permit);
        store
            .record_external_outcome(RecordExternalOutcomeRequest {
                attempt: granted.attempt,
                outcome: ExternalOutcome::Completed { evidence },
            })
            .expect("recording the landed effect");

        assert_eq!(adapter.delivered(), [granted.attempt], "{label}");
        let decision = store
            .resolve_external_attempt(granted.attempt)
            .expect("resolving");
        assert!(
            !decision.is_execute(),
            "{label}: a completed effect replays"
        );
    }
}

#[test]
fn research_02_kill_after_intent_before_io() {
    // Three ways the process can die in the window between "the intent is
    // decided" and "the call is issued", crossed with every effect class.
    for (label, class) in effect_classes() {
        // (a) The crash lands before the intent row commits. Nothing exists, so
        //     no permit was ever surrendered and nothing needs reconciling.
        let harness = Harness::new();
        let crash = StoreCrash::at("record_external_intent", Failpoint::AfterIntentInsert);
        let store = harness.open_with(Some(crash.boxed())).expect("opening");
        let adapter = FakeExternalDestination::attach(&harness.database_path());
        let error = store
            .record_external_intent(intent(&store, class.clone()))
            .expect_err("the injected crash aborts before COMMIT");
        assert!(matches!(error, StoreError::Sqlite(_)), "{label}");
        assert_eq!(count(&harness.inspect(), "external_attempts"), 0, "{label}");
        adapter.assert_untouched(label);
        drop(store);
        let store = harness.open().expect("reopen");
        assert_eq!(store.startup().recovery.ambiguous_attempts, 0, "{label}");
        drop(store);

        // (b) The intent commits and the process dies before dispatching.
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let adapter = FakeExternalDestination::attach(&harness.database_path());
        let granted = store
            .record_external_intent(intent(&store, class.clone()))
            .expect("a durable intent");
        let attempt = granted.attempt;
        drop(granted);
        drop(store);
        adapter.assert_untouched(label);
        assert_reconciles(&harness, attempt, label);

        // (c) The crash lands inside the outcome transaction, before it
        //     commits, with nothing dispatched. One lifetime, with the crash
        //     armed from the start: a second concurrent open would quarantine
        //     the intent first and turn this into a different test.
        let harness = Harness::new();
        let crash = StoreCrash::at("record_external_outcome", Failpoint::BeforeCommit);
        let store = harness.open_with(Some(crash.boxed())).expect("opening");
        let granted = store
            .record_external_intent(intent(&store, class))
            .expect("a durable intent");
        let before = dump_domain(&harness.inspect());
        let error = store
            .record_external_outcome(RecordExternalOutcomeRequest {
                attempt: granted.attempt,
                outcome: ExternalOutcome::FailedBeforeEffect {
                    proof: NoEffectClass::NotAttempted,
                },
            })
            .expect_err("the injected crash aborts before COMMIT");
        assert!(
            matches!(error, StoreError::Sqlite(_)),
            "{label}: expected an injected abort, got {error}"
        );
        assert!(crash.fired(), "{label}: the point was never reached");
        assert_unchanged(
            &before,
            &dump_domain(&harness.inspect()),
            &format!("{label}: an interrupted outcome changes nothing"),
        );

        // Reopening quarantines the still-undecided intent, exactly as the
        // pre-dispatch window does.
        drop(store);
        assert_reconciles(&harness, granted.attempt, label);
    }
}

#[test]
fn research_03_kill_after_io_before_outcome() {
    for (label, class) in effect_classes() {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut adapter = FakeExternalDestination::attach(&harness.database_path());

        let granted = store
            .record_external_intent(intent(&store, class.clone()))
            .expect("a durable intent");
        store
            .mark_external_dispatched(MarkExternalDispatchedRequest {
                attempt: granted.attempt,
            })
            .expect("the dispatch fence");
        // The call really is issued, and then the process dies.
        let _evidence = adapter.deliver(&granted.permit);
        let attempt = granted.attempt;
        drop(granted);
        drop(store);

        let dispatched: i64 = harness
            .inspect()
            .query_row(
                "SELECT dispatched FROM external_attempts WHERE external_attempt_id = ?1",
                rusqlite::params![attempt.to_string()],
                |row| row.get(0),
            )
            .expect("reading the dispatch fence");
        assert_eq!(dispatched, 1, "{label}: the call may well have landed");

        // Same answer as the pre-dispatch window, deliberately: the fate is
        // unknown either way, and unknown never projects success — not even for
        // a `Read`, whose *outcome* is still unknown even though a repeat would
        // be harmless.
        assert_reconciles(&harness, attempt, label);

        // And "we never tried" is no longer an admissible proof once the
        // dispatch fence is committed.
        let store = harness.open().expect("reopen");
        let error = store
            .record_external_outcome(RecordExternalOutcomeRequest {
                attempt,
                outcome: ExternalOutcome::FailedBeforeEffect {
                    proof: NoEffectClass::NotAttempted,
                },
            })
            .expect_err("a quarantined attempt is terminal");
        assert!(error.conflict_code().is_some(), "{label}: {error}");
        let _ = class;
    }
}

/// Asserts one attempt was quarantined into reconciliation, and stays there.
fn assert_reconciles(
    harness: &Harness,
    attempt: governor_core::id::ExternalAttemptId,
    label: &str,
) {
    let store = harness.open().expect("reopen");
    let recovery = &store.startup().recovery;
    assert_eq!(recovery.ambiguous_attempts, 1, "{label}");
    assert_eq!(recovery.reconciliation_conditions, 1, "{label}");

    let decision = store
        .resolve_external_attempt(attempt)
        .expect("resolving the orphaned attempt");
    assert!(!decision.is_execute(), "{label}: zero automatic replay");
    let EffectDecision::Reconcile(required) = decision else {
        panic!("{label}: expected reconciliation");
    };
    assert_eq!(required.attempt(), attempt, "{label}");
    assert_eq!(
        required.reason(),
        EffectAmbiguityReason::OrphanedByRestart,
        "{label}"
    );

    let conditions = store.open_health_conditions().expect("reading conditions");
    assert_eq!(conditions.len(), 1, "{label}");
    assert_eq!(
        conditions[0].kind,
        HealthConditionKind::ReconciliationRequired,
        "{label}"
    );
    assert_eq!(
        conditions[0].scope.external_attempt,
        Some(attempt),
        "{label}"
    );

    // A second recovery pass adds nothing.
    drop(store);
    let store = harness.open().expect("reopen again");
    assert_eq!(store.startup().recovery.ambiguous_attempts, 0, "{label}");
    assert_eq!(
        store
            .open_health_conditions()
            .expect("reading conditions")
            .len(),
        1,
        "{label}"
    );
}

// --- The mutation-command journal ---------------------------------------------

fn begin(store: &Store, command: u128, parameter: &str) -> BeginMutationRequest {
    BeginMutationRequest {
        actor: id(1),
        command: id(command),
        kind: MutationCommandKind::new(token("worker.resume")),
        fingerprint: fingerprint(parameter),
        daemon_epoch: store.daemon_epoch(),
    }
}

fn fingerprint(parameter: &str) -> MutationFingerprint {
    MutationFingerprint::derive(
        &MutationCommandKind::new(token("worker.resume")),
        &[&token(parameter)],
    )
}

#[test]
fn research_04_a_completed_mutation_replays_without_the_adapter() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let adapter = FakeExternalDestination::attach(&harness.database_path());

    store
        .begin_mutation(begin(&store, 900, "turn-7"))
        .expect("a new identity");
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(900),
            fingerprint: fingerprint("turn-7"),
            result: SafeMutationResult::Applied {
                reference: Some(token("resumed-1")),
            },
        })
        .expect("committing the safe result");

    // The exact retry returns the recorded result, and the destination is never
    // reached — that is the whole point of the journal.
    for _ in 0..5 {
        assert_eq!(
            store
                .begin_mutation(begin(&store, 900, "turn-7"))
                .expect("an exact retry resolves"),
            MutationAdmission::Replayed(SafeMutationResult::Applied {
                reference: Some(token("resumed-1"))
            })
        );
    }
    adapter.assert_untouched("research 4");
}

#[test]
fn research_05_a_pending_mutation_never_dispatches() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let adapter = FakeExternalDestination::attach(&harness.database_path());
    store
        .begin_mutation(begin(&store, 901, "turn-7"))
        .expect("a new identity");

    for _ in 0..5 {
        let error = store
            .begin_mutation(begin(&store, 901, "turn-7"))
            .expect_err("a received identity with no result is uncertain");
        assert_eq!(error.conflict_code(), Some("mutation_result_uncertain"));
    }
    drop(store);

    // Across restarts, indefinitely.
    for round in 0..5 {
        let store = harness.open().expect("reopen");
        let error = store
            .begin_mutation(begin(&store, 901, "turn-7"))
            .expect_err("still uncertain");
        assert_eq!(
            error.conflict_code(),
            Some("mutation_result_uncertain"),
            "round {round}"
        );
    }
    adapter.assert_untouched("research 5");
    assert_eq!(count(&harness.inspect(), "mutation_commands"), 1);
}

#[test]
fn research_06_a_different_command_identity_is_new_work() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    store
        .begin_mutation(begin(&store, 904, "turn-7"))
        .expect("first identity");
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(904),
            fingerprint: fingerprint("turn-7"),
            result: SafeMutationResult::AlreadySatisfied,
        })
        .expect("completing it");

    // Same parameters, different command identity: genuinely new work that must
    // pass normal policy rather than being deduplicated against the first.
    assert_eq!(
        store
            .begin_mutation(begin(&store, 905, "turn-7"))
            .expect("a new identity"),
        MutationAdmission::Dispatch
    );
    // Same identity, different actor: also new.
    assert_eq!(
        store
            .begin_mutation(BeginMutationRequest {
                actor: id(2),
                ..begin(&store, 904, "turn-7")
            })
            .expect("a different actor is a different identity"),
        MutationAdmission::Dispatch
    );
    // Same identity, different operation: a typed mismatch, never the first
    // operation's answer.
    let error = store
        .begin_mutation(begin(&store, 904, "turn-8"))
        .expect_err("this is not an exact retry");
    assert_eq!(error.conflict_code(), Some("mutation_command_mismatch"));
    assert_eq!(count(&harness.inspect(), "mutation_commands"), 3);
}

#[test]
fn research_07_a_recycled_process_slot_cannot_own_the_lease() {
    // Proven twice already, against real SQLite in `governor-store-sqlite`
    // `store_durability::a_recycled_process_slot_cannot_release_a_lease` and in
    // the pure machine in `governor-core`
    // `durable_execution_invariants::a_reused_process_slot_cannot_own_or_release_the_old_lease`.
    // Naming it here keeps the coverage table auditable without a third copy.
    assert_eq!(
        governor_core::error::ConflictKind::StaleProcessIncarnation.code(),
        "stale_process_incarnation"
    );
}

#[test]
fn research_08_a_stale_token_or_epoch_cannot_mutate_ownership() {
    // As above: `store_durability::a_stale_token_or_daemon_epoch_cannot_mutate_ownership`
    // and `durable_execution_invariants::stale_lease_fences_cannot_mutate_current_ownership`,
    // plus the two-instance case in this crate's `db_acceptance`.
    assert_eq!(
        governor_core::error::ConflictKind::StaleLeaseToken.code(),
        "stale_lease_token"
    );
    assert_eq!(
        governor_core::error::ConflictKind::StaleDaemonEpoch.code(),
        "stale_daemon_epoch"
    );
}

#[test]
fn research_09_a_receipt_ack_permits_retention_only() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    store
        .begin_mutation(begin(&store, 910, "turn-7"))
        .expect("a mutation identity");
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(910),
            fingerprint: fingerprint("turn-7"),
            result: SafeMutationResult::Applied { reference: None },
        })
        .expect("a safe result");
    assert_eq!(
        store
            .ack_mutation_receipt(AckMutationReceiptRequest {
                actor: id(1),
                command: id(910),
            })
            .expect("ACK layer 1"),
        MutationCommandStatus::Acked
    );

    // Layer 1 unlocked retention on a journal row and reached nothing else,
    // including across a restart.
    for round in 0..3 {
        let store = harness.open().expect("reopen");
        let current = snapshot(&store, work.obligation);
        let _ = round;
        assert_eq!(current.state, ObligationState::CompletedUnprocessed);
        assert!(current.open, "a receipt ACK closes no engineering work");
        assert_eq!(
            governor_testkit::scenario::artifact_rows(&harness.inspect())[0].retention(),
            governor_core::artifact::RetentionState::Pinned
        );
    }
}

#[test]
fn research_10_every_transport_receipt_leaves_the_work_open() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // Layer 2: the browser wake is accepted — the consumer has the
    // notification. Layer 1: every mutation receipt is acknowledged. Layer 3
    // has not happened.
    for command in 920u128..925 {
        store
            .begin_mutation(begin(&store, command, "turn-7"))
            .expect("an identity");
        store
            .complete_mutation(CompleteMutationRequest {
                actor: id(1),
                command: id(command),
                fingerprint: fingerprint("turn-7"),
                result: SafeMutationResult::AlreadySatisfied,
            })
            .expect("a result");
        store
            .ack_mutation_receipt(AckMutationReceiptRequest {
                actor: id(1),
                command: id(command),
            })
            .expect("a receipt ACK");
    }

    // And an external attempt that completed, which is a third kind of receipt.
    let granted = store
        .record_external_intent(intent(&store, ExternalEffectClass::Read))
        .expect("an intent");
    store
        .mark_external_dispatched(MarkExternalDispatchedRequest {
            attempt: granted.attempt,
        })
        .expect("the fence");
    store
        .record_external_outcome(RecordExternalOutcomeRequest {
            attempt: granted.attempt,
            outcome: ExternalOutcome::Completed {
                evidence: AttemptEvidence::new(token("dest-1")),
            },
        })
        .expect("a landed effect");

    let current = snapshot(&store, work.obligation);
    assert_eq!(current.state, ObligationState::CompletedUnprocessed);
    assert!(current.open, "no accumulation of receipts closes work");
    drop(store);
    let store = harness.open().expect("reopen");
    assert!(snapshot(&store, work.obligation).open);
    assert_eq!(
        governor_testkit::scenario::artifact_rows(&harness.inspect())[0].retention(),
        governor_core::artifact::RetentionState::Pinned
    );
}

#[test]
fn research_11_replay_equivalence_after_every_scenario() {
    // Every scenario family this suite generates, replayed after every step.
    // `db_acceptance::db_001_projection_replay_equivalence` does the same for
    // the obligation lifecycle; this covers the durable-execution families,
    // which are deliberately *not* ledger-derived and are therefore re-proved
    // by their own loaders on every read.
    for (label, class) in effect_classes() {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let work = accepted_work(&store, &mut artifacts, "conv-A");
        store
            .verify_projections()
            .expect("replay after the lifecycle");

        let granted = store
            .record_external_intent(intent(&store, class))
            .expect("an intent");
        store.verify_projections().expect("replay after the intent");
        store
            .mark_external_dispatched(MarkExternalDispatchedRequest {
                attempt: granted.attempt,
            })
            .expect("the fence");
        store.verify_projections().expect("replay after the fence");
        store
            .record_external_outcome(RecordExternalOutcomeRequest {
                attempt: granted.attempt,
                outcome: ExternalOutcome::Ambiguous {
                    reason: EffectAmbiguityReason::ResponseLost,
                },
            })
            .expect("an unknown fate");
        store
            .verify_projections()
            .expect("replay after the outcome");

        store
            .begin_mutation(begin(&store, 930, "turn-7"))
            .expect("an identity");
        store
            .verify_projections()
            .expect("replay after the journal row");

        let minted = governor_testkit::scenario::mint_claim(
            &store,
            work.obligation,
            &work.wake,
            work.generation,
            LIVE_CLAIM,
        )
        .expect("a claim");
        store.verify_projections().expect("replay after the claim");
        governor_testkit::scenario::handoff(&store, work.obligation, minted.claim)
            .expect("a handoff");
        let verified = store
            .verify_projections()
            .expect("replay after the handoff");
        assert_eq!(verified.obligations, 1, "{label}");
        assert_eq!(verified.deliveries, 1, "{label}");

        drop(store);
        let store = harness.open().expect("reopen");
        store
            .verify_projections()
            .unwrap_or_else(|error| panic!("{label}: replay after a restart: {error}"));
    }
}

#[test]
fn research_12_the_journal_holds_no_forbidden_data() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // Drive every durable-execution family, so the sweep has command-journal
    // rows, external-attempt rows and lease rows to scan and not just a ledger.
    store
        .begin_mutation(begin(&store, 940, "turn-7"))
        .expect("an identity");
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(940),
            fingerprint: fingerprint("turn-7"),
            result: SafeMutationResult::Applied {
                reference: Some(token("resumed-1")),
            },
        })
        .expect("a safe result");
    let granted = store
        .record_external_intent(intent(&store, ExternalEffectClass::NonIdempotentWrite))
        .expect("an intent");
    store
        .mark_external_dispatched(MarkExternalDispatchedRequest {
            attempt: granted.attempt,
        })
        .expect("the fence");
    store
        .record_external_outcome(RecordExternalOutcomeRequest {
            attempt: granted.attempt,
            outcome: ExternalOutcome::Completed {
                evidence: AttemptEvidence::new(token("dest-1")),
            },
        })
        .expect("a landed effect");
    drop(store);

    // The command journal and attempt tables must never contain a raw prompt,
    // tool arguments or results, a shell command, a cwd, a transcript, a
    // provider stream record, or a credential. Nothing here filters by table:
    // the whole state root is scanned.
    let files = harness.all_files();
    assert_no_forbidden_bytes(&files, "research 12");

    // And the honest positive control: the sweep does find something when
    // something is there, so a clean result is not a scanner that never works.
    let planted = vec![("planted".to_owned(), FORBIDDEN[0].value.as_bytes().to_vec())];
    assert_eq!(sweep(&planted, FORBIDDEN).len(), 1);

    // The one deliberate exception is the final assistant result, which lives
    // in the artifact the obligation pins.
    let objects_prefix = format!("artifacts/objects/{}", work.artifact.key());
    assert!(
        files
            .iter()
            .any(|(name, bytes)| name == &objects_prefix && bytes == FINAL_RESULT),
        "the designated artifact must actually hold the final result"
    );
}
