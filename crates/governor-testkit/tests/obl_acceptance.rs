//! Obligation acceptance tests: OBL-001 … OBL-010.
//!
//! # Coverage
//!
//! | Test | `docs/testing.md` | Status |
//! | --- | --- | --- |
//! | [`obl_001_completion_cannot_disappear_before_ack`] | OBL-001 | covered here |
//! | [`obl_002_restart_preserves_open_attention_states`] | OBL-002 | covered here (store-reachable states) |
//! | [`obl_002_claim_expiry_restores_needs_input`] | OBL-002 (`needs_input`) | covered in `governor-core` `obligation::tests::claim_expiry_restores_attention_and_never_closes`; asserted here as unreachable through the store |
//! | [`obl_003_stale_binding_generation_cannot_ack`] | OBL-003 | covered here |
//! | [`obl_004_stale_claim_cannot_ack`] | OBL-004 | reclaim case covered in `governor-store-sqlite` `store_lifecycle`; the expired-without-reclaim case is covered here |
//! | [`obl_005_duplicate_terminal_source_event_is_idempotent`] | OBL-005 | covered here (100 replays across 10 restarts) |
//! | [`obl_006_conflicting_terminal_evidence_is_visible`] | OBL-006 | covered here: no second obligation, and a durable turn-scoped reconciliation condition |
//! | [`obl_007_physical_settlement_is_not_ack`] | OBL-007 | covered here |
//! | [`obl_008_mcp_result_handoff_is_not_ack`] | OBL-008 | covered here |
//! | [`obl_009_ack_requires_exact_source_and_version`] | OBL-009 | covered here |
//! | [`obl_010_failure_is_unprocessed_work`] | OBL-010 | covered here |

use governor_core::foreman_turn::{ForemanTurn, ForemanTurnEvent, ForemanTurnState};
use governor_core::health::{HealthConditionKind, HealthScope};
use governor_core::obligation::{Disposition, ObligationState};
use governor_core::time::Timestamp;
use governor_core::worker_evidence::{
    ChildExitReceipt, ChildExitStatus, FinalResultReceipt, ManagedRunEvidence, ManagedRunOutcome,
    RuntimeObservation, WorkerOutcome,
};
use governor_store_sqlite::{
    AcknowledgeRequest, CompletionReceipts, OpenCondition, PublishWorkerResultRequest, Store,
    StoreResult, TerminalEvidenceConflictRequest,
};
use governor_testkit::clock::DEFAULT_CLOCK_START_MS;
use governor_testkit::dump::{assert_unchanged, count, dump_domain, scalar};
use governor_testkit::harness::Harness;
use governor_testkit::scenario::{
    AcceptedWork, FINAL_RESULT, LIVE_CLAIM, RETENTION_GRACE, accept_wake, accepted_work,
    acknowledge, bind, completion_receipts, expire_claim, handed_over, handoff, id, lapse_claim,
    mint_claim, open_named_turn, open_turn, publish_result, record_failure, schedule_wake,
    snapshot, source, start_worker,
};

/// The retention state of the single committed artifact.
fn retention(harness: &Harness) -> Option<String> {
    scalar(
        &harness.inspect(),
        "SELECT retention_state FROM result_artifacts",
    )
}

/// Asserts an obligation is still owed, with its artifact still pinned.
fn assert_still_owed(harness: &Harness, store: &Store, work: &AcceptedWork, context: &str) {
    let current = snapshot(store, work.obligation);
    assert!(current.open, "{context}: the obligation must stay open");
    assert_eq!(
        current.state,
        ObligationState::CompletedUnprocessed,
        "{context}: the confirmed result must still be awaiting review"
    );
    assert_eq!(
        retention(harness).as_deref(),
        Some("pinned"),
        "{context}: an open obligation pins its artifact"
    );
}

#[test]
fn obl_001_completion_cannot_disappear_before_ack() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    assert_still_owed(&harness, &store, &work, "immediately after publication");

    // The bytes really are on disk and really do verify.
    let stored = artifacts
        .read_verified(
            work.artifact.key(),
            work.artifact.digest(),
            work.artifact.byte_len(),
        )
        .expect("the published artifact reads back");
    assert_eq!(stored, FINAL_RESULT);
    drop(store);

    // 1. Restart the daemon and the store, repeatedly.
    for round in 0..3 {
        let store = harness.open().expect("reopen");
        assert_still_owed(&harness, &store, &work, &format!("after restart {round}"));
        store.verify_projections().expect("replay equivalence");
    }

    let opened = harness
        .open_full(DEFAULT_CLOCK_START_MS, None)
        .expect("reopen");
    let store = opened.store;
    let before = dump_domain(&harness.inspect());

    // 2. Close and delete the fake runtime session. A transport observation is
    //    the weakest evidence class there is and has no path to a terminal
    //    worker outcome, so nothing durable moves.
    for observation in [
        RuntimeObservation::Idle,
        RuntimeObservation::Ended,
        RuntimeObservation::Working,
    ] {
        let classified = ManagedRunEvidence::new()
            .with_runtime(observation)
            .classify();
        assert_eq!(
            classified,
            WorkerOutcome::Indeterminate,
            "a runtime transport sample is never terminal worker evidence"
        );
    }
    assert_still_owed(&harness, &store, &work, "after the runtime session closed");

    // 3. Settle a fake ChatGPT turn. Accepted != settled != ACK.
    let settled = settle_turn();
    assert_eq!(settled.state(), ForemanTurnState::Settled);
    assert_still_owed(&harness, &store, &work, "after physical settlement");
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "neither a runtime observation nor a physical settlement is durable",
    );

    // 4. Expire a foreman claim. Internal coordination, never a decision. The
    //    handoff happens while the claim is live — a lapsed claim authorises
    //    nothing — and the claim lapses afterwards by moving the clock.
    let minted = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("minting a claim from the accepted wake");
    handoff(&store, work.obligation, minted.claim).expect("handing the result over");
    lapse_claim(&opened.clock);
    let expired = expire_claim(&store, work.obligation, minted.claim).expect("a lapsed claim");
    assert!(expired.obligation.state.is_open());
    assert_still_owed(&harness, &store, &work, "after claim expiry");

    // Only now, and only with every fence, does the work close.
    let reclaim = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("an expired claim may be replaced");
    handoff(&store, work.obligation, reclaim.claim).expect("handing over again");
    let acked = acknowledge(
        &store,
        work.obligation,
        work.generation,
        reclaim.claim,
        Disposition::Accepted,
    )
    .expect("a fully fenced ACK");
    assert_eq!(acked.obligation.state, ObligationState::Acknowledged);
    assert!(!snapshot(&store, work.obligation).open);
    assert_eq!(retention(&harness).as_deref(), Some("eligible"));
}

/// A physical assistant turn observed all the way to settlement.
fn settle_turn() -> ForemanTurn {
    let generation = governor_core::fence::BindingGeneration::FIRST;
    let mut turn = ForemanTurn::unobserved(id(900), generation);
    for event in [
        ForemanTurnEvent::Started {
            binding_generation: generation,
            trigger: None,
            at: Timestamp::from_unix_millis(1),
        },
        ForemanTurnEvent::BecameActive {
            binding_generation: generation,
            at: Timestamp::from_unix_millis(2),
        },
        ForemanTurnEvent::Settled {
            binding_generation: generation,
            at: Timestamp::from_unix_millis(3),
        },
    ] {
        turn = turn
            .apply(&event)
            .expect("an observation from the current generation")
            .or_unchanged(turn.clone());
    }
    turn
}

#[test]
fn obl_002_restart_preserves_open_attention_states() {
    // Every state the Phase 1 store can actually reach. `needs_input` is
    // deliberately absent — there is no input-boundary event kind, so no store
    // write path leads there — and is covered by the pure machine instead; see
    // `obl_002_claim_expiry_restores_needs_input`.
    for target in [
        ObligationState::Created,
        ObligationState::Running,
        ObligationState::Failed,
        ObligationState::CompletedUnprocessed,
        ObligationState::ClaimedByForeman,
        ObligationState::Processing,
    ] {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let obligation = drive_to(&store, &mut artifacts, target);

        let before = snapshot(&store, obligation);
        assert_eq!(before.state, target);
        assert!(before.open, "{target:?} is an open state");
        let rows = dump_domain(&harness.inspect());
        drop(store);

        let store = harness.open().expect("reopen");
        let after = snapshot(&store, obligation);
        assert_eq!(
            before, after,
            "{target:?}: state, version, source fence, claim and artifact must survive a restart"
        );
        assert_unchanged(
            &rows,
            &dump_domain(&harness.inspect()),
            &format!("{target:?}: a restart is not a mutation"),
        );
        let verified = store.verify_projections().expect("replay equivalence");
        assert_eq!(verified.obligations, 1);
    }
}

/// Drives one fresh obligation to a target state through the real store.
fn drive_to(
    store: &Store,
    artifacts: &mut governor_artifacts::ArtifactStore,
    target: ObligationState,
) -> governor_core::id::ObligationId {
    match target {
        ObligationState::Created => open_turn(store).obligation,
        ObligationState::Running => {
            let turn = open_turn(store);
            start_worker(store, turn.obligation, "run-1");
            turn.obligation
        }
        ObligationState::Failed => {
            let turn = open_turn(store);
            start_worker(store, turn.obligation, "run-1");
            record_failure(store, turn.obligation, "run-1").expect("a verified failure");
            turn.obligation
        }
        ObligationState::CompletedUnprocessed => {
            let turn = open_turn(store);
            start_worker(store, turn.obligation, "run-1");
            publish_result(store, artifacts, turn.obligation, "run-1", FINAL_RESULT)
                .expect("publication");
            turn.obligation
        }
        ObligationState::ClaimedByForeman => {
            let work = accepted_work(store, artifacts, "conv-A");
            mint_claim(
                store,
                work.obligation,
                &work.wake,
                work.generation,
                LIVE_CLAIM,
            )
            .expect("minting a claim");
            work.obligation
        }
        ObligationState::Processing => {
            let (work, _claim) = handed_over(store, artifacts, "conv-A", LIVE_CLAIM);
            work.obligation
        }
        other => panic!("{other:?} is not reachable through the Phase 1 store"),
    }
}

#[test]
fn obl_002_claim_expiry_restores_needs_input() {
    // The state itself is unreachable through the store, and this test says so
    // rather than quietly skipping it: nothing in the ledger's event vocabulary
    // can produce an input boundary. The rule that an expired claim returns an
    // obligation to `needs_input` — and never closes it — is proven in
    // `governor-core`, where the machine that owns it lives.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    let before = dump_domain(&harness.inspect());

    // A confirmed defer is a real, constructible domain value; what is missing
    // is a store operation that would commit it.
    let defer = ManagedRunEvidence::new()
        .with_final_result(FinalResultReceipt {
            run_ref: governor_testkit::scenario::token("run-1"),
            complete: true,
            outcome: ManagedRunOutcome::ToolDeferred,
        })
        .with_child_exit(ChildExitReceipt {
            run_ref: governor_testkit::scenario::token("run-1"),
            status: ChildExitStatus::Success,
        })
        .classify();
    assert!(
        matches!(defer, WorkerOutcome::ConfirmedDeferred(_)),
        "the evidence for `needs_input` classifies, so only the durable half is missing"
    );

    assert_eq!(
        snapshot(&store, turn.obligation).state,
        ObligationState::Running,
        "no store operation moved the obligation to needs_input"
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "classifying a defer commits nothing",
    );
}

#[test]
fn obl_003_stale_binding_generation_cannot_ack() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (work, claim) = handed_over(&store, &mut artifacts, "conv-A", LIVE_CLAIM);

    // Rebind: generation N is superseded while the claim is still live.
    let newer = bind(&store, "conv-B");
    assert!(newer > work.generation);

    let processing = snapshot(&store, work.obligation);
    let before = dump_domain(&harness.inspect());
    let error = store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation: work.obligation,
            expected_version: processing.version,
            expected_source: processing.source.clone(),
            binding_generation: work.generation,
            claim,
            disposition: Disposition::Accepted,
            retention_grace: RETENTION_GRACE,
        })
        .expect_err("a superseded generation cannot close current work");
    assert_eq!(error.conflict_code(), Some("stale_binding_generation"));

    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "OBL-003: a refused ACK changes zero rows",
    );
    assert_eq!(snapshot(&store, work.obligation), processing);
    assert_eq!(
        retention(&harness).as_deref(),
        Some("pinned"),
        "the artifact is still required"
    );
}

#[test]
fn obl_004_stale_claim_cannot_ack() {
    // The reclaim case — expire, mint a second claim, then ACK with the first —
    // is proven in `governor-store-sqlite`'s `store_lifecycle`. The remaining
    // case is the one with no second claim at all: an expired claim that is
    // still the last one anybody minted must not be able to close the work.
    let harness = Harness::new();
    let opened = harness
        .open_full(DEFAULT_CLOCK_START_MS, None)
        .expect("opening");
    let store = opened.store;
    let mut artifacts = harness.open_artifacts();
    let (work, claim) = handed_over(&store, &mut artifacts, "conv-A", LIVE_CLAIM);
    lapse_claim(&opened.clock);

    let processing = snapshot(&store, work.obligation);
    let before = dump_domain(&harness.inspect());
    let error = store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation: work.obligation,
            expected_version: processing.version,
            expected_source: processing.source.clone(),
            binding_generation: work.generation,
            claim,
            disposition: Disposition::Accepted,
            retention_grace: RETENTION_GRACE,
        })
        .expect_err("an elapsed claim no longer authorises a mutation");
    assert_eq!(error.conflict_code(), Some("expired_claim"));
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "OBL-004: a refused ACK changes zero rows",
    );

    // And after the expiry is recorded, the same claim is stale rather than
    // merely late: the obligation is no longer held at all.
    expire_claim(&store, work.obligation, claim).expect("a lapsed claim expires");
    let restored = snapshot(&store, work.obligation);
    let before = dump_domain(&harness.inspect());
    let error = store
        .acknowledge_obligation(AcknowledgeRequest {
            obligation: work.obligation,
            expected_version: restored.version,
            expected_source: restored.source.clone(),
            binding_generation: work.generation,
            claim,
            disposition: Disposition::Accepted,
            retention_grace: RETENTION_GRACE,
        })
        .expect_err("a released claim cannot close the work");
    assert_eq!(
        snapshot(&store, work.obligation).state,
        ObligationState::CompletedUnprocessed,
        "the obligation went back to the attention it was claimed from"
    );
    assert_eq!(
        error.conflict_code(),
        Some("expired_claim"),
        "the claim's own lifetime is checked before the obligation's state, so a \
         released claim is refused as expired rather than as an illegal transition"
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "OBL-004: still zero rows",
    );
    assert!(snapshot(&store, work.obligation).open);
}

#[test]
fn obl_005_duplicate_terminal_source_event_is_idempotent() {
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
    assert!(!first.obligation.duplicate);
    drop(store);

    // Ten replays in each of ten incarnations: a hundred deliveries of the
    // exact same confirmed terminal fact.
    let replay = |store: &Store| -> StoreResult<()> {
        for _ in 0..10 {
            let again = store.publish_worker_result(PublishWorkerResultRequest {
                obligation: turn.obligation,
                source: source("claude.result", "run-1", "final"),
                incarnation: governor_core::fence::IncarnationGeneration::FIRST,
                receipts: completion_receipts("run-1"),
                artifact: published.durable(),
            })?;
            assert!(again.obligation.duplicate, "a replay is never a transition");
            assert_eq!(
                again.artifact, first.artifact,
                "and never a second artifact"
            );
        }
        Ok(())
    };
    for round in 0..10 {
        let store = harness.open().expect("reopen");
        replay(&store).unwrap_or_else(|error| panic!("round {round}: {error}"));
        store.verify_projections().expect("replay equivalence");
    }

    let conn = harness.inspect();
    assert_eq!(count(&conn, "result_artifacts"), 1);
    assert_eq!(count(&conn, "obligations"), 1);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind = 'result_published'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("counting terminal events"),
        1,
        "exactly one terminal event exists"
    );
    let store = harness.open().expect("reopen");
    assert_eq!(
        snapshot(&store, turn.obligation).state,
        ObligationState::CompletedUnprocessed
    );
}

#[test]
fn obl_006_conflicting_terminal_evidence_is_visible() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let turn = open_turn(&store);
    start_worker(&store, turn.obligation, "run-1");
    publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("publication");

    // Contradictory terminal evidence for the same turn: a verified failure
    // arriving after a confirmed success.
    let before = dump_domain(&harness.inspect());
    let error = record_failure(&store, turn.obligation, "run-1-contradiction")
        .expect_err("a second terminal fact cannot apply");
    assert_eq!(error.conflict_code(), Some("illegal_obligation_transition"));

    // No second obligation, and no mutation of the first.
    assert_eq!(count(&harness.inspect(), "obligations"), 1);
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "OBL-006: contradictory evidence creates nothing",
    );

    // The arbitration half of "the conflict is visible" is pure, and it is
    // reconciliation attention rather than a decision.
    let contradictory = ManagedRunEvidence::new()
        .with_final_result(FinalResultReceipt {
            run_ref: governor_testkit::scenario::token("run-1"),
            complete: true,
            outcome: ManagedRunOutcome::Success,
        })
        .with_child_exit(ChildExitReceipt {
            run_ref: governor_testkit::scenario::token("run-1"),
            status: ChildExitStatus::Nonzero { code: 1 },
        })
        .classify();
    assert_eq!(
        contradictory,
        WorkerOutcome::NeedsReconciliation(HealthConditionKind::RuntimeStateConflict),
        "a success record with a failing exit is reconciliation, never a second result"
    );

    // And the durable half. The store runs the same arbitration — the caller
    // hands in receipts, never a conclusion — and records the class it returned
    // against the turn.
    let recorded = store
        .record_terminal_evidence_conflict(TerminalEvidenceConflictRequest {
            obligation: turn.obligation,
            receipts: CompletionReceipts {
                run_ref: governor_testkit::scenario::token("run-1"),
                final_result_complete: true,
                outcome: ManagedRunOutcome::Success,
                child_exit: ChildExitStatus::Nonzero { code: 1 },
            },
        })
        .expect("contradictory evidence for a turn that already has a result");
    assert!(!recorded.duplicate);
    assert_eq!(
        store.open_health_conditions().expect("reading conditions"),
        vec![OpenCondition {
            kind: HealthConditionKind::RuntimeStateConflict,
            scope: HealthScope::turn(turn.turn),
        }],
        "OBL-006: a durable reconciliation condition, scoped to the turn"
    );

    // It is attention and nothing else: no second obligation, no transition,
    // and no closure. And it is raised once, not once per report.
    let after_raise = dump_domain(&harness.inspect());
    assert_eq!(count(&harness.inspect(), "obligations"), 1);
    let current = snapshot(&store, turn.obligation);
    assert_eq!(current.state, ObligationState::CompletedUnprocessed);
    assert!(current.open, "OBL-006: attention never closes the work");
    for _ in 0..5 {
        assert!(
            store
                .record_terminal_evidence_conflict(TerminalEvidenceConflictRequest {
                    obligation: turn.obligation,
                    receipts: CompletionReceipts {
                        run_ref: governor_testkit::scenario::token("run-1"),
                        final_result_complete: true,
                        outcome: ManagedRunOutcome::Success,
                        child_exit: ChildExitStatus::Nonzero { code: 1 },
                    },
                })
                .expect("a repeat is convergence")
                .duplicate
        );
    }
    assert_unchanged(
        &after_raise,
        &dump_domain(&harness.inspect()),
        "OBL-006: one condition, not one per contradictory report",
    );

    // Evidence that is *not* contradictory cannot open one, so the ledger
    // cannot be filled with conflicts that were never observed.
    let error = store
        .record_terminal_evidence_conflict(TerminalEvidenceConflictRequest {
            obligation: turn.obligation,
            receipts: completion_receipts("run-1"),
        })
        .expect_err("a confirmed completion is not a conflict");
    assert_eq!(error.conflict_code(), Some("illegal_obligation_transition"));
    assert_unchanged(
        &after_raise,
        &dump_domain(&harness.inspect()),
        "OBL-006: a refused report changes nothing",
    );

    store
        .verify_projections()
        .expect("the condition replays from its event");
}

#[test]
fn obl_007_physical_settlement_is_not_ack() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    let before = dump_domain(&harness.inspect());
    let settled = settle_turn();
    assert_eq!(settled.state(), ForemanTurnState::Settled);
    assert!(
        settled.permits_new_wake(),
        "a settled surface is quiescent again"
    );

    assert_still_owed(&harness, &store, &work, "OBL-007");
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "OBL-007: settlement is not a durable fact about the obligation",
    );
}

#[test]
fn obl_008_mcp_result_handoff_is_not_ack() {
    let harness = Harness::new();
    let opened = harness
        .open_full(DEFAULT_CLOCK_START_MS, None)
        .expect("opening");
    let store = opened.store;
    let mut artifacts = harness.open_artifacts();
    let (work, claim) = handed_over(&store, &mut artifacts, "conv-A", LIVE_CLAIM);
    assert_eq!(
        snapshot(&store, work.obligation).state,
        ObligationState::Processing
    );

    // Every page of the result is returned to the claiming foreman. The store
    // holds one bounded final result, so "every page" is the whole artifact,
    // read back and verified.
    let pages = artifacts
        .read_verified(
            work.artifact.key(),
            work.artifact.digest(),
            work.artifact.byte_len(),
        )
        .expect("returning the result to the foreman");
    assert_eq!(pages, FINAL_RESULT);

    // The client disconnects. Nothing about that is a decision.
    let current = snapshot(&store, work.obligation);
    assert!(current.open, "OBL-008: a handoff is not an ACK");
    assert_eq!(retention(&harness).as_deref(), Some("pinned"));

    // And after the claim's bounded lifetime lapses it may be reclaimed.
    lapse_claim(&opened.clock);
    expire_claim(&store, work.obligation, claim).expect("a lapsed claim expires");
    let reclaim = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("OBL-008: the obligation may later be reclaimed");
    assert_ne!(reclaim.claim, claim);
    assert!(snapshot(&store, work.obligation).open);
}

#[test]
fn obl_009_ack_requires_exact_source_and_version() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (work, claim) = handed_over(&store, &mut artifacts, "conv-A", LIVE_CLAIM);

    // A second, independent obligation with its own confirmed result. It is the
    // "result identity" fence: `foreman_ack` carries no artifact field, so the
    // way to present the wrong result is to present the wrong obligation.
    let other = open_named_turn(&store, "turn-2");
    start_worker(&store, other.obligation, "run-2");
    publish_result(
        &store,
        &mut artifacts,
        other.obligation,
        "run-2",
        FINAL_RESULT,
    )
    .expect("a second publication");

    let processing = snapshot(&store, work.obligation);
    let good = AcknowledgeRequest {
        obligation: work.obligation,
        expected_version: processing.version,
        expected_source: processing.source.clone(),
        binding_generation: work.generation,
        claim,
        disposition: Disposition::Accepted,
        retention_grace: RETENTION_GRACE,
    };

    let variants: Vec<(&str, AcknowledgeRequest, &str)> = vec![
        (
            "obligation / result identity",
            AcknowledgeRequest {
                obligation: other.obligation,
                ..good.clone()
            },
            "stale_claim",
        ),
        (
            "obligation version",
            AcknowledgeRequest {
                expected_version: governor_core::fence::ObligationVersion::FIRST,
                ..good.clone()
            },
            "stale_obligation_version",
        ),
        (
            "source event",
            AcknowledgeRequest {
                expected_source: source("claude.result", "run-0", "final"),
                ..good.clone()
            },
            "stale_source_fence",
        ),
        (
            "binding generation",
            AcknowledgeRequest {
                binding_generation: governor_core::fence::BindingGeneration::new(99),
                ..good.clone()
            },
            "unknown_binding_generation",
        ),
        (
            "claim",
            AcknowledgeRequest {
                claim: id(4242),
                ..good.clone()
            },
            "stale_claim",
        ),
        (
            "disposition",
            AcknowledgeRequest {
                disposition: Disposition::FailureAcknowledged,
                ..good.clone()
            },
            "invalid_disposition",
        ),
    ];

    for (field, request, expected) in variants {
        let before = dump_domain(&harness.inspect());
        let error = store
            .acknowledge_obligation(request)
            .expect_err("a stale variant must not close the work");
        assert_eq!(
            error.conflict_code(),
            Some(expected),
            "OBL-009: varying the {field} must be refused"
        );
        assert_unchanged(
            &before,
            &dump_domain(&harness.inspect()),
            &format!("OBL-009: a refused ACK varying the {field} changes zero rows"),
        );
    }

    // The fully fenced request still closes the work, so the six refusals above
    // were about the fences and not about some unrelated obstacle.
    let acked = store
        .acknowledge_obligation(good)
        .expect("every fence presented exactly");
    assert_eq!(acked.obligation.state, ObligationState::Acknowledged);
}

#[test]
fn obl_010_failure_is_unprocessed_work() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    record_failure(&store, turn.obligation, "run-1").expect("a verified terminal failure");

    let failed = snapshot(&store, turn.obligation);
    assert_eq!(failed.state, ObligationState::Failed);
    assert!(failed.open, "OBL-010: a failure is unprocessed work");

    // A runtime close cannot discard it.
    assert_eq!(
        ManagedRunEvidence::new()
            .with_runtime(RuntimeObservation::Ended)
            .classify(),
        WorkerOutcome::Indeterminate
    );
    drop(store);

    // A restart cannot discard it.
    let store = harness.open().expect("reopen");
    assert_eq!(snapshot(&store, turn.obligation), failed);

    // Nor can an assistant settlement.
    let wake = schedule_wake(
        &store,
        turn.obligation,
        generation,
        governor_core::fence::DeliveryRevision::FIRST,
    )
    .expect("a failure is wake-eligible work");
    accept_wake(&store, &wake, generation, "msg-1");
    assert_eq!(settle_turn().state(), ForemanTurnState::Settled);
    assert!(snapshot(&store, turn.obligation).open);

    // Only a disposition that matches the attention closes it.
    let minted = mint_claim(&store, turn.obligation, &wake, generation, LIVE_CLAIM)
        .expect("minting a claim");
    handoff(&store, turn.obligation, minted.claim).expect("handing the failure over");
    let error = acknowledge(
        &store,
        turn.obligation,
        generation,
        minted.claim,
        Disposition::Accepted,
    )
    .expect_err("a success disposition cannot close a failure");
    assert_eq!(error.conflict_code(), Some("invalid_disposition"));

    let acked = acknowledge(
        &store,
        turn.obligation,
        generation,
        minted.claim,
        Disposition::FailureAcknowledged,
    )
    .expect("the matching disposition closes it");
    assert_eq!(acked.obligation.state, ObligationState::Acknowledged);
}
