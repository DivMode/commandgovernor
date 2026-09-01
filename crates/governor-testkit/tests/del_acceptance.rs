//! Browser delivery acceptance tests: DEL-001 … DEL-018.
//!
//! Every one of them runs against [`FakeBrowser`], which reads the committed
//! database through its own connection and panics rather than acting when the
//! store does not already show the required state. Live Chrome conformance is
//! Gate B and nothing here stands in for it.
//!
//! # Coverage
//!
//! | Test | `docs/testing.md` | Status |
//! | --- | --- | --- |
//! | [`del_001_delivery_key_deterministic_delivery_id_random`] | DEL-001 | covered here |
//! | [`del_002_duplicate_scheduling_converges`] | DEL-002 | basics in `governor-store-sqlite` `store_lifecycle`; convergence across a restart covered here |
//! | [`del_003_claim_precedes_all_browser_io`] | DEL-003 | covered here |
//! | [`del_003_a_dead_attempt_refuses_browser_io`] | DEL-003 (negative) | covered here |
//! | [`del_004_definite_pre_submit_failure_retries_safely`] | DEL-004 | covered here |
//! | [`del_005_send_requires_the_armed_fence`] | DEL-005 | covered here |
//! | [`del_006_crash_after_claimed_becomes_ambiguous`] | DEL-006 | covered here |
//! | [`del_007_crash_around_the_activation_fence`] | DEL-007 | covered here (both physical worlds) |
//! | [`del_008_ambiguous_never_auto_resends`] | DEL-008 | covered here |
//! | [`del_009_accepted_never_auto_resends`] | DEL-009 | covered here |
//! | [`del_010_exact_bound_conversation_enforced`] | DEL-010 | covered here |
//! | [`del_011_target_reverified_immediately_before_send`] | DEL-011 | covered here |
//! | [`del_012_target_obligation_version_reverified_before_send`] | DEL-012 | covered here |
//! | [`del_013_one_revision_is_never_submitted_twice`] | DEL-013 | covered here |
//! | [`del_014_semantic_evidence_required_for_accepted`] | DEL-014 | covered here |
//! | [`del_015_only_exact_reconciliation_promotes_ambiguous`] | DEL-015 | pure half covered here and in `governor-core`; **no store operation applies `ReconciledAccepted`** — see the note |
//! | [`del_016_startup_recovery_precedes_browser_recovery`] | DEL-016 | covered here |
//! | [`del_016_a_quarantined_attempt_refuses_browser_recovery`] | DEL-016 (negative) | covered here |
//! | [`del_017_new_resume_revision_gets_new_random_correlation_id`] | DEL-017 | covered here |
//! | [`del_018_deterministic_metadata_cannot_reconstruct_delivery_id`] | DEL-018 | covered here (64 independently seeded state roots) |

use governor_core::delivery::{DELIVERY_ID_BYTES, DeliveryId, DeliveryKey, WeakBrowserSignal};
use governor_core::fence::{AttemptNo, BindingGeneration, DeliveryRevision};
use governor_core::id::ObligationId;
use governor_core::obligation::ObligationState;
use governor_core::outbound::{AmbiguityReason, DeliveryState, FailureClass};
use governor_core::time::DurationMs;
use governor_store_sqlite::{DeliveryOutcome, Store};
use governor_testkit::browser::{BrowserWorld, FakeBrowser, SendBehaviour, deliver_wake};
use governor_testkit::dump::{assert_unchanged, count, dump_domain, scalar};
use governor_testkit::harness::Harness;
use governor_testkit::scenario::{
    FINAL_RESULT, LIVE_CLAIM, accepted_work, arm_send, bind, mint_claim, open_turn, publish_result,
    record_outcome, schedule_wake, snapshot, start_worker,
};

/// The `browser_deliveries.state` label the store holds.
fn delivery_state(harness: &Harness) -> Option<String> {
    scalar(&harness.inspect(), "SELECT state FROM browser_deliveries")
}

#[test]
fn del_001_delivery_key_deterministic_delivery_id_random() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");
    publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("publication");
    let wake = schedule_wake(&store, turn.obligation, generation, DeliveryRevision::FIRST)
        .expect("scheduling");

    // Deterministic in the scheduling tuple, and only in it.
    let key = DeliveryKey::derive(turn.obligation, generation, DeliveryRevision::FIRST);
    assert_eq!(
        key,
        DeliveryKey::derive(turn.obligation, generation, DeliveryRevision::FIRST),
        "identical inputs derive an identical key"
    );
    assert_eq!(
        scalar(
            &harness.inspect(),
            "SELECT delivery_key FROM browser_deliveries"
        )
        .as_deref(),
        Some(key.to_hex().as_str()),
        "and the durable row records exactly that key"
    );
    for other in [
        DeliveryKey::derive(turn.obligation, generation, DeliveryRevision::new(2)),
        DeliveryKey::derive(
            turn.obligation,
            BindingGeneration::new(2),
            DeliveryRevision::FIRST,
        ),
        DeliveryKey::derive(
            governor_testkit::scenario::id(0xDEAD),
            generation,
            DeliveryRevision::FIRST,
        ),
    ] {
        assert_ne!(key, other, "independent logical revisions derive apart");
    }

    // At least 192 bits of entropy in the production construction.
    const { assert!(DELIVERY_ID_BYTES * 8 >= 192) };
    assert_eq!(wake.delivery_id.expose_bytes().len(), DELIVERY_ID_BYTES);

    // And not a hash of the deterministic metadata. Every deterministic value
    // the store itself derives from the same tuple is checked against it.
    let payload_digest = scalar(
        &harness.inspect(),
        "SELECT wake_payload_digest FROM browser_deliveries",
    )
    .expect("the row records a payload digest");
    for (label, candidate) in [
        ("the delivery key", key.to_hex()),
        ("the wake payload digest", payload_digest),
        ("the obligation identity", turn.obligation.to_string()),
    ] {
        assert_ne!(
            wake.delivery_id.expose_hex(),
            candidate,
            "the correlation ID must not be {label}"
        );
    }
}

#[test]
fn del_002_duplicate_scheduling_converges() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work_without_send(&store, &mut artifacts);
    let first = work.1;

    // A proven pre-submit failure, so the revision is retryable.
    record_outcome(
        &store,
        &first,
        first.attempt,
        DeliveryOutcome::Failed {
            failure: FailureClass::ComposerNotReady,
        },
    )
    .expect("a proven pre-submit failure");
    drop(store);

    // A *different process*, with its own CSPRNG stream, schedules the same
    // logical revision. It draws a fresh candidate correlation ID and throws it
    // away, because the revision already exists and owns one for its whole life.
    let store = harness.open().expect("reopen");
    let second = schedule_wake(&store, work.0, work.2, DeliveryRevision::FIRST)
        .expect("the same logical revision");
    assert!(!second.created, "the deterministic key found the same row");
    assert_eq!(
        second.delivery_id, first.delivery_id,
        "one revision keeps one previously generated correlation ID"
    );
    assert_eq!(second.attempt, AttemptNo::new(2));

    let conn = harness.inspect();
    assert_eq!(
        count(&conn, "browser_deliveries"),
        1,
        "never two physical revisions for one logical one"
    );
}

/// A published obligation with a wake scheduled but nothing sent yet.
fn accepted_work_without_send(
    store: &Store,
    artifacts: &mut governor_artifacts::ArtifactStore,
) -> (
    ObligationId,
    governor_store_sqlite::ClaimedDelivery,
    BindingGeneration,
) {
    let turn = open_turn(store);
    let generation = bind(store, "conv-A");
    start_worker(store, turn.obligation, "run-1");
    publish_result(store, artifacts, turn.obligation, "run-1", FINAL_RESULT).expect("publication");
    let wake = schedule_wake(store, turn.obligation, generation, DeliveryRevision::FIRST)
        .expect("scheduling");
    (turn.obligation, wake, generation)
}

#[test]
fn del_003_claim_precedes_all_browser_io() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);

    // The store shows `claimed` before the browser is touched at all — the fake
    // checks that itself, on every call, through its own connection.
    assert_eq!(
        scalar(&harness.inspect(), "SELECT state FROM delivery_attempts").as_deref(),
        Some("claimed")
    );
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    let state = deliver_wake(&store, &mut browser, obligation, generation, &wake);
    assert_eq!(state, DeliveryState::Accepted);
    assert_eq!(browser.sends_for(&wake.delivery_id), 1);
    assert_eq!(
        browser.calls(),
        ["navigate", "select_app", "stage_composer", "send"],
        "navigation, DOM and the activation all crossed the claim fence"
    );
}

#[test]
#[should_panic(expected = "DEL-003")]
fn del_003_a_dead_attempt_refuses_browser_io() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (_, wake, _) = accepted_work_without_send(&store, &mut artifacts);
    record_outcome(
        &store,
        &wake,
        wake.attempt,
        DeliveryOutcome::Failed {
            failure: FailureClass::ComposerNotReady,
        },
    )
    .expect("a proven pre-submit failure");

    // The attempt no longer owns the external effect, so no browser method may
    // touch it — not even navigation.
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.navigate(&wake.delivery_id, wake.attempt);
}

#[test]
fn del_004_definite_pre_submit_failure_retries_safely() {
    for (label, world) in pre_submit_worlds() {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
        let mut browser = FakeBrowser::attach(&harness.database_path(), world);

        let state = deliver_wake(&store, &mut browser, obligation, generation, &wake);
        assert_eq!(state, DeliveryState::Failed, "{label}");
        assert_eq!(browser.sends().len(), 0, "{label}: zero submitted messages");

        // A bounded retry may create the next attempt under the same revision.
        let retry = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
            .expect("a proven pre-submit failure permits a bounded retry");
        assert_eq!(retry.attempt, AttemptNo::new(2), "{label}");
        assert_eq!(retry.delivery_id, wake.delivery_id, "{label}");
        assert_eq!(
            count(&harness.inspect(), "browser_deliveries"),
            1,
            "{label}: still one physical revision"
        );

        // And the retry succeeds once the page is healthy again.
        browser.displace(BrowserWorld::healthy("conv-A"));
        let state = deliver_wake(&store, &mut browser, obligation, generation, &retry);
        assert_eq!(state, DeliveryState::Accepted, "{label}");
        assert_eq!(browser.sends().len(), 1, "{label}: exactly one submission");
    }
}

/// Every page state that proves nothing was submitted.
fn pre_submit_worlds() -> Vec<(&'static str, BrowserWorld)> {
    vec![
        (
            "target not found",
            BrowserWorld {
                target_present: false,
                ..BrowserWorld::healthy("conv-A")
            },
        ),
        (
            "wrong chat",
            BrowserWorld {
                resolved_conversation: "conv-B".to_owned(),
                ..BrowserWorld::healthy("conv-A")
            },
        ),
        (
            "app not selected",
            BrowserWorld {
                app_selected: false,
                ..BrowserWorld::healthy("conv-A")
            },
        ),
        (
            "composer not ready",
            BrowserWorld {
                composer_ready: false,
                ..BrowserWorld::healthy("conv-A")
            },
        ),
    ]
}

#[test]
#[should_panic(expected = "DEL-005")]
fn del_005_send_requires_the_armed_fence() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, _) = accepted_work_without_send(&store, &mut artifacts);

    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.navigate(&wake.delivery_id, wake.attempt);
    browser
        .stage_composer(&wake.delivery_id, wake.attempt)
        .expect("a healthy page stages");
    // The fence was never armed. Send must be unreachable.
    browser.send(&wake.delivery_id, wake.attempt, obligation);
}

#[test]
fn del_006_crash_after_claimed_becomes_ambiguous() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, _wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    assert_eq!(delivery_state(&harness).as_deref(), Some("claimed"));

    // The process dies with the attempt still owning the external effect.
    drop(store);
    let store = harness.open().expect("reopen");
    assert_eq!(store.startup().recovery.quarantined_deliveries, 1);
    assert_eq!(delivery_state(&harness).as_deref(), Some("ambiguous"));

    // Startup converted it before the caller could schedule anything, and the
    // browser was never reached.
    let browser = FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.assert_untouched("DEL-006");

    let error = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
        .expect_err("ambiguous is frozen");
    assert_eq!(error.conflict_code(), Some("delivery_revision_frozen"));
    assert_eq!(browser.sends().len(), 0);
}

#[test]
fn del_007_crash_around_the_activation_fence() {
    // Both physical worlds around the fence: one where nothing was submitted
    // and one where a message really did go out. The restart must behave
    // identically, because it cannot tell them apart — that is the point of the
    // fence.
    for (label, behaviour, expected_sends) in [
        ("zero-send world", SendBehaviour::RefuseActivation, 0),
        ("one-send world", SendBehaviour::LoseObservation, 1),
    ] {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
        let mut browser = FakeBrowser::attach(
            &harness.database_path(),
            BrowserWorld {
                send_behaviour: behaviour,
                ..BrowserWorld::healthy("conv-A")
            },
        );

        browser.navigate(&wake.delivery_id, wake.attempt);
        browser
            .stage_composer(&wake.delivery_id, wake.attempt)
            .expect("a healthy page stages");
        arm_send(&store, &wake, generation).expect("arming the Send fence");
        assert_eq!(
            scalar(&harness.inspect(), "SELECT state FROM delivery_attempts").as_deref(),
            Some("activation_armed"),
            "{label}: the fence is durable before the exact Send"
        );

        // The activation happens and then the process dies before any outcome
        // is recorded.
        let outcome = browser.send(&wake.delivery_id, wake.attempt, obligation);
        assert_eq!(browser.sends().len(), expected_sends, "{label}");
        let _ = outcome;
        drop(store);

        let store = harness.open().expect("reopen");
        assert_eq!(
            store.startup().recovery.quarantined_deliveries,
            1,
            "{label}"
        );
        assert_eq!(
            delivery_state(&harness).as_deref(),
            Some("ambiguous"),
            "{label}"
        );
        let error = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
            .expect_err("no resend in either world");
        assert_eq!(
            error.conflict_code(),
            Some("delivery_revision_frozen"),
            "{label}"
        );
        assert_eq!(
            browser.sends().len(),
            expected_sends,
            "{label}: the restart added no submission"
        );
    }
}

#[test]
fn del_008_ambiguous_never_auto_resends() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    let mut browser = FakeBrowser::attach(
        &harness.database_path(),
        BrowserWorld {
            send_behaviour: SendBehaviour::LoseObservation,
            ..BrowserWorld::healthy("conv-A")
        },
    );
    let state = deliver_wake(&store, &mut browser, obligation, generation, &wake);
    assert_eq!(state, DeliveryState::Ambiguous);
    let sends = browser.sends().len();
    drop(store);

    // Advance timers and recovery indefinitely: twenty process lifetimes, each
    // a day later than the last.
    for round in 0..20 {
        let day = 86_400_000_i64;
        let store = harness
            .open_at(1_000 + day * i64::from(round), None)
            .expect("reopen");
        let error = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
            .expect_err("ambiguous never resends");
        assert_eq!(error.conflict_code(), Some("delivery_revision_frozen"));
        assert_eq!(delivery_state(&harness).as_deref(), Some("ambiguous"));
    }
    assert_eq!(
        browser.sends().len(),
        sends,
        "DEL-008: zero additional Send calls for that revision"
    );
}

#[test]
fn del_009_accepted_never_auto_resends() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    assert_eq!(
        deliver_wake(&store, &mut browser, obligation, generation, &wake),
        DeliveryState::Accepted
    );
    assert_eq!(browser.sends().len(), 1);
    drop(store);

    // A browser crash: the whole fake is replaced.
    drop(browser);
    let browser = FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));

    // A daemon crash, an MCP outage (nothing calls resume), a long delay, and a
    // physical settlement. None of them is a licence to resend.
    for round in 0..5 {
        let store = harness
            .open_at(1_000 + 3_600_000 * i64::from(round), None)
            .expect("reopen");
        let error = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
            .expect_err("accepted is frozen");
        assert_eq!(error.conflict_code(), Some("delivery_revision_frozen"));
        assert_eq!(delivery_state(&harness).as_deref(), Some("accepted"));
        assert!(snapshot(&store, obligation).open);
    }
    assert_eq!(
        browser.sends().len(),
        0,
        "the replacement browser sent nothing"
    );
}

#[test]
fn del_010_exact_bound_conversation_enforced() {
    // Bound to `/c/A`. Every other resolution is refused before the composer is
    // mutated, and none of them submits anything.
    for resolved in ["conv-B", "root", "project-scoped-wrong", "login-redirect"] {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
        let mut browser = FakeBrowser::attach(
            &harness.database_path(),
            BrowserWorld {
                resolved_conversation: resolved.to_owned(),
                ..BrowserWorld::healthy("conv-A")
            },
        );
        let state = deliver_wake(&store, &mut browser, obligation, generation, &wake);
        assert_eq!(state, DeliveryState::Failed, "resolved {resolved}");
        assert_eq!(browser.sends().len(), 0, "resolved {resolved}");
        assert_eq!(
            scalar(
                &harness.inspect(),
                "SELECT failure_class FROM delivery_attempts"
            )
            .as_deref(),
            Some("wrong_conversation"),
            "resolved {resolved}"
        );
    }

    // A deleted chat resolves to nothing at all.
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    let mut browser = FakeBrowser::attach(
        &harness.database_path(),
        BrowserWorld {
            target_present: false,
            ..BrowserWorld::healthy("conv-A")
        },
    );
    assert_eq!(
        deliver_wake(&store, &mut browser, obligation, generation, &wake),
        DeliveryState::Failed
    );
    assert_eq!(browser.sends().len(), 0);
}

#[test]
fn del_011_target_reverified_immediately_before_send() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));

    // Correct during staging.
    browser.navigate(&wake.delivery_id, wake.attempt);
    browser
        .stage_composer(&wake.delivery_id, wake.attempt)
        .expect("the page is the bound one while staging");

    // Displaced before activation. The adapter re-checks and never sends.
    browser.displace(BrowserWorld {
        resolved_conversation: "conv-B".to_owned(),
        ..BrowserWorld::healthy("conv-A")
    });
    let failure = browser
        .stage_composer(&wake.delivery_id, wake.attempt)
        .expect_err("the re-check before activation sees the displacement");
    assert_eq!(failure, FailureClass::WrongConversation);

    // The fence was never armed, so the outcome is a proven pre-submit failure
    // and a bounded retry stays safe.
    let state = record_outcome(
        &store,
        &wake,
        wake.attempt,
        DeliveryOutcome::Failed { failure },
    )
    .expect("recording a proven pre-submit failure");
    assert_eq!(state, DeliveryState::Failed);
    assert_eq!(browser.sends().len(), 0);
    schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
        .expect("the ambiguity fence was never crossed, so a retry is admissible");

    // Had the fence been armed first, the same displacement would be ambiguous
    // rather than failed: `ComposerNotReady` no longer proves anything.
    let armed = harness_with_armed_attempt();
    let (armed_harness, armed_store, armed_wake) = armed;
    let error = record_outcome(
        &armed_store,
        &armed_wake,
        armed_wake.attempt,
        DeliveryOutcome::Failed {
            failure: FailureClass::WrongConversation,
        },
    )
    .expect_err("after arming, a displacement report proves nothing");
    assert_eq!(error.conflict_code(), Some("failure_not_proven"));
    assert_eq!(
        scalar(
            &armed_harness.inspect(),
            "SELECT state FROM delivery_attempts"
        )
        .as_deref(),
        Some("activation_armed"),
        "and the refusal changed nothing"
    );
}

/// A state root whose single attempt has crossed the ambiguity fence.
fn harness_with_armed_attempt() -> (Harness, Store, governor_store_sqlite::ClaimedDelivery) {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (_, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    arm_send(&store, &wake, generation).expect("arming");
    (harness, store, wake)
}

#[test]
fn del_012_target_obligation_version_reverified_before_send() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let turn = open_turn(&store);
    let generation = bind(&store, "conv-A");
    start_worker(&store, turn.obligation, "run-1");

    // A wake aimed at the obligation as it stands *now*.
    let wake = schedule_wake(&store, turn.obligation, generation, DeliveryRevision::FIRST)
        .expect("scheduling against the running obligation");
    let targeted = snapshot(&store, turn.obligation);

    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.navigate(&wake.delivery_id, wake.attempt);
    browser
        .stage_composer(&wake.delivery_id, wake.attempt)
        .expect("staging");

    // The obligation moves on: a confirmed result changes both its version and
    // its source fact.
    publish_result(
        &store,
        &mut artifacts,
        turn.obligation,
        "run-1",
        FINAL_RESULT,
    )
    .expect("publication");
    let moved = snapshot(&store, turn.obligation);
    assert_ne!(moved.version, targeted.version);

    // Arming re-verifies the target and refuses.
    let before = dump_domain(&harness.inspect());
    let error = arm_send(&store, &wake, generation)
        .expect_err("a wake aimed at a superseded snapshot may not submit");
    assert_eq!(error.conflict_code(), Some("stale_delivery_target"));
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "DEL-012: a stale delivery changes zero rows",
    );
    assert_eq!(browser.sends().len(), 0, "DEL-012: zero Send");
}

#[test]
fn del_013_one_revision_is_never_submitted_twice() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    let mut browser = FakeBrowser::attach(
        &harness.database_path(),
        BrowserWorld {
            composer_ready: false,
            ..BrowserWorld::healthy("conv-A")
        },
    );

    // Retry: one proven pre-submit failure, then a healthy attempt.
    assert_eq!(
        deliver_wake(&store, &mut browser, obligation, generation, &wake),
        DeliveryState::Failed
    );
    let retry = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
        .expect("a bounded retry");
    browser.displace(BrowserWorld::healthy("conv-A"));
    assert_eq!(
        deliver_wake(&store, &mut browser, obligation, generation, &retry),
        DeliveryState::Accepted
    );
    assert_eq!(browser.sends_for(&wake.delivery_id), 1);
    drop(store);

    // Restart, then reconciliation pressure: nothing may add a second physical
    // message for this revision.
    for _ in 0..5 {
        let store = harness.open().expect("reopen");
        let error = schedule_wake(&store, obligation, generation, DeliveryRevision::FIRST)
            .expect_err("accepted is frozen");
        assert_eq!(error.conflict_code(), Some("delivery_revision_frozen"));
        let error = record_outcome(
            &store,
            &retry,
            retry.attempt,
            DeliveryOutcome::Ambiguous {
                reason: AmbiguityReason::ObservationLost,
            },
        )
        .expect_err("an accepted attempt is terminal");
        assert_eq!(error.conflict_code(), Some("illegal_delivery_transition"));
    }
    assert_eq!(
        browser.sends_for(&wake.delivery_id),
        1,
        "DEL-013: at most one physical submitted message per revision"
    );
}

#[test]
fn del_014_semantic_evidence_required_for_accepted() {
    for signal in [
        WeakBrowserSignal::ComposerEmptied,
        WeakBrowserSignal::StopButtonAppeared,
        WeakBrowserSignal::UrlChanged,
        WeakBrowserSignal::AssistantStarted,
        WeakBrowserSignal::WakeTextInDom,
    ] {
        let harness = Harness::new();
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
        let mut browser = FakeBrowser::attach(
            &harness.database_path(),
            BrowserWorld {
                send_behaviour: SendBehaviour::WeakSignalOnly(signal),
                ..BrowserWorld::healthy("conv-A")
            },
        );

        let state = deliver_wake(&store, &mut browser, obligation, generation, &wake);
        assert_eq!(
            state,
            DeliveryState::Ambiguous,
            "{signal:?} can never produce accepted"
        );
        assert_eq!(
            scalar(
                &harness.inspect(),
                "SELECT accepted_message_ref FROM browser_deliveries"
            ),
            None,
            "{signal:?}: no acceptance evidence was recorded"
        );

        // And an accepted wake is a prerequisite for a claim, so a weak signal
        // cannot open the MCP path either.
        let error = mint_claim(&store, obligation, &wake, generation, LIVE_CLAIM)
            .expect_err("an ambiguous wake mints no claim");
        assert_eq!(error.conflict_code(), Some("unknown_delivery_id"));
    }
}

#[test]
fn del_015_only_exact_reconciliation_promotes_ambiguous() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (obligation, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    let mut browser = FakeBrowser::attach(
        &harness.database_path(),
        BrowserWorld {
            send_behaviour: SendBehaviour::LoseObservation,
            ..BrowserWorld::healthy("conv-A")
        },
    );
    assert_eq!(
        deliver_wake(&store, &mut browser, obligation, generation, &wake),
        DeliveryState::Ambiguous
    );

    // Phase 1 has **no store operation** that applies `ReconciledAccepted`, so
    // the promotion half cannot be driven durably here. What is provable is the
    // half that matters for safety: nothing in the store's surface promotes an
    // ambiguous revision, and trying changes nothing.
    let before = dump_domain(&harness.inspect());
    let error = record_outcome(
        &store,
        &wake,
        wake.attempt,
        DeliveryOutcome::Accepted {
            message: governor_core::foreman_turn::ProviderMessageRef::new(
                governor_testkit::scenario::token("msg-late"),
            ),
        },
    )
    .expect_err("an ambiguous attempt is terminal to the outcome operation");
    assert_eq!(error.conflict_code(), Some("illegal_delivery_transition"));
    assert_unchanged(
        &before,
        &dump_domain(&harness.inspect()),
        "DEL-015: a late acceptance report changes nothing",
    );
    assert_eq!(delivery_state(&harness).as_deref(), Some("ambiguous"));
    assert_eq!(
        browser.sends().len(),
        1,
        "and produces no second submission"
    );
}

#[test]
fn del_016_startup_recovery_precedes_browser_recovery() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (_, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    arm_send(&store, &wake, generation).expect("arming");
    drop(store);

    // A live browser target exists, and startup runs.
    let store = harness.open().expect("reopen");

    // By the time the caller holds a `Store`, the orphan is already frozen:
    // `OpenStore::start` quarantines before it hands anything back, so there is
    // no window in which a browser supervisor could have seen a live attempt.
    assert_eq!(store.startup().recovery.quarantined_deliveries, 1);
    assert_eq!(
        scalar(&harness.inspect(), "SELECT state FROM delivery_attempts").as_deref(),
        Some("ambiguous"),
        "DEL-016: conversion happened before the store was returned"
    );
    let browser = FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.assert_untouched("DEL-016");
}

#[test]
#[should_panic(expected = "DEL-003")]
fn del_016_a_quarantined_attempt_refuses_browser_recovery() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let (_, wake, generation) = accepted_work_without_send(&store, &mut artifacts);
    arm_send(&store, &wake, generation).expect("arming");
    drop(store);
    let _store = harness.open().expect("reopen quarantines the orphan");

    // Browser recovery tries to resume the attempt it left behind.
    let mut browser =
        FakeBrowser::attach(&harness.database_path(), BrowserWorld::healthy("conv-A"));
    browser.navigate(&wake.delivery_id, wake.attempt);
}

#[test]
fn del_017_new_resume_revision_gets_new_random_correlation_id() {
    let harness = Harness::new();
    let store = harness.open().expect("opening");
    let mut artifacts = harness.open_artifacts();
    let work = accepted_work(&store, &mut artifacts, "conv-A");

    // Accepted, settled, unACKed, and past the policy delay: eligible for a
    // bounded resume, which is a *new revision* rather than a replay.
    let second = schedule_wake(
        &store,
        work.obligation,
        work.generation,
        DeliveryRevision::new(2),
    )
    .expect("a bounded resume creates the next revision");
    assert!(second.created);
    assert_ne!(
        second.delivery_id, work.wake.delivery_id,
        "DEL-017: an independent random correlation ID"
    );
    assert_ne!(
        DeliveryKey::derive(work.obligation, work.generation, DeliveryRevision::new(2)),
        DeliveryKey::derive(work.obligation, work.generation, DeliveryRevision::FIRST),
        "DEL-017: a new deterministic key"
    );

    // The old accepted revision is untouched.
    let conn = harness.inspect();
    assert_eq!(count(&conn, "browser_deliveries"), 2);
    let old: String = conn
        .query_row(
            "SELECT state FROM browser_deliveries WHERE delivery_id = ?1",
            rusqlite::params![work.wake.delivery_id.expose_hex()],
            |row| row.get(0),
        )
        .expect("the first revision is still there");
    assert_eq!(old, "accepted", "DEL-017: the old revision stays immutable");
}

#[test]
fn del_018_deterministic_metadata_cannot_reconstruct_delivery_id() {
    // The attacker is given every deterministic input and every value derived
    // from them, across many independently seeded state roots. None of them is
    // ever the correlation ID, and none of them can claim.
    for seed in 0..64u64 {
        let harness = Harness::with_seed(seed.wrapping_mul(7).wrapping_add(1));
        let store = harness.open().expect("opening");
        let mut artifacts = harness.open_artifacts();
        let work = accepted_work(&store, &mut artifacts, "conv-A");

        let key = DeliveryKey::derive(work.obligation, work.generation, DeliveryRevision::FIRST);
        let payload_digest = scalar(
            &harness.inspect(),
            "SELECT wake_payload_digest FROM browser_deliveries",
        )
        .expect("a payload digest");

        let forgeries = [
            (
                "the delivery key",
                DeliveryId::from_persisted_bytes(*key.as_bytes()),
            ),
            (
                "the wake payload digest",
                DeliveryId::parse_persisted(&payload_digest).expect("a 32-byte digest"),
            ),
            (
                "the obligation identity, padded",
                DeliveryId::from_persisted_bytes(padded(work.obligation.as_uuid().as_bytes())),
            ),
        ];
        for (label, forged) in forgeries {
            assert_ne!(
                forged.expose_hex(),
                work.wake.delivery_id.expose_hex(),
                "seed {seed}: {label} is the correlation ID"
            );
            let before = dump_domain(&harness.inspect());
            let error = store
                .mint_foreman_claim(governor_store_sqlite::MintClaimRequest {
                    obligation: work.obligation,
                    presented_delivery_id: forged,
                    binding_generation: work.generation,
                    expected_version: snapshot(&store, work.obligation).version,
                    expected_source: snapshot(&store, work.obligation).source,
                    lifetime: DurationMs::from_millis(60_000),
                })
                .expect_err("deterministic metadata cannot claim");
            assert_eq!(error.conflict_code(), Some("unknown_delivery_id"));
            assert_unchanged(
                &before,
                &dump_domain(&harness.inspect()),
                &format!("seed {seed}: a forged resume with {label} mutates nothing"),
            );
        }

        // The real one does claim, so the refusals above were about possession.
        let claimed = mint_claim(
            &store,
            work.obligation,
            &work.wake,
            work.generation,
            LIVE_CLAIM,
        )
        .expect("the exact accepted correlation ID claims");
        assert_eq!(
            snapshot(&store, work.obligation).state,
            ObligationState::ClaimedByForeman
        );
        assert_eq!(snapshot(&store, work.obligation).claim, Some(claimed.claim));
    }
}

/// Widens a 16-byte identity to the correlation ID's width.
fn padded(bytes: &[u8; 16]) -> [u8; DELIVERY_ID_BYTES] {
    let mut out = [0u8; DELIVERY_ID_BYTES];
    out[..16].copy_from_slice(bytes);
    out[16..].copy_from_slice(bytes);
    out
}
