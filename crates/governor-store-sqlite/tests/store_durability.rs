//! Durable intent, kill windows, the mutation journal, recovery and leases.
//!
//! Numbered tests are from "Acceptance tests to add before adapters" in
//! `docs/research/2026-08-31-durable-orchestration-pattern-review.md`.
//!
//! | Test | Requirement |
//! | --- | --- |
//! | [`the_permit_is_only_produced_after_the_intent_is_observable`] | research 1 |
//! | [`a_crash_before_the_intent_is_durable_grants_no_permit`] | research 1 |
//! | [`kill_after_intent_before_dispatch_yields_reconciliation`] | research 2 |
//! | [`kill_after_dispatch_before_outcome_yields_reconciliation`] | research 3 |
//! | [`a_completed_mutation_retry_replays_its_result`] | research 4 |
//! | [`a_pending_mutation_retry_is_uncertain_and_never_dispatches`] | research 5 |
//! | [`a_different_command_id_is_a_new_operation`] | research 6 |
//! | [`a_recycled_process_slot_cannot_release_a_lease`] | research 7 |
//! | [`a_stale_token_or_daemon_epoch_cannot_mutate_ownership`] | research 8 |
//! | [`a_receipt_ack_cannot_close_a_worker_obligation`] | research 9 |
//! | [`every_transport_receipt_may_ack_while_work_stays_open`] | research 10 |
//! | [`startup_quarantines_orphaned_claimed_and_armed_attempts`] | DB-006, invariant 12 |
//! | [`an_uncertain_mutation_may_still_be_resolved_by_late_evidence`] | journal semantics |

mod support;

use governor_core::effect::{
    DestinationRef, EffectDecision, ExternalEffectClass, ExternalExecutionPermit,
    IdempotencyContract, IdempotencyKey,
};
use governor_core::health::HealthConditionKind;
use governor_core::lease::{
    LeaseHolderProof, LeaseState, ProcessIncarnation, ProcessSlot, ProcessStartRef,
    ResourceIdentity, ResourceNamespace,
};
use governor_core::mutation::{
    MutationCommandKind, MutationCommandStatus, MutationFingerprint, SafeMutationResult,
};
use governor_core::obligation::ObligationState;
use governor_core::time::DurationMs;
use governor_store_sqlite::{
    AckMutationReceiptRequest, AcquireLeaseRequest, AttemptEvidence, BeginMutationRequest,
    CompleteMutationRequest, ExternalOutcome, Failpoint, LeaseHolderRequest,
    MarkExternalDispatchedRequest, MutationAdmission, RecordExternalIntentRequest,
    RecordExternalOutcomeRequest, ResourceRef, Store, StoreError,
};
use rusqlite::Connection;
use support::{
    FireOnce, Harness, bind, count, id, open_turn, publish_result, schedule_wake, source,
    start_worker, token,
};

// --- Durable intent and the permit ------------------------------------------

fn destination() -> DestinationRef {
    DestinationRef::new(token("worker-host"), token("turn-7"), token("gen-1"))
}

fn idempotent_write(key: &str) -> ExternalEffectClass {
    ExternalEffectClass::IdempotentWrite {
        contract: IdempotencyContract::DeduplicatedByKey {
            window: DurationMs::from_millis(60_000),
        },
        key: IdempotencyKey::new(token(key)),
    }
}

fn intent_request(class: ExternalEffectClass, store: &Store) -> RecordExternalIntentRequest {
    RecordExternalIntentRequest {
        class,
        destination: destination(),
        source: source("worker.resume", "cmd-1", "rev-1"),
        daemon_epoch: store.daemon_epoch(),
    }
}

/// A stand-in adapter that refuses to act on a permit whose intent is not
/// already visible to an independent reader.
///
/// This is research test 1 made mechanical: the adapter genuinely looks, and
/// panics rather than performing the effect if the row is not there. Because it
/// reads through its own connection, "observable" means committed, not merely
/// written inside a transaction the writer still holds.
fn adapter_requiring_durable_intent(
    inspect: &Connection,
    permit: ExternalExecutionPermit,
) -> AttemptEvidence {
    let recorded: i64 = inspect
        .query_row(
            "SELECT COUNT(*) FROM external_attempts
              WHERE external_attempt_id = ?1 AND state = 'intent_recorded'",
            rusqlite::params![permit.attempt().to_string()],
            |row| row.get(0),
        )
        .expect("reading the intent row");
    assert_eq!(
        recorded, 1,
        "an adapter must never be reachable before its intent is durable"
    );
    AttemptEvidence::new(token("dest-ref-1"))
}

#[test]
fn the_permit_is_only_produced_after_the_intent_is_observable() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let inspect = harness.inspect();

    let granted = store
        .record_external_intent(intent_request(idempotent_write("k-1"), &store))
        .expect("recording a durable intent");

    // The permit carries fences, never payload, and names the exact key.
    assert_eq!(granted.permit.attempt(), granted.attempt);
    assert_eq!(granted.permit.destination(), &destination());
    assert_eq!(
        granted.permit.class().idempotency_key(),
        Some(&IdempotencyKey::new(token("k-1")))
    );

    // Handing it to the adapter is the assertion: the adapter looks for itself.
    let evidence = adapter_requiring_durable_intent(&inspect, granted.permit);

    store
        .mark_external_dispatched(MarkExternalDispatchedRequest {
            attempt: granted.attempt,
        })
        .expect("committing the dispatch fence");
    store
        .record_external_outcome(RecordExternalOutcomeRequest {
            attempt: granted.attempt,
            outcome: ExternalOutcome::Completed { evidence },
        })
        .expect("recording the landed effect");

    // A completed attempt replays; it never yields a second permit.
    let decision = store
        .resolve_external_attempt(granted.attempt)
        .expect("resolving a completed attempt");
    assert!(!decision.is_execute());
    assert_eq!(
        decision.replayed(),
        Some(AttemptEvidence::new(token("dest-ref-1")))
    );
}

#[test]
fn a_crash_before_the_intent_is_durable_grants_no_permit() {
    let harness = Harness::new();
    let store = harness
        .open_with(Some(Box::new(FireOnce::new(
            "record_external_intent",
            Failpoint::AfterIntentInsert,
        ))))
        .expect("opening");

    let error = store
        .record_external_intent(intent_request(
            ExternalEffectClass::NonIdempotentWrite,
            &store,
        ))
        .expect_err("the injected crash aborts before COMMIT");
    assert!(matches!(error, StoreError::Sqlite(_)));

    // No row, therefore no acceptance, therefore no permit ever existed. The
    // ordering is structural: `finish` runs only after `COMMIT` returns `Ok`.
    let inspect = harness.inspect();
    assert_eq!(count(&inspect, "external_attempts"), 0);

    drop(store);
    let store = harness.open().expect("reopen");
    assert_eq!(
        store.startup().recovery.ambiguous_attempts,
        0,
        "there is nothing to reconcile: the intent never landed"
    );
}

/// Records an intent, optionally dispatches it, then abandons the process.
fn abandon_attempt(harness: &Harness, dispatch: bool) -> governor_core::id::ExternalAttemptId {
    let store = harness.open().expect("opening");
    let granted = store
        .record_external_intent(intent_request(
            ExternalEffectClass::NonIdempotentWrite,
            &store,
        ))
        .expect("recording a durable intent");
    let attempt = granted.attempt;
    if dispatch {
        store
            .mark_external_dispatched(MarkExternalDispatchedRequest { attempt })
            .expect("committing the dispatch fence");
    }
    // The permit is dropped without an outcome ever being recorded: exactly
    // what a process death looks like from the database's point of view.
    drop(granted);
    drop(store);
    attempt
}

fn assert_reconciliation(harness: &Harness, attempt: governor_core::id::ExternalAttemptId) {
    let store = harness.open().expect("reopen");
    let recovery = &store.startup().recovery;
    assert_eq!(recovery.ambiguous_attempts, 1);
    assert_eq!(recovery.reconciliation_conditions, 1);

    // Ambiguous is terminal, and resolving it offers reconciliation, never a
    // permit. There is no variant of the answer that authorises a replay.
    let decision = store
        .resolve_external_attempt(attempt)
        .expect("resolving the orphaned attempt");
    assert!(!decision.is_execute(), "zero automatic replay");
    let EffectDecision::Reconcile(required) = decision else {
        panic!("expected reconciliation");
    };
    assert_eq!(required.attempt(), attempt);
    assert_eq!(
        required.reason(),
        governor_core::effect::EffectAmbiguityReason::OrphanedByRestart
    );

    // And the attention record is durable and attempt-scoped.
    let conditions = store.open_health_conditions().expect("reading conditions");
    assert_eq!(conditions.len(), 1);
    assert_eq!(
        conditions[0].kind,
        HealthConditionKind::ReconciliationRequired
    );
    assert_eq!(conditions[0].scope.external_attempt, Some(attempt));

    // A second recovery pass finds nothing new and opens no duplicate.
    drop(store);
    let store = harness.open().expect("reopen again");
    assert_eq!(store.startup().recovery.ambiguous_attempts, 0);
    assert_eq!(
        store
            .open_health_conditions()
            .expect("reading conditions")
            .len(),
        1
    );
}

#[test]
fn kill_after_intent_before_dispatch_yields_reconciliation() {
    let harness = Harness::new();
    let attempt = abandon_attempt(&harness, false);
    assert_reconciliation(&harness, attempt);
}

#[test]
fn kill_after_dispatch_before_outcome_yields_reconciliation() {
    let harness = Harness::new();
    let attempt = abandon_attempt(&harness, true);

    let inspect = harness.inspect();
    let dispatched: i64 = inspect
        .query_row("SELECT dispatched FROM external_attempts", [], |row| {
            row.get(0)
        })
        .expect("reading the dispatch fence");
    assert_eq!(dispatched, 1, "the call may well have landed");

    // Same answer as the pre-dispatch window, on purpose: the fate is unknown
    // either way, and unknown never projects success.
    assert_reconciliation(&harness, attempt);
}

// --- The mutation-command journal -------------------------------------------

fn fingerprint(kind: &str, parameter: &str) -> MutationFingerprint {
    MutationFingerprint::derive(&MutationCommandKind::new(token(kind)), &[&token(parameter)])
}

fn begin(store: &Store, command: u128, parameter: &str) -> BeginMutationRequest {
    BeginMutationRequest {
        actor: id(1),
        command: id(command),
        kind: MutationCommandKind::new(token("worker.resume")),
        fingerprint: fingerprint("worker.resume", parameter),
        daemon_epoch: store.daemon_epoch(),
    }
}

#[test]
fn a_completed_mutation_retry_replays_its_result() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");

    assert_eq!(
        store
            .begin_mutation(begin(&store, 900, "turn-7"))
            .expect("a new identity"),
        MutationAdmission::Dispatch
    );
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(900),
            fingerprint: fingerprint("worker.resume", "turn-7"),
            result: SafeMutationResult::Applied {
                reference: Some(token("resumed-1")),
            },
        })
        .expect("committing the safe result");

    // The exact retry returns the recorded result, with no dispatch.
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

    // And it survives a restart.
    drop(store);
    let store = harness.open().expect("reopen");
    assert!(matches!(
        store
            .begin_mutation(begin(&store, 900, "turn-7"))
            .expect("a retry after restart"),
        MutationAdmission::Replayed(_)
    ));

    // Reusing the identity for a *different* operation is a typed mismatch,
    // never the first operation's answer.
    let error = store
        .begin_mutation(begin(&store, 900, "turn-8"))
        .expect_err("this is not an exact retry");
    assert_eq!(error.conflict_code(), Some("mutation_command_mismatch"));
}

#[test]
fn a_pending_mutation_retry_is_uncertain_and_never_dispatches() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    store
        .begin_mutation(begin(&store, 901, "turn-7"))
        .expect("a new identity");

    // No result was committed, so the identity is uncertain — in this process
    // and in every later one.
    let error = store
        .begin_mutation(begin(&store, 901, "turn-7"))
        .expect_err("a received identity with no result is uncertain");
    assert_eq!(error.conflict_code(), Some("mutation_result_uncertain"));

    drop(store);
    let store = harness.open().expect("reopen");
    assert_eq!(
        store.startup().recovery.uncertain_mutations,
        1,
        "startup records the uncertainty rather than guessing"
    );
    let error = store
        .begin_mutation(begin(&store, 901, "turn-7"))
        .expect_err("still uncertain after recovery");
    assert_eq!(error.conflict_code(), Some("mutation_result_uncertain"));

    let inspect = harness.inspect();
    let status: String = inspect
        .query_row("SELECT status FROM mutation_commands", [], |row| row.get(0))
        .expect("reading the journal row");
    assert_eq!(status, "uncertain");
    assert_eq!(count(&inspect, "mutation_commands"), 1);
}

#[test]
fn a_crash_between_dispatch_and_the_result_leaves_the_identity_uncertain() {
    let harness = Harness::new();
    let store = harness
        .open_with(Some(Box::new(FireOnce::new(
            "complete_mutation",
            Failpoint::AfterMutationResult,
        ))))
        .expect("opening");
    store
        .begin_mutation(begin(&store, 902, "turn-7"))
        .expect("a new identity");

    let error = store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(902),
            fingerprint: fingerprint("worker.resume", "turn-7"),
            result: SafeMutationResult::AlreadySatisfied,
        })
        .expect_err("the injected crash aborts before COMMIT");
    assert!(matches!(error, StoreError::Sqlite(_)));
    drop(store);

    let store = harness.open().expect("reopen");
    assert_eq!(store.startup().recovery.uncertain_mutations, 1);
    let error = store
        .begin_mutation(begin(&store, 902, "turn-7"))
        .expect_err("never a redispatch");
    assert_eq!(error.conflict_code(), Some("mutation_result_uncertain"));
}

#[test]
fn an_uncertain_mutation_may_still_be_resolved_by_late_evidence() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    store
        .begin_mutation(begin(&store, 903, "turn-7"))
        .expect("a new identity");
    drop(store);

    let store = harness.open().expect("reopen");
    assert_eq!(store.startup().recovery.uncertain_mutations, 1);

    // Late proven evidence that the mutation did commit resolves the
    // uncertainty. It dispatches nothing: this is a *record*, not a retry.
    let status = store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(903),
            fingerprint: fingerprint("worker.resume", "turn-7"),
            result: SafeMutationResult::AlreadySatisfied,
        })
        .expect("committed evidence resolves uncertainty");
    assert_eq!(status, MutationCommandStatus::Completed);
    assert_eq!(
        store
            .begin_mutation(begin(&store, 903, "turn-7"))
            .expect("now an exact retry replays"),
        MutationAdmission::Replayed(SafeMutationResult::AlreadySatisfied)
    );
}

#[test]
fn a_different_command_id_is_a_new_operation() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    store
        .begin_mutation(begin(&store, 904, "turn-7"))
        .expect("first identity");
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(904),
            fingerprint: fingerprint("worker.resume", "turn-7"),
            result: SafeMutationResult::AlreadySatisfied,
        })
        .expect("completing it");

    // A different command id for the same parameters is genuinely new work and
    // must pass normal policy rather than being deduplicated against the first.
    assert_eq!(
        store
            .begin_mutation(begin(&store, 905, "turn-7"))
            .expect("a new identity"),
        MutationAdmission::Dispatch
    );

    // So is the same command id under a different actor.
    assert_eq!(
        store
            .begin_mutation(BeginMutationRequest {
                actor: id(2),
                ..begin(&store, 904, "turn-7")
            })
            .expect("a different actor is a different identity"),
        MutationAdmission::Dispatch
    );
    assert_eq!(count(&harness.inspect(), "mutation_commands"), 3);
}

// --- The three ACK layers stay separate -------------------------------------

#[test]
fn a_receipt_ack_cannot_close_a_worker_obligation() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");

    store
        .begin_mutation(begin(&store, 910, "turn-7"))
        .expect("a mutation identity");
    store
        .complete_mutation(CompleteMutationRequest {
            actor: id(1),
            command: id(910),
            fingerprint: fingerprint("worker.resume", "turn-7"),
            result: SafeMutationResult::Applied { reference: None },
        })
        .expect("committing the safe result");
    let status = store
        .ack_mutation_receipt(AckMutationReceiptRequest {
            actor: id(1),
            command: id(910),
        })
        .expect("ACK layer 1");
    assert_eq!(status, MutationCommandStatus::Acked);

    // Layer 1 unlocked retention on a journal row. It reached nothing else.
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(snapshot.state, ObligationState::CompletedUnprocessed);
    assert!(snapshot.open, "a receipt ACK closes no engineering work");

    let inspect = harness.inspect();
    let retention: String = inspect
        .query_row("SELECT retention_state FROM result_artifacts", [], |row| {
            row.get(0)
        })
        .expect("reading retention");
    assert_eq!(retention, "pinned", "the artifact is still required");
}

#[test]
fn every_transport_receipt_may_ack_while_work_stays_open() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(&store, turn.obligation, "run-1").expect("publication");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");

    // Layer 2: a browser wake is accepted — the consumer has the notification.
    let claimed = schedule_wake(
        &store,
        turn.obligation,
        generation,
        snapshot.version,
        snapshot.source.clone(),
    )
    .expect("scheduling");
    support::accept_wake(&store, &claimed, generation, "msg-1");

    // Layer 1: every mutation receipt is acknowledged.
    for command in 920u128..925 {
        store
            .begin_mutation(begin(&store, command, "turn-7"))
            .expect("an identity");
        store
            .complete_mutation(CompleteMutationRequest {
                actor: id(1),
                command: id(command),
                fingerprint: fingerprint("worker.resume", "turn-7"),
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

    // Layer 3 has not happened, so the obligation is still owed.
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(snapshot.state, ObligationState::CompletedUnprocessed);
    assert!(snapshot.open);

    // Including across a restart: no accumulation of receipts closes work.
    drop(store);
    let store = harness.open().expect("reopen");
    let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
    assert_eq!(snapshot.state, ObligationState::CompletedUnprocessed);
    assert!(snapshot.open);
}

// --- Startup quarantine ------------------------------------------------------

#[test]
fn startup_quarantines_orphaned_claimed_and_armed_attempts() {
    for arm in [false, true] {
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
        if arm {
            store
                .arm_delivery_send(governor_store_sqlite::ArmDeliverySendRequest {
                    delivery_id: claimed.delivery_id.clone(),
                    binding_generation: generation,
                    attempt: claimed.attempt,
                })
                .expect("arming the Send fence");
        }
        // The process dies with the attempt still owning the external effect.
        drop(store);

        let store = harness.open().expect("reopen");
        assert_eq!(
            store.startup().recovery.quarantined_deliveries,
            1,
            "arm={arm}: invariant 12 quarantines before any browser recovery"
        );

        let inspect = harness.inspect();
        let attempt_state: String = inspect
            .query_row("SELECT state FROM delivery_attempts", [], |row| row.get(0))
            .expect("reading the attempt");
        let delivery_state: String = inspect
            .query_row("SELECT state FROM browser_deliveries", [], |row| row.get(0))
            .expect("reading the delivery");
        assert_eq!(attempt_state, "ambiguous");
        assert_eq!(delivery_state, "ambiguous");

        // Frozen: invariant 13 forbids an automatic resend of this revision.
        let error = schedule_wake(
            &store,
            turn.obligation,
            generation,
            snapshot.version,
            snapshot.source.clone(),
        )
        .expect_err("ambiguous is never automatically resent");
        assert_eq!(error.conflict_code(), Some("delivery_revision_frozen"));

        // And the obligation is untouched: quarantine records uncertainty, it
        // does not fabricate a terminal state.
        let snapshot = store.read_obligation(turn.obligation).expect("snapshot");
        assert_eq!(snapshot.state, ObligationState::CompletedUnprocessed);
        assert!(snapshot.open);
        assert!(
            store.verify_projections().is_ok(),
            "the quarantined projection still replays exactly"
        );
    }
}

// --- Resource leases ---------------------------------------------------------

fn resource() -> ResourceRef {
    ResourceRef::of(&ResourceIdentity::canonical(
        ResourceNamespace::new(token("session")),
        "/Volumes/Data/state/session-a",
    ))
}

fn incarnation(slot: u32, start: &str) -> ProcessIncarnation {
    ProcessIncarnation::new(ProcessSlot::new(slot), ProcessStartRef::new(token(start)))
}

#[test]
fn a_recycled_process_slot_cannot_release_a_lease() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let granted = store
        .acquire_lease(AcquireLeaseRequest {
            resource: resource(),
            holder: id(1),
            incarnation: incarnation(4242, "start-a"),
            daemon_epoch: store.daemon_epoch(),
            ttl: DurationMs::from_millis(30_000),
        })
        .expect("acquiring an unowned resource");
    let epoch = store.daemon_epoch();
    drop(store);

    // A later process inherits the same conceptual PID but has its own start
    // identity. It holds the right token and is still not the holder.
    let store = harness.open().expect("reopen");
    let impostor = LeaseHolderRequest {
        resource: resource(),
        proof: LeaseHolderProof {
            token: granted.token.clone(),
            incarnation: incarnation(4242, "start-b"),
            daemon_epoch: epoch,
        },
        ttl: DurationMs::from_millis(30_000),
    };
    let error = store
        .release_lease(impostor.clone())
        .expect_err("a recycled process number is a different incarnation");
    assert_eq!(error.conflict_code(), Some("stale_process_incarnation"));
    let error = store
        .renew_lease(impostor)
        .expect_err("and it cannot renew either");
    assert_eq!(error.conflict_code(), Some("stale_process_incarnation"));

    // The exact holder still can, across the restart.
    let holder = LeaseHolderRequest {
        resource: resource(),
        proof: LeaseHolderProof {
            token: granted.token.clone(),
            incarnation: incarnation(4242, "start-a"),
            daemon_epoch: epoch,
        },
        ttl: DurationMs::from_millis(30_000),
    };
    store
        .renew_lease(holder.clone())
        .expect("the exact holder renews");
    assert_eq!(
        store.release_lease(holder).expect("and releases"),
        LeaseState::Released
    );
}

#[test]
fn a_stale_token_or_daemon_epoch_cannot_mutate_ownership() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let first_epoch = store.daemon_epoch();
    let granted = store
        .acquire_lease(AcquireLeaseRequest {
            resource: resource(),
            holder: id(1),
            incarnation: incarnation(4242, "start-a"),
            daemon_epoch: first_epoch,
            ttl: DurationMs::from_millis(30_000),
        })
        .expect("acquiring");
    drop(store);

    // While the lease is still live, nothing takes it over.
    let store = harness.open().expect("reopen inside the liveness window");
    let error = store
        .acquire_lease(AcquireLeaseRequest {
            resource: resource(),
            holder: id(2),
            incarnation: incarnation(5151, "start-c"),
            daemon_epoch: store.daemon_epoch(),
            ttl: DurationMs::from_millis(30_000),
        })
        .expect_err("a live lease holds the resource exclusively");
    assert_eq!(error.conflict_code(), Some("resource_already_leased"));
    drop(store);

    // A much later process finds it expired and takes it over.
    let store = harness
        .open_at(support::DEFAULT_CLOCK_START + 60_000, None)
        .expect("reopen after the liveness window");
    let second_epoch = store.daemon_epoch();
    assert!(second_epoch > first_epoch);
    let taken = store
        .acquire_lease(AcquireLeaseRequest {
            resource: resource(),
            holder: id(2),
            incarnation: incarnation(5151, "start-c"),
            daemon_epoch: second_epoch,
            ttl: DurationMs::from_millis(30_000),
        })
        .expect("an expired lease may be taken over");
    assert_ne!(
        taken.token.expose_bytes(),
        granted.token.expose_bytes(),
        "a takeover mints a fresh token"
    );

    // The superseded holder's token no longer matches.
    let error = store
        .release_lease(LeaseHolderRequest {
            resource: resource(),
            proof: LeaseHolderProof {
                token: granted.token,
                incarnation: incarnation(4242, "start-a"),
                daemon_epoch: first_epoch,
            },
            ttl: DurationMs::from_millis(1),
        })
        .expect_err("the superseded holder cannot release");
    assert_eq!(error.conflict_code(), Some("stale_lease_token"));

    // And a stale daemon epoch cannot mutate the current lease even with the
    // right token and the right incarnation.
    let error = store
        .renew_lease(LeaseHolderRequest {
            resource: resource(),
            proof: LeaseHolderProof {
                token: taken.token,
                incarnation: incarnation(5151, "start-c"),
                daemon_epoch: first_epoch,
            },
            ttl: DurationMs::from_millis(1_000),
        })
        .expect_err("a superseded daemon lifetime is not the owner");
    assert_eq!(error.conflict_code(), Some("stale_daemon_epoch"));
}
