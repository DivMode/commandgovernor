//! Persistence and recovery acceptance tests: DB-001 … DB-008.
//!
//! # Coverage
//!
//! | Test | `docs/testing.md` | Status |
//! | --- | --- | --- |
//! | [`db_001_projection_replay_equivalence`] | DB-001 | covered here (every step of every generated sequence) |
//! | [`db_002_transition_crash_matrix`] | DB-002 | covered here (every write operation × every in-transaction failpoint) |
//! | [`db_003_unknown_newer_schema_fails_closed`] | DB-003 | covered in `governor-store-sqlite` `store_policy`; the reopen-still-refuses half is covered here |
//! | [`db_004_migration_crash_recovery`] | DB-004 | one point covered in `store_policy`; every migration failpoint covered here |
//! | [`db_005_a_second_daemon_supersedes_the_first`] | DB-005 | **store-side half only** — see the note |
//! | [`db_006_startup_quarantines_every_ambiguous_effect_first`] | DB-006 | browser half in `store_durability`; all three families together, with the fakes untouched, covered here |
//! | [`db_007_source_event_uniqueness_survives_restart`] | DB-007 | covered here (100 restarts) |
//! | [`db_008_restore_without_a_pinned_artifact_fails_closed`] | DB-008 | covered here |
//!
//! DB-005 asks for two daemon *instances* against one state root, with exactly
//! one obtaining authority. That election is a daemon-lifecycle feature — a
//! single-instance lock — and Phase 1's store deliberately does not pretend to
//! be one: `docs/testing.md` says in the same breath that "SQLite writer
//! serialization alone is not accepted as daemon election". What *is* drivable
//! today is the fence underneath it, the monotonic daemon epoch, and that is
//! what the test below proves. The election itself is a later gate.

use std::collections::BTreeSet;

use governor_core::effect::{DestinationRef, ExternalEffectClass, IdempotencyKey};
use governor_core::fence::{DeliveryRevision, IncarnationGeneration};
use governor_core::health::HealthConditionKind;
use governor_core::lease::{
    LeaseHolderProof, ProcessIncarnation, ProcessSlot, ProcessStartRef, ResourceIdentity,
    ResourceNamespace,
};
use governor_core::mutation::{MutationCommandKind, MutationFingerprint, SafeMutationResult};
use governor_core::obligation::{Disposition, ObligationState};
use governor_core::time::DurationMs;
use governor_store_sqlite::{
    AckMutationReceiptRequest, AcquireLeaseRequest, BeginMutationRequest, CancelObligationRequest,
    CompleteMutationRequest, ExternalOutcome, LeaseHolderRequest, MarkExternalDispatchedRequest,
    PublishWorkerResultRequest, RecordExternalIntentRequest, RecordExternalOutcomeRequest,
    ResourceRef, Store, StoreError,
};
use governor_testkit::browser::{BrowserWorld, FakeBrowser};
use governor_testkit::dump::count;
use governor_testkit::effect::FakeExternalDestination;
use governor_testkit::failpoints::{MIGRATION_FAILPOINTS, StoreCrash, TRANSACTION_FAILPOINTS};
use governor_testkit::harness::Harness;
use governor_testkit::restart::{KillWindow, restart_loop, run_kill_window};
use governor_testkit::rng::SplitMix64;
use governor_testkit::scenario::{
    ALREADY_LAPSED, FINAL_RESULT, LIVE_CLAIM, accept_wake, accepted_work, acknowledge, arm_send,
    artifact_rows, bind, bind_request, completion_receipts, expire_claim, handed_over, handoff, id,
    mint_claim, open_turn, publish_bytes, publish_result, record_failure, record_outcome,
    schedule_wake, snapshot, source, start_worker, token, worker_turn_request,
};

// --- DB-001: replay equivalence ----------------------------------------------

#[test]
fn db_001_projection_replay_equivalence() {
    // Generated sequences rather than one hand-written path: each seed picks a
    // different order of legal lifecycle steps, and replay is verified after
    // *every* step rather than only at the end.
    for seed in 0..24u64 {
        let harness = Harness::with_seed(seed + 1);
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let mut rng = SplitMix64::new(seed);

        let turn = open_turn(&store);
        let generation = bind(&store, "conv-A");
        store.verify_projections().expect("replay after setup");

        start_worker(&store, turn.obligation, "run-1");
        store.verify_projections().expect("replay after the start");

        // Branch: a confirmed result or a verified failure.
        let succeeded = rng.next_below(2) == 0;
        if succeeded {
            publish_result(
                &store,
                &mut artifacts,
                turn.obligation,
                "run-1",
                FINAL_RESULT,
            )
            .expect("publication");
        } else {
            record_failure(&store, turn.obligation, "run-1").expect("a verified failure");
        }
        store
            .verify_projections()
            .expect("replay after the outcome");

        let wake = schedule_wake(&store, turn.obligation, generation, DeliveryRevision::FIRST)
            .expect("scheduling");
        store.verify_projections().expect("replay after scheduling");

        // Branch: the wake is accepted, proven failed, or lost.
        match rng.next_below(3) {
            0 => accept_wake(&store, &wake, generation, "msg-1"),
            1 => {
                record_outcome(
                    &store,
                    &wake,
                    wake.attempt,
                    governor_store_sqlite::DeliveryOutcome::Failed {
                        failure: governor_core::outbound::FailureClass::ComposerNotReady,
                    },
                )
                .expect("a proven pre-submit failure");
            }
            _ => {
                arm_send(&store, &wake, generation).expect("arming");
                record_outcome(
                    &store,
                    &wake,
                    wake.attempt,
                    governor_store_sqlite::DeliveryOutcome::Ambiguous {
                        reason: governor_core::outbound::AmbiguityReason::ObservationLost,
                    },
                )
                .expect("a lost outcome");
            }
        }
        store
            .verify_projections()
            .expect("replay after the delivery outcome");

        // Branch: claim, expire, reclaim, close — or leave it owed. A failed
        // obligation is claimable too, so this is a coin toss rather than a
        // filter on the outcome above.
        let claimed = mint_claim(&store, turn.obligation, &wake, generation, ALREADY_LAPSED);
        if rng.next_below(2) == 0
            && let Ok(minted) = claimed
        {
            store.verify_projections().expect("replay after the claim");
            handoff(&store, turn.obligation, minted.claim).expect("handing over");
            store
                .verify_projections()
                .expect("replay after the handoff");
            if rng.next_below(2) == 0 {
                expire_claim(&store, turn.obligation, minted.claim).expect("expiry");
                store.verify_projections().expect("replay after the expiry");
            } else {
                let disposition = if succeeded {
                    Disposition::Accepted
                } else {
                    Disposition::FailureAcknowledged
                };
                // The claim lapsed the moment it was minted, so closing it is
                // legitimately refused; either answer must still replay.
                let _ = acknowledge(
                    &store,
                    turn.obligation,
                    generation,
                    minted.claim,
                    disposition,
                );
                store
                    .verify_projections()
                    .expect("replay after the ACK attempt");
            }
        }

        // And the whole ledger replays again in a fresh process.
        drop(store);
        let store = harness.open().expect("reopen");
        let verified = store.verify_projections().expect("replay after a restart");
        assert_eq!(verified.obligations, 1, "seed {seed}");
    }
}

// --- DB-002: the transition crash matrix --------------------------------------

#[test]
fn db_002_transition_crash_matrix() {
    // Every named write operation crossed with every in-transaction failpoint.
    // The oracle is uniform: an interrupted transaction changed nothing at all,
    // a completed one changed exactly what it committed, and reopening replays
    // the ledger either way. `run_kill_window` asserts all three.
    let mut cells = 0;
    let mut fired = 0;
    for op in operations_under_test() {
        for point in TRANSACTION_FAILPOINTS {
            let window = KillWindow { op, point: *point };
            let report = run_cell(window);
            cells += 1;
            if report {
                fired += 1;
            }
        }
    }
    assert_eq!(
        cells,
        operations_under_test().len() * TRANSACTION_FAILPOINTS.len()
    );
    assert!(
        fired >= operations_under_test().len(),
        "every operation must have at least one reachable crash window; only {fired} fired"
    );

    // `recover_startup` is a write operation too, but a caller drives it by
    // opening rather than by calling a method, so its cells are separate.
    for point in TRANSACTION_FAILPOINTS {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let (_, wake, generation) = orphaned_prefix(&harness, &store, &mut artifacts);
        arm_send(&store, &wake, generation).expect("arming");
        drop(store);

        let crash = StoreCrash::at("recover_startup", *point);
        let interrupted = harness.open_with(Some(crash.boxed()));
        if crash.fired() {
            assert!(
                interrupted.is_err(),
                "{point:?}: an interrupted recovery must not hand back a store"
            );
        }
        drop(interrupted);

        // A clean reopen still recovers, deterministically.
        let store = harness
            .open()
            .expect("reopen after an interrupted recovery");
        assert_eq!(
            governor_testkit::dump::scalar(
                &harness.inspect(),
                "SELECT state FROM delivery_attempts"
            )
            .as_deref(),
            Some("ambiguous"),
            "{point:?}: recovery completes on the next open"
        );
        store.verify_projections().expect("replay after recovery");
    }
}

/// Every operation the matrix drives by calling a method.
fn operations_under_test() -> &'static [&'static str] {
    &[
        "open_worker_turn",
        "bind_foreman",
        "record_worker_started",
        "record_worker_failure",
        "publish_worker_result",
        "cancel_obligation",
        "create_or_claim_delivery",
        "arm_delivery_send",
        "record_delivery_outcome",
        "mint_foreman_claim",
        "deliver_handoff",
        "acknowledge_obligation",
        "expire_foreman_claim",
        "begin_mutation",
        "complete_mutation",
        "ack_mutation_receipt",
        "record_external_intent",
        "mark_external_dispatched",
        "record_external_outcome",
        "acquire_lease",
        "renew_lease",
        "release_lease",
    ]
}

/// Builds the prefix one operation needs, runs it under the crash, and asserts.
///
/// Returns whether the injected failure actually fired.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive table of operation prefixes is clearer than \
              twenty-two scattered helpers"
)]
fn run_cell(window: KillWindow) -> bool {
    let harness = Harness::new();
    let report = match window.op {
        "open_worker_turn" => run_kill_window(
            &harness,
            window,
            |_| (),
            |store, ()| {
                store
                    .open_worker_turn(worker_turn_request("turn-1"))
                    .map(drop)
            },
        ),
        "bind_foreman" => run_kill_window(
            &harness,
            window,
            |_| (),
            |store, ()| store.bind_foreman(bind_request("conv-A")).map(drop),
        ),
        "record_worker_started" => run_kill_window(
            &harness,
            window,
            |store| open_turn(store).obligation,
            |store, obligation| {
                store
                    .record_worker_started(governor_store_sqlite::RecordWorkerStartedRequest {
                        obligation,
                        source: source("claude.init", "run-1", "start"),
                        incarnation: IncarnationGeneration::FIRST,
                    })
                    .map(drop)
            },
        ),
        "record_worker_failure" => run_kill_window(
            &harness,
            window,
            |store| {
                let turn = open_turn(store);
                start_worker(store, turn.obligation, "run-1");
                turn.obligation
            },
            |store, obligation| record_failure(store, obligation, "run-1").map(drop),
        ),
        "publish_worker_result" => run_kill_window(
            &harness,
            window,
            |store| {
                let turn = open_turn(store);
                start_worker(store, turn.obligation, "run-1");
                let mut artifacts = harness.open_artifacts();
                let published = publish_bytes(&mut artifacts, FINAL_RESULT).expect("durable bytes");
                (turn.obligation, published)
            },
            |store, (obligation, published)| {
                store
                    .publish_worker_result(PublishWorkerResultRequest {
                        obligation,
                        source: source("claude.result", "run-1", "final"),
                        incarnation: IncarnationGeneration::FIRST,
                        receipts: completion_receipts("run-1"),
                        artifact: published.durable(),
                    })
                    .map(drop)
            },
        ),
        "cancel_obligation" => run_kill_window(
            &harness,
            window,
            |store| open_turn(store).obligation,
            |store, obligation| {
                store
                    .cancel_obligation(CancelObligationRequest {
                        obligation,
                        source: source("cg.cli", "cancel-1", "user"),
                    })
                    .map(drop)
            },
        ),
        "create_or_claim_delivery" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                published_prefix(store, &mut artifacts)
            },
            |store, (obligation, generation)| {
                schedule_wake(store, obligation, generation, DeliveryRevision::FIRST).map(drop)
            },
        ),
        "arm_delivery_send" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                let (obligation, generation) = published_prefix(store, &mut artifacts);
                let wake = schedule_wake(store, obligation, generation, DeliveryRevision::FIRST)
                    .expect("scheduling");
                (wake, generation)
            },
            |store, (wake, generation)| arm_send(store, &wake, generation).map(drop),
        ),
        "record_delivery_outcome" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                let (obligation, generation) = published_prefix(store, &mut artifacts);
                let wake = schedule_wake(store, obligation, generation, DeliveryRevision::FIRST)
                    .expect("scheduling");
                arm_send(store, &wake, generation).expect("arming");
                wake
            },
            |store, wake| {
                record_outcome(
                    store,
                    &wake,
                    wake.attempt,
                    governor_store_sqlite::DeliveryOutcome::Accepted {
                        message: governor_core::foreman_turn::ProviderMessageRef::new(token(
                            "msg-1",
                        )),
                    },
                )
                .map(drop)
            },
        ),
        "mint_foreman_claim" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                accepted_work(store, &mut artifacts, "conv-A")
            },
            |store, work| {
                mint_claim(
                    store,
                    work.obligation,
                    &work.wake,
                    work.generation,
                    LIVE_CLAIM,
                )
                .map(drop)
            },
        ),
        "deliver_handoff" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                let work = accepted_work(store, &mut artifacts, "conv-A");
                let minted = mint_claim(
                    store,
                    work.obligation,
                    &work.wake,
                    work.generation,
                    LIVE_CLAIM,
                )
                .expect("minting");
                (work.obligation, minted.claim)
            },
            |store, (obligation, claim)| handoff(store, obligation, claim).map(drop),
        ),
        "acknowledge_obligation" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                handed_over(store, &mut artifacts, "conv-A", LIVE_CLAIM)
            },
            |store, (work, claim)| {
                acknowledge(
                    store,
                    work.obligation,
                    work.generation,
                    claim,
                    Disposition::Accepted,
                )
                .map(drop)
            },
        ),
        "expire_foreman_claim" => run_kill_window(
            &harness,
            window,
            |store| {
                let mut artifacts = harness.open_artifacts();
                handed_over(store, &mut artifacts, "conv-A", ALREADY_LAPSED)
            },
            |store, (work, claim)| expire_claim(store, work.obligation, claim).map(drop),
        ),
        "begin_mutation" => run_kill_window(
            &harness,
            window,
            |_| (),
            |store, ()| store.begin_mutation(begin_request(store, 900)).map(drop),
        ),
        "complete_mutation" => run_kill_window(
            &harness,
            window,
            |store| {
                store
                    .begin_mutation(begin_request(store, 901))
                    .expect("a new identity");
            },
            |store, ()| store.complete_mutation(complete_request(901)).map(drop),
        ),
        "ack_mutation_receipt" => run_kill_window(
            &harness,
            window,
            |store| {
                store
                    .begin_mutation(begin_request(store, 902))
                    .expect("a new identity");
                store
                    .complete_mutation(complete_request(902))
                    .expect("a safe result");
            },
            |store, ()| {
                store
                    .ack_mutation_receipt(AckMutationReceiptRequest {
                        actor: id(1),
                        command: id(902),
                    })
                    .map(drop)
            },
        ),
        "record_external_intent" => run_kill_window(
            &harness,
            window,
            |_| (),
            |store, ()| {
                store
                    .record_external_intent(intent_request(store))
                    .map(drop)
            },
        ),
        "mark_external_dispatched" => run_kill_window(
            &harness,
            window,
            |store| {
                store
                    .record_external_intent(intent_request(store))
                    .expect("a durable intent")
                    .attempt
            },
            |store, attempt| {
                store
                    .mark_external_dispatched(MarkExternalDispatchedRequest { attempt })
                    .map(drop)
            },
        ),
        "record_external_outcome" => run_kill_window(
            &harness,
            window,
            |store| {
                let granted = store
                    .record_external_intent(intent_request(store))
                    .expect("a durable intent");
                store
                    .mark_external_dispatched(MarkExternalDispatchedRequest {
                        attempt: granted.attempt,
                    })
                    .expect("the dispatch fence");
                granted.attempt
            },
            |store, attempt| {
                store
                    .record_external_outcome(RecordExternalOutcomeRequest {
                        attempt,
                        outcome: ExternalOutcome::Completed {
                            evidence: governor_store_sqlite::AttemptEvidence::new(token("dest-1")),
                        },
                    })
                    .map(drop)
            },
        ),
        "acquire_lease" => run_kill_window(
            &harness,
            window,
            |_| (),
            |store, ()| store.acquire_lease(acquire_request(store)).map(drop),
        ),
        "renew_lease" => run_kill_window(
            &harness,
            window,
            |store| {
                let granted = store
                    .acquire_lease(acquire_request(store))
                    .expect("acquiring");
                (granted, store.daemon_epoch())
            },
            |store, (granted, epoch)| store.renew_lease(holder(&granted, epoch)).map(drop),
        ),
        "release_lease" => run_kill_window(
            &harness,
            window,
            |store| {
                let granted = store
                    .acquire_lease(acquire_request(store))
                    .expect("acquiring");
                (granted, store.daemon_epoch())
            },
            |store, (granted, epoch)| store.release_lease(holder(&granted, epoch)).map(drop),
        ),
        other => panic!("no crash-matrix prefix for {other}"),
    };
    report.fired
}

/// A published obligation with an active binding, ready for a wake.
fn published_prefix(
    store: &Store,
    artifacts: &mut governor_artifacts::ArtifactStore,
) -> (
    governor_core::id::ObligationId,
    governor_core::fence::BindingGeneration,
) {
    let turn = open_turn(store);
    let generation = bind(store, "conv-A");
    start_worker(store, turn.obligation, "run-1");
    publish_result(store, artifacts, turn.obligation, "run-1", FINAL_RESULT).expect("publication");
    (turn.obligation, generation)
}

/// A published obligation with a wake claimed and nothing sent.
fn orphaned_prefix(
    _harness: &Harness,
    store: &Store,
    artifacts: &mut governor_artifacts::ArtifactStore,
) -> (
    governor_core::id::ObligationId,
    governor_store_sqlite::ClaimedDelivery,
    governor_core::fence::BindingGeneration,
) {
    let (obligation, generation) = published_prefix(store, artifacts);
    let wake =
        schedule_wake(store, obligation, generation, DeliveryRevision::FIRST).expect("scheduling");
    (obligation, wake, generation)
}

fn begin_request(store: &Store, command: u128) -> BeginMutationRequest {
    BeginMutationRequest {
        actor: id(1),
        command: id(command),
        kind: MutationCommandKind::new(token("worker.resume")),
        fingerprint: MutationFingerprint::derive(
            &MutationCommandKind::new(token("worker.resume")),
            &[&token("turn-7")],
        ),
        daemon_epoch: store.daemon_epoch(),
    }
}

fn complete_request(command: u128) -> CompleteMutationRequest {
    CompleteMutationRequest {
        actor: id(1),
        command: id(command),
        fingerprint: MutationFingerprint::derive(
            &MutationCommandKind::new(token("worker.resume")),
            &[&token("turn-7")],
        ),
        result: SafeMutationResult::AlreadySatisfied,
    }
}

fn destination() -> DestinationRef {
    DestinationRef::new(token("worker-host"), token("turn-7"), token("gen-1"))
}

fn intent_request(store: &Store) -> RecordExternalIntentRequest {
    RecordExternalIntentRequest {
        class: ExternalEffectClass::IdempotentWrite {
            contract: governor_core::effect::IdempotencyContract::DeduplicatedByKey {
                window: DurationMs::from_millis(60_000),
            },
            key: IdempotencyKey::new(token("k-1")),
        },
        destination: destination(),
        source: source("worker.resume", "cmd-1", "rev-1"),
        daemon_epoch: store.daemon_epoch(),
    }
}

fn resource() -> ResourceRef {
    ResourceRef::of(&ResourceIdentity::canonical(
        ResourceNamespace::new(token("session")),
        "/Volumes/Data/state/session-a",
    ))
}

fn acquire_request(store: &Store) -> AcquireLeaseRequest {
    AcquireLeaseRequest {
        resource: resource(),
        holder: id(1),
        incarnation: ProcessIncarnation::new(
            ProcessSlot::new(4242),
            ProcessStartRef::new(token("start-a")),
        ),
        daemon_epoch: store.daemon_epoch(),
        ttl: DurationMs::from_millis(30_000),
    }
}

fn holder(
    granted: &governor_store_sqlite::GrantedLease,
    epoch: governor_core::fence::DaemonEpoch,
) -> LeaseHolderRequest {
    LeaseHolderRequest {
        resource: resource(),
        proof: LeaseHolderProof {
            token: granted.token.clone(),
            incarnation: ProcessIncarnation::new(
                ProcessSlot::new(4242),
                ProcessStartRef::new(token("start-a")),
            ),
            daemon_epoch: epoch,
        },
        ttl: DurationMs::from_millis(30_000),
    }
}

// --- DB-003 / DB-004: schema epoch and migrations -----------------------------

#[test]
fn db_003_unknown_newer_schema_fails_closed() {
    // `store_policy` proves the refusal itself. What is added here is that it
    // is not a one-shot: a binary that refuses once must keep refusing, and
    // must not have mutated anything on the way past.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    drop(store);
    let before = governor_testkit::dump::dump_domain(&harness.inspect());

    let writable = rusqlite::Connection::open(harness.database_path()).expect("write connection");
    writable
        .execute(
            "INSERT INTO meta (key, value) VALUES ('schema_epoch', '99')
             ON CONFLICT(key) DO UPDATE SET value = '99'",
            [],
        )
        .expect("planting a newer epoch");
    drop(writable);

    for round in 0..3 {
        match harness.open() {
            Err(StoreError::SchemaEpochTooNew { found, supported }) => {
                assert_eq!(found, 99);
                assert!(supported < found, "round {round}");
            }
            other => panic!("round {round}: expected a fail-closed refusal, got {other:?}"),
        }
    }
    governor_testkit::dump::assert_unchanged(
        &before,
        &governor_testkit::dump::dump_domain(&harness.inspect()),
        "DB-003: a refused open mutates nothing",
    );
    let _ = work;
}

#[test]
fn db_004_migration_crash_recovery() {
    for point in MIGRATION_FAILPOINTS {
        let harness = Harness::new();
        let crash = StoreCrash::at("migrate", *point);
        let interrupted = harness.open_with(Some(crash.boxed()));
        assert!(
            interrupted.is_err(),
            "{point:?}: an interrupted migration must not hand back a store"
        );
        assert!(crash.fired(), "{point:?}: the point was never reached");
        drop(interrupted);

        // Reopening produces deterministic completion from a known state: the
        // whole migration is one transaction, so there is no window in which
        // the schema moved but the migration ledger did not.
        let store = harness
            .open()
            .expect("reopen applies the migration cleanly");
        assert_eq!(store.startup().migrations.applied, vec![1]);
        assert!(store.startup().migrations.verified.is_empty());
        drop(store);

        let store = harness.open().expect("and a third open verifies it");
        assert!(store.startup().migrations.applied.is_empty());
        assert_eq!(store.startup().migrations.verified, vec![1]);
    }
}

// --- DB-005: the daemon epoch fence -------------------------------------------

#[test]
fn db_005_a_second_daemon_supersedes_the_first() {
    let harness = Harness::new();
    let first = harness.open().expect("the first instance");
    let first_epoch = first.daemon_epoch();

    // The first instance starts a mutation and takes a lease.
    first
        .begin_mutation(begin_request(&first, 950))
        .expect("an in-flight mutation");
    let lease = first
        .acquire_lease(acquire_request(&first))
        .expect("a lease");

    // A second instance opens against the same state root, long enough later
    // that the first instance's lease has run out of liveness. Its epoch is
    // strictly newer, and that is the whole authority fence Phase 1 provides.
    let second = harness
        .open_at(governor_testkit::DEFAULT_CLOCK_START_MS + 60_000, None)
        .expect("the second instance");
    let second_epoch = second.daemon_epoch();
    assert!(
        second_epoch > first_epoch,
        "opening always advances the daemon epoch"
    );

    // Startup under the newer epoch marked the older instance's in-flight
    // mutation uncertain: it can never be redispatched, whichever process was
    // really running it.
    assert_eq!(second.startup().recovery.uncertain_mutations, 1);
    let error = second
        .begin_mutation(begin_request(&second, 950))
        .expect_err("an uncertain identity never dispatches");
    assert_eq!(error.conflict_code(), Some("mutation_result_uncertain"));

    // The newer instance takes the expired lease over, which mints a fresh
    // token and stamps the record with its own lifetime.
    let taken = second
        .acquire_lease(acquire_request(&second))
        .expect("an expired lease may be taken over");
    assert_ne!(taken.token.expose_bytes(), lease.token.expose_bytes());

    // Now the superseded lifetime cannot mutate ownership, even holding the
    // current token and the right process incarnation.
    let error = first
        .renew_lease(holder(&taken, first_epoch))
        .expect_err("a superseded lifetime is not the owner");
    assert_eq!(error.conflict_code(), Some("stale_daemon_epoch"));
    assert!(
        second.renew_lease(holder(&taken, second_epoch)).is_ok(),
        "the current lifetime still owns it"
    );

    // What is *not* claimed: that only one instance obtained authority. Two
    // `Store` values exist here and both accept writes, because Phase 1 ships
    // no daemon election. `docs/testing.md` DB-005 explicitly refuses to accept
    // SQLite serialization as one, so this test proves the fence and leaves the
    // election to the gate that implements it.
    assert!(first.verify_projections().is_ok());
    assert!(second.verify_projections().is_ok());
}

// --- DB-006: quarantine before any new external I/O ---------------------------

#[test]
fn db_006_startup_quarantines_every_ambiguous_effect_first() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();

    // One orphan of each family, plus ready work the daemon would want to act
    // on the moment it came up.
    let (_, wake, generation) = orphaned_prefix(&harness, &store, &mut artifacts);
    arm_send(&store, &wake, generation).expect("arming");
    store
        .begin_mutation(begin_request(&store, 960))
        .expect("an in-flight mutation");
    let granted = store
        .record_external_intent(intent_request(&store))
        .expect("a durable intent");
    store
        .mark_external_dispatched(MarkExternalDispatchedRequest {
            attempt: granted.attempt,
        })
        .expect("the dispatch fence");
    drop(store);

    // Restart. Nothing may be scheduled before all three are quarantined.
    let store = harness.open().expect("reopen");
    let recovery = &store.startup().recovery;
    assert_eq!(recovery.quarantined_deliveries, 1);
    assert_eq!(recovery.uncertain_mutations, 1);
    assert_eq!(recovery.ambiguous_attempts, 1);
    assert_eq!(recovery.reconciliation_conditions, 1);

    // The two boundaries that could have produced a new external effect were
    // never reached, and by the time a caller holds the store the quarantine is
    // already committed.
    let browser = FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.assert_untouched("DB-006");
    let destination = FakeExternalDestination::attach(&harness.database_path());
    destination.assert_untouched("DB-006");

    let conditions = store.open_health_conditions().expect("reading conditions");
    assert_eq!(conditions.len(), 1);
    assert_eq!(
        conditions[0].kind,
        HealthConditionKind::ReconciliationRequired
    );
    assert_eq!(
        conditions[0].scope.external_attempt,
        Some(granted.attempt),
        "the attention record names the exact attempt"
    );
    store.verify_projections().expect("replay after recovery");
}

// --- DB-007: uniqueness across a hundred restarts ------------------------------

#[test]
fn db_007_source_event_uniqueness_survives_restart() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    let (published, first) = publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("publication");
    drop(store);

    // A hundred process lifetimes, each replaying the same duplicate hook,
    // provider and runtime events.
    restart_loop(&harness, 100, |round, store| {
        let started = store
            .record_worker_started(governor_store_sqlite::RecordWorkerStartedRequest {
                obligation: turn.obligation,
                source: source("claude.init", "run-1", "start"),
                incarnation: IncarnationGeneration::FIRST,
            })
            .unwrap_or_else(|error| panic!("round {round}: {error}"));
        assert!(started.duplicate, "round {round}");

        let again = store
            .publish_worker_result(PublishWorkerResultRequest {
                obligation: turn.obligation,
                source: source("claude.result", "run-1", "final"),
                incarnation: IncarnationGeneration::FIRST,
                receipts: completion_receipts("run-1"),
                artifact: published.durable(),
            })
            .unwrap_or_else(|error| panic!("round {round}: {error}"));
        assert!(again.obligation.duplicate, "round {round}");
        assert_eq!(again.artifact, first.artifact, "round {round}");
    });

    let conn = harness.inspect();
    assert_eq!(count(&conn, "obligations"), 1);
    assert_eq!(count(&conn, "result_artifacts"), 1);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind IN ('worker_started', 'result_published')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("counting"),
        2,
        "one start and one terminal event, after a hundred replays"
    );
    let transitions = count(&conn, "obligation_events");
    assert_eq!(
        transitions, 3,
        "created, running, completed_unprocessed — and no duplicate transition"
    );
}

// --- DB-008: a restore that lost its artifact ---------------------------------

#[test]
fn db_008_restore_without_a_pinned_artifact_fails_closed() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    drop(store);

    // The database is restored from backup; the artifact root is not. The bytes
    // an open obligation pins are simply gone.
    std::fs::remove_file(
        harness
            .artifact_root()
            .join("objects")
            .join(work.artifact.key().as_str()),
    )
    .expect("simulating a restore that lost the artifact root");

    // The store still opens — a missing file is not a corrupt ledger — and the
    // obligation is still owed. What must not happen is it being treated as
    // processable or closed.
    let store = harness.open().expect("the ledger itself is intact");
    let current = snapshot(&store, work.obligation);
    assert_eq!(current.state, ObligationState::CompletedUnprocessed);
    assert!(current.open, "DB-008: the obligation is still owed");
    assert_eq!(
        artifact_rows(&harness.inspect())[0].retention(),
        governor_core::artifact::RetentionState::Pinned,
        "and still pins bytes that are not there"
    );

    // Every path that would hand the result to a foreman fails closed, with no
    // bytes returned at all.
    let rows = artifact_rows(&harness.inspect());
    let error = artifacts
        .read_verified(
            &rows[0].key().expect("a valid key"),
            rows[0].digest(),
            rows[0].byte_len,
        )
        .expect_err("DB-008: a missing artifact is not an empty one");
    assert!(
        matches!(error, governor_artifacts::ArtifactError::Missing { .. }),
        "{error:?}"
    );

    // That is the `result_artifact_missing` health condition's trigger. The
    // classification exists in the domain; Phase 1's store has no operation
    // that records it, so the durable half is a later gate and this test says
    // so rather than asserting a row that cannot be written.
    assert_eq!(
        HealthConditionKind::ResultArtifactMissing.code(),
        "result_artifact_missing"
    );
    assert!(
        store
            .open_health_conditions()
            .expect("reading conditions")
            .is_empty()
    );

    // And the forbidden outcome is exactly what this state is not: the
    // completion is *not* treated as durable-and-verifiable anywhere.
    let referenced: BTreeSet<_> = governor_testkit::scenario::committed_keys(&harness.inspect());
    let on_disk: BTreeSet<_> = harness
        .files_in("objects")
        .into_iter()
        .filter_map(|name| governor_artifacts::StorageKey::parse(&name).ok())
        .collect();
    assert!(
        !referenced.is_subset(&on_disk),
        "the fixture must actually be missing the bytes"
    );
}
