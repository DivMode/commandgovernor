//! ChatGPT processing / MCP acceptance tests: GPT-001 … GPT-009.
//!
//! Fake foreman/MCP state only. No live surface is contacted and none of these
//! tests may be read as evidence about one: that is Live Gate A.
//!
//! # Coverage
//!
//! | Test | `docs/testing.md` | Status |
//! | --- | --- | --- |
//! | [`gpt_001_accepted_is_not_processed`] | GPT-001 | covered here |
//! | [`gpt_002_physical_settlement_is_not_processed`] | GPT-002 | covered here |
//! | [`gpt_003_resume_claim_without_ack_stays_open`] | GPT-003 | covered here |
//! | [`gpt_004_bounded_resume_creates_a_new_revision`] | GPT-004 | covered here |
//! | [`gpt_005_never_overlap_an_active_or_unknown_turn`] | GPT-005 | covered here (pure turn machine plus the scheduling gate) |
//! | [`gpt_006_resume_budget_exhausts_safely`] | GPT-006 | covered here: budget, zero further sends, the open obligation, and the durable `foreman_unreachable` condition with its idempotence, restart survival and resolution |
//! | [`gpt_007_bootstrap_is_low_information`] | GPT-007, SEC-002 | covered here |
//! | [`gpt_008_unrelated_connector_cannot_claim_from_bootstrap`] | GPT-008 | covered here |
//! | [`gpt_009_current_accepted_wake_can_claim`] | GPT-009 | covered here |
//!
//! GPT-010 … GPT-012 are skipped deliberately. They are about the connector ABI
//! negotiated with a live surface, the loss of a write capability that was
//! proven against one, and a product confirmation dialog that only exists in
//! one. Phase 1 builds no MCP client and no connector, so there is no ABI to
//! mismatch and no write action to lose; `docs/testing.md` places all three
//! behind Live Gate A. The one half that is representable today —
//! `WriteCapabilityState` never relaxing the ACK requirement — is proven in
//! `governor-core` `binding::tests::capability_loss_is_recorded_without_closing_anything`.

use governor_core::fence::{BindingGeneration, DeliveryRevision, ObligationVersion};
use governor_core::foreman_turn::{ForemanTurn, ForemanTurnEvent, ForemanTurnState};
use governor_core::health::{HealthConditionKind, HealthLedger, HealthScope};
use governor_core::obligation::ObligationState;
use governor_core::outbound::DeliveryState;
use governor_core::time::{DurationMs, Timestamp};
use governor_store_sqlite::{MintClaimRequest, OpenCondition, RaiseForemanUnreachableRequest};
use governor_testkit::browser::{BrowserWorld, FakeBrowser, deliver_wake};
use governor_testkit::dump::{assert_unchanged, count, dump_domain};
use governor_testkit::foreman::{ResumeBudget, ResumeDecision, WakeGate, bootstrap};
use governor_testkit::harness::Harness;
use governor_testkit::scenario::{
    ALREADY_LAPSED, AcceptedWork, FINAL_RESULT, LIVE_CLAIM, accepted_work, expire_claim, handoff,
    id, mint_claim, schedule_wake, snapshot,
};

/// The instant every bootstrap in this suite is taken at.
const NOW: Timestamp = Timestamp::from_unix_millis(500_000);

/// A physical assistant turn driven to `state`.
fn turn_in(state: ForemanTurnState) -> ForemanTurn {
    let generation = BindingGeneration::FIRST;
    let turn = ForemanTurn::unobserved(id(900), generation);
    let events: &[ForemanTurnEvent] = match state {
        ForemanTurnState::IdleUnknown => &[],
        ForemanTurnState::Starting => &[ForemanTurnEvent::Started {
            binding_generation: BindingGeneration::FIRST,
            trigger: None,
            at: Timestamp::from_unix_millis(1),
        }],
        ForemanTurnState::Active => &[ForemanTurnEvent::BecameActive {
            binding_generation: BindingGeneration::FIRST,
            at: Timestamp::from_unix_millis(2),
        }],
        ForemanTurnState::Settled => &[ForemanTurnEvent::Settled {
            binding_generation: BindingGeneration::FIRST,
            at: Timestamp::from_unix_millis(3),
        }],
        ForemanTurnState::ObservationLost => &[ForemanTurnEvent::ObservationLost {
            binding_generation: BindingGeneration::FIRST,
            at: Timestamp::from_unix_millis(4),
        }],
    };
    events.iter().fold(turn, |turn, event| {
        turn.apply(event)
            .expect("an observation from the current generation")
            .or_unchanged(turn.clone())
    })
}

#[test]
fn gpt_001_accepted_is_not_processed() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // The wake landed. No MCP call has happened.
    assert_eq!(
        governor_testkit::dump::scalar(&harness.inspect(), "SELECT state FROM browser_deliveries")
            .as_deref(),
        Some("accepted")
    );
    assert_eq!(count(&harness.inspect(), "foreman_claims"), 0);

    let current = snapshot(&store, work.obligation);
    assert_eq!(current.state, ObligationState::CompletedUnprocessed);
    assert!(current.open, "GPT-001: accepted is not processed");
}

#[test]
fn gpt_002_physical_settlement_is_not_processed() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    let before = dump_domain(&harness.inspect());
    let settled = turn_in(ForemanTurnState::Settled);
    assert_eq!(settled.state(), ForemanTurnState::Settled);

    assert!(snapshot(&store, work.obligation).open);
    assert_eq!(count(&harness.inspect(), "foreman_claims"), 0);
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "GPT-002: a settled assistant turn is not a durable fact about the work",
    );
}

#[test]
fn gpt_003_resume_claim_without_ack_stays_open() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // A successful `foreman_resume`, and every page of the result returned.
    let minted = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("the accepted wake claims");
    handoff(&store, work.obligation, minted.claim).expect("handing the result over");
    let pages = artifacts
        .read_verified(
            work.artifact.key(),
            work.artifact.digest(),
            work.artifact.byte_len(),
        )
        .expect("every page of the result");
    assert_eq!(pages, FINAL_RESULT);

    // The assistant settles. Still no ACK.
    assert_eq!(
        turn_in(ForemanTurnState::Settled).state(),
        ForemanTurnState::Settled
    );
    let current = snapshot(&store, work.obligation);
    assert_eq!(current.state, ObligationState::Processing);
    assert!(current.open, "GPT-003: processing is not closed");
}

#[test]
fn gpt_004_bounded_resume_creates_a_new_revision() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // Accepted, settled, no ACK, and the policy delay has passed.
    assert_eq!(
        turn_in(ForemanTurnState::Settled).state(),
        ForemanTurnState::Settled
    );
    let gate = WakeGate::new(turn_in(ForemanTurnState::Settled));
    assert!(gate.may_activate(), "a settled surface accepts a new wake");

    let resumed = schedule_wake(
        &store,
        work.obligation,
        work.generation,
        DeliveryRevision::new(2),
    )
    .expect("a bounded resume");
    assert!(resumed.created);
    assert_eq!(resumed.revision, DeliveryRevision::new(2));
    assert_ne!(resumed.delivery_id, work.wake.delivery_id);

    // Same obligation, one more revision, and the original untouched.
    assert_eq!(
        snapshot(&store, work.obligation).state,
        ObligationState::CompletedUnprocessed
    );
    let conn = harness.inspect();
    assert_eq!(count(&conn, "browser_deliveries"), 2);
    let original: String = conn
        .query_row(
            "SELECT state FROM browser_deliveries WHERE delivery_id = ?1",
            rusqlite::params![work.wake.delivery_id.expose_hex()],
            |row| row.get(0),
        )
        .expect("the original revision");
    assert_eq!(
        original, "accepted",
        "the accepted revision stays immutable"
    );
}

#[test]
fn gpt_005_never_overlap_an_active_or_unknown_turn() {
    for state in [
        ForemanTurnState::Starting,
        ForemanTurnState::Active,
        ForemanTurnState::ObservationLost,
    ] {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let work = accepted_work(&store, &mut artifacts, "conv-A");
        let browser =
            FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));

        // The resume timer fires while the surface is busy or unobserved.
        let gate = WakeGate::new(turn_in(state));
        assert!(!gate.may_activate(), "{state:?} must block a wake");
        assert_eq!(gate.blocked_by(), Some(state));

        let before = dump_domain(&harness.inspect());
        if gate.may_activate() {
            panic!("{state:?}: the scheduler must not reach this branch");
        }
        assert_unchanged(
            &before,
            &dump_domain(&harness.inspect()),
            &format!("GPT-005: {state:?} created no delivery activation"),
        );
        assert_eq!(count(&harness.inspect(), "browser_deliveries"), 1);
        browser.assert_untouched("GPT-005");
        assert!(snapshot(&store, work.obligation).open);
    }

    // And the two quiescent states do permit one.
    for state in [ForemanTurnState::IdleUnknown, ForemanTurnState::Settled] {
        assert!(WakeGate::new(turn_in(state)).may_activate(), "{state:?}");
    }
}

#[test]
fn gpt_006_resume_budget_exhausts_safely() {
    const BUDGET: u32 = 2;
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));

    let mut budget = ResumeBudget::new(BUDGET);
    let mut revision = DeliveryRevision::FIRST;
    while budget.take() == ResumeDecision::Schedule {
        revision = revision.next();
        let resumed = schedule_wake(&store, work.obligation, work.generation, revision)
            .expect("an automatic resume within budget");
        assert_eq!(
            deliver_wake(
                &store,
                &mut browser,
                work.obligation,
                work.generation,
                &resumed
            ),
            DeliveryState::Accepted
        );
    }
    assert_eq!(budget.used(), BUDGET);
    assert_eq!(
        browser.sends().len(),
        BUDGET as usize,
        "one physical send per automatic resume, and no more"
    );
    assert_eq!(
        count(&harness.inspect(), "browser_deliveries"),
        1 + BUDGET as i64
    );

    // The budget is spent. One attention record, and no further activation
    // however long the timer keeps firing.
    let ledger = ResumeBudget::exhausted(&HealthLedger::new(), id(1), work.obligation, NOW);
    assert!(ledger.is_open(
        HealthConditionKind::ForemanUnreachable,
        HealthScope::obligation(work.obligation)
    ));
    assert_eq!(ledger.open().count(), 1);
    // Raising it again is idempotent: one condition, not one per timer tick.
    let repeated = ResumeBudget::exhausted(&ledger, id(2), work.obligation, NOW);
    assert_eq!(repeated.open().count(), 1);

    // The durable half: the same decision, committed, and scoped to the exact
    // obligation rather than to the daemon.
    let raised = store
        .raise_foreman_unreachable(RaiseForemanUnreachableRequest {
            obligation: work.obligation,
        })
        .expect("attention on an open obligation");
    assert!(!raised.duplicate);
    assert_eq!(
        store.open_health_conditions().expect("reading conditions"),
        vec![OpenCondition {
            kind: HealthConditionKind::ForemanUnreachable,
            scope: HealthScope::obligation(work.obligation),
        }],
        "GPT-006: exactly one durable attention record"
    );

    // The timer keeps firing. Nothing is scheduled, nothing is sent, and the
    // repeated raise is a durable no-op rather than a second condition or a
    // second event.
    let sends = browser.sends().len();
    let before = dump_domain(&harness.inspect());
    for _ in 0..50 {
        assert_eq!(
            budget.take(),
            ResumeDecision::Exhausted,
            "the budget never refills"
        );
        assert!(
            store
                .raise_foreman_unreachable(RaiseForemanUnreachableRequest {
                    obligation: work.obligation,
                })
                .expect("a repeat is convergence, not a refusal")
                .duplicate
        );
    }
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "GPT-006: an exhausted budget schedules nothing",
    );
    assert_eq!(browser.sends().len(), sends, "GPT-006: zero further sends");

    // And the obligation is still owed, indefinitely, with its attention.
    assert!(snapshot(&store, work.obligation).open);
    drop(store);
    let store = harness
        .open_at(1_000 + 30 * 86_400_000, None)
        .expect("a month later");
    assert!(snapshot(&store, work.obligation).open);
    assert_eq!(
        store
            .open_health_conditions()
            .expect("reading conditions")
            .len(),
        1,
        "GPT-006: the attention record survives a restart"
    );

    // The condition's other half. A delivery that lands *is* the evidence that
    // the foreman was reachable after all, so it closes the attention that said
    // otherwise. This resume is operator-driven: the automatic budget is still
    // spent and still refuses.
    assert_eq!(budget.take(), ResumeDecision::Exhausted);
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    let resumed = schedule_wake(&store, work.obligation, work.generation, revision.next())
        .expect("an operator-driven resume");
    assert_eq!(
        deliver_wake(
            &store,
            &mut browser,
            work.obligation,
            work.generation,
            &resumed
        ),
        DeliveryState::Accepted
    );
    assert!(
        store
            .open_health_conditions()
            .expect("reading conditions")
            .is_empty(),
        "GPT-006: an accepted delivery resolves `foreman_unreachable`"
    );
    store
        .verify_projections()
        .expect("the health ledger replays from its events");
}

#[test]
fn gpt_007_bootstrap_is_low_information() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // An unrelated connector calls bootstrap. It reads the same database, with
    // no claim and no wake.
    let view = bootstrap(&harness.inspect(), NOW);

    // What it may learn.
    assert_eq!(view.protocol_version, "command-governor-foreman/v1");
    assert_eq!(
        view.connector_abi.as_deref(),
        Some("command-governor-foreman.v1")
    );
    assert_eq!(view.capability_epoch, Some(1));
    assert!(view.write_actions_available);
    assert_eq!(view.binding_generation, Some(work.generation.get()));
    assert_eq!(view.outstanding_count, 1);
    assert_eq!(view.attention.len(), 1);
    assert_eq!(view.attention[0].kind, "completed_unprocessed");
    assert_eq!(view.attention[0].count, 1);
    assert_eq!(view.attention[0].highest_priority, 10);
    assert!(view.attention[0].oldest_age_ms > 0);
    assert_eq!(view.attention[0].wake_state, "scheduled_or_accepted");
    assert_eq!(view.health.ambiguous_deliveries, 0);

    // What it must not. The whole rendered value is searched, so a field added
    // later that leaked one of these would fail here rather than in review.
    let rendered = format!("{view:#?}");
    let forbidden: Vec<(&str, String)> = vec![
        ("source host", "github.com".to_owned()),
        ("repository display", "DivMode.commandgovernor".to_owned()),
        ("repository id", "R_kgDO".to_owned()),
        ("issue ref", "issue-2".to_owned()),
        ("runtime instance", "pane-3".to_owned()),
        ("worker session", "sess-9".to_owned()),
        ("worker turn", "turn-1".to_owned()),
        ("run identity", "run-1".to_owned()),
        ("obligation identity", work.obligation.to_string()),
        ("artifact key", work.artifact.key().as_str().to_owned()),
        ("accepted message", "msg-1".to_owned()),
        ("accepted delivery id", work.wake.delivery_id.expose_hex()),
        (
            "result content",
            String::from_utf8_lossy(FINAL_RESULT).trim().to_owned(),
        ),
    ];
    for (label, value) in forbidden {
        assert!(
            !rendered.contains(&value),
            "GPT-007: bootstrap disclosed the {label}:\n{rendered}"
        );
    }
}

#[test]
fn gpt_008_unrelated_connector_cannot_claim_from_bootstrap() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // The attacker learns every bootstrap field, plus every deterministic piece
    // of delivery metadata: the obligation ID, the generation, the revision and
    // therefore the whole delivery key.
    let view = bootstrap(&harness.inspect(), NOW);
    let generation = BindingGeneration::new(view.binding_generation.expect("a bound surface"));
    let key = governor_core::delivery::DeliveryKey::derive(
        work.obligation,
        generation,
        DeliveryRevision::FIRST,
    );
    let forged = governor_core::delivery::DeliveryId::from_persisted_bytes(*key.as_bytes());
    let current = snapshot(&store, work.obligation);

    let before = dump_domain(&harness.inspect());
    let error = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: work.obligation,
            presented_delivery_id: forged,
            binding_generation: generation,
            expected_version: current.version,
            expected_source: current.source.clone(),
            lifetime: DurationMs::from_millis(60_000),
        })
        .expect_err("bootstrap knowledge is not possession of the wake");
    assert_eq!(
        error.conflict_code(),
        Some("unknown_delivery_id"),
        "and the refusal does not say whether the delivery exists"
    );
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "GPT-008: zero claim, zero state mutation",
    );
    assert_eq!(count(&harness.inspect(), "foreman_claims"), 0);
    assert_eq!(snapshot(&store, work.obligation), current);
}

#[test]
fn gpt_009_current_accepted_wake_can_claim() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");
    let current = snapshot(&store, work.obligation);

    // The exact accepted random delivery ID, generation and version create one
    // current claim.
    let minted = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("GPT-009: the exact wake claims");
    assert_eq!(count(&harness.inspect(), "foreman_claims"), 1);
    let claimed = snapshot(&store, work.obligation);
    assert_eq!(claimed.state, ObligationState::ClaimedByForeman);
    assert_eq!(claimed.claim, Some(minted.claim));
    assert!(claimed.version > current.version);

    // Repeated and parallel claim attempts are deterministic: every one of them
    // is refused, with the *same* answer every time rather than one that
    // depends on ordering. That answer is `stale_delivery_target` rather than
    // `obligation_already_claimed`, and the ordering is why: minting advanced
    // the obligation past the snapshot the accepted wake was aimed at, so the
    // wake stops being about current work one fence before exclusivity is even
    // consulted. Either refusal keeps the single-claim property; asserting the
    // exact one keeps the *determinism* claim honest.
    for round in 0..10 {
        let before = dump_domain(&harness.inspect());
        let error = mint_claim(
            &store,
            work.obligation,
            &work.wake,
            work.generation,
            LIVE_CLAIM,
        )
        .expect_err("a live claim holds the obligation exclusively");
        assert_eq!(
            error.conflict_code(),
            Some("stale_delivery_target"),
            "round {round}: the refusal must not vary"
        );
        assert_unchanged(
            &before,
            &dump_domain(&harness.inspect()),
            "GPT-009: a refused second claim changes nothing",
        );
    }
    assert_eq!(count(&harness.inspect(), "foreman_claims"), 1);

    // And a stale version, presented with the right correlation ID, is refused
    // rather than tolerated.
    let error = store
        .mint_foreman_claim(MintClaimRequest {
            obligation: work.obligation,
            presented_delivery_id: work.wake.delivery_id.clone(),
            binding_generation: work.generation,
            expected_version: ObligationVersion::FIRST,
            expected_source: claimed.source.clone(),
            lifetime: LIVE_CLAIM,
        })
        .expect_err("a stale version cannot claim");
    assert!(
        matches!(
            error.conflict_code(),
            Some("obligation_already_claimed" | "stale_delivery_target")
        ),
        "unexpected refusal: {error}"
    );
}

#[test]
fn gpt_009_an_expired_claim_yields_to_exactly_one_successor() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work: AcceptedWork = accepted_work(&store, &mut artifacts, "conv-A");
    let first = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        ALREADY_LAPSED,
    )
    .expect("the first claim");
    expire_claim(&store, work.obligation, first.claim).expect("it lapses");

    let second = mint_claim(
        &store,
        work.obligation,
        &work.wake,
        work.generation,
        LIVE_CLAIM,
    )
    .expect("exactly one successor");
    assert_ne!(second.claim, first.claim);
    assert_eq!(count(&harness.inspect(), "foreman_claims"), 2);
    assert_eq!(
        snapshot(&store, work.obligation).claim,
        Some(second.claim),
        "the obligation is held by the successor and by nothing else"
    );
}
