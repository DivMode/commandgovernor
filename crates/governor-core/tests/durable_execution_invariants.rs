//! The durable-execution acceptance items of
//! `docs/research/2026-08-31-durable-orchestration-pattern-review.md`, driven
//! entirely through `governor-core`'s public API.
//!
//! Like `state_machine_invariants.rs`, this suite lives *outside* the crate, so
//! it may only use what a real caller can use. That is what makes the
//! type-level claims here checkable: if an execution permit could be forged, a
//! receipt ACK converted into a disposition, or a lease token recomputed from
//! metadata, it would have to be possible from this file.
//!
//! Items 2, 3 (kill-window half), 11 and 12 of the review's acceptance list are
//! **not** here and are not faked. They need a real crash boundary, a real
//! SQLite transaction, or a durable-schema scan:
//!
//! | Item | Pure half proven here | Durable half owed by |
//! | --- | --- | --- |
//! | 1. intent before I/O | [`no_execute_permit_exists_before_a_durable_intent_is_accepted`] | store: commit before `accept_committed` |
//! | 2. kill after intent, before I/O | — | store + testkit failpoints |
//! | 3. kill after I/O, before outcome | `an_ambiguous_attempt_never_decides_to_execute` | store + testkit failpoints |
//! | 4. completed mutation retry | [`a_completed_mutation_retry_returns_the_recorded_result`] | store journal transaction |
//! | 5. pending mutation retry | [`a_pending_mutation_retry_is_uncertain_and_never_dispatches`] | store journal transaction |
//! | 6. different command id | [`a_different_command_identity_is_a_new_operation`] | store `PRIMARY KEY(actor_id, command_id)` |
//! | 7. lease PID reuse | [`a_reused_process_slot_cannot_own_or_release_the_old_lease`] | daemon process-start probe |
//! | 8. stale token/epoch | [`stale_lease_fences_cannot_mutate_current_ownership`] | store lease row |
//! | 9. receipt ACK is not a closure | [`a_receipt_ack_cannot_close_a_result_obligation`] | — |
//! | 10. semantic ACK separation | [`every_transport_ack_may_land_while_the_obligation_stays_open`] | — |
//! | 11. projection replay equivalence | — | store + testkit |
//! | 12. forbidden-data scan | [`the_new_durable_types_have_nowhere_to_put_forbidden_content`] (shape half) | store schema scan |

use governor_core::binding::{
    BindingEvent, BindingLedger, BrowserProfileRef, ConnectorAbi, ConversationRef,
    VerifiedBindingTarget, WriteCapabilityState,
};
use governor_core::claim::{AckOutcome, ForemanClaim, ResumeRequest, acknowledge, mint_claim};
use governor_core::delivery::{AcceptedWakeEvidence, BrowserWake, WakeTarget};
use governor_core::effect::{
    DestinationRef, EffectAmbiguityReason, EffectDecision, ExternalAttempt, ExternalAttemptEvent,
    ExternalAttemptState, ExternalEffectClass, IdempotencyContract, IdempotencyKey, NoEffectClass,
    RetryAdmissibility,
};
use governor_core::fence::{
    AttemptNo, BindingGeneration, DaemonEpoch, IncarnationGeneration, SafeToken, SourceRef,
};
use governor_core::foreman_turn::ProviderMessageRef;
use governor_core::id::{
    ActorId, ClaimId, ExternalAttemptId, ForemanBindingId, MutationCommandId, ObligationId,
    ResourceLeaseId, ResultArtifactId, TaskId, TurnId,
};
use governor_core::lease::{
    IncarnationMismatch, LeaseHolderProof, LeaseRequest, LeaseState, LeaseToken,
    ProcessIncarnation, ProcessSlot, ProcessStartRef, ResourceIdentity, ResourceNamespace,
    ResourceOwnership,
};
use governor_core::mutation::{
    CompactionEligibility, MutationCommand, MutationCommandEvent, MutationCommandKind,
    MutationDisposition, MutationFingerprint, MutationJournal, ReceiptAck, SafeMutationResult,
};
use governor_core::obligation::{
    AckRequest, Disposition, Obligation, ObligationEvent, ObligationKind, ObligationState,
};
use governor_core::outbound::DeliveryEvent;
use governor_core::random::SecureRandom;
use governor_core::time::{DurationMs, Timestamp};
use governor_core::worker_evidence::{
    ChildExitReceipt, ChildExitStatus, ConfirmedFinalResult, FinalResultReceipt,
    ManagedRunEvidence, ManagedRunOutcome, WorkerOutcome,
};
use uuid::Uuid;

// ---------------------------------------------------------------- fixtures --

/// A deterministic byte stream standing in for the daemon's CSPRNG.
struct StreamRng {
    next: u8,
}

impl StreamRng {
    const fn seeded(seed: u8) -> Self {
        Self { next: seed }
    }
}

impl SecureRandom for StreamRng {
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for slot in dest.iter_mut() {
            *slot = self.next;
            self.next = self.next.wrapping_add(1);
        }
    }
}

fn token(value: &str) -> SafeToken {
    SafeToken::new(value).expect("fixture tokens are safe by construction")
}

fn source(namespace: &str, event: &str, fence: &str) -> SourceRef {
    SourceRef::new(token(namespace), token(event), token(fence))
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(ms)
}

fn actor(n: u128) -> ActorId {
    ActorId::from_uuid(Uuid::from_u128(n))
}

fn command_id(n: u128) -> MutationCommandId {
    MutationCommandId::from_uuid(Uuid::from_u128(n))
}

fn attempt_id(n: u128) -> ExternalAttemptId {
    ExternalAttemptId::from_uuid(Uuid::from_u128(n))
}

fn lease_id(n: u128) -> ResourceLeaseId {
    ResourceLeaseId::from_uuid(Uuid::from_u128(n))
}

// ------------------------------------------------- external-effect fixtures --

fn destination(endpoint: &str) -> DestinationRef {
    DestinationRef::new(token("worker-host"), token(endpoint), token("gen-1"))
}

fn effect_source() -> SourceRef {
    source("cg.internal", "cmd-1", "rev-1")
}

fn idempotent(key: &str) -> ExternalEffectClass {
    ExternalEffectClass::IdempotentWrite {
        contract: IdempotencyContract::DeduplicatedByKey {
            window: DurationMs::from_millis(60_000),
        },
        key: IdempotencyKey::new(token(key)),
    }
}

/// An attempt whose intent is recorded but whose acceptance has been dropped —
/// the state a caller is in when it has *not* proven the intent is durable.
fn intent_only(class: ExternalEffectClass) -> ExternalAttempt<u8> {
    let (attempt, _dropped) = ExternalAttempt::<u8>::record_intent(
        attempt_id(1),
        class,
        destination("turn-7"),
        effect_source(),
        DaemonEpoch::FIRST,
        at(1),
    )
    .accept_committed();
    attempt
}

fn advance_attempt(
    attempt: &ExternalAttempt<u8>,
    event: &ExternalAttemptEvent<u8>,
) -> ExternalAttempt<u8> {
    attempt
        .apply(event)
        .expect("fixture attempt transitions are legal")
        .advanced()
        .expect("fixture attempt transitions advance")
}

// ------------------------------------------------------- mutation fixtures --

fn command_kind() -> MutationCommandKind {
    MutationCommandKind::new(token("worker.resume"))
}

fn fingerprint(parameter: &str) -> MutationFingerprint {
    let parameter = token(parameter);
    MutationFingerprint::derive(&command_kind(), &[&parameter])
}

fn received_command() -> MutationCommand {
    MutationCommand::received(
        actor(1),
        command_id(9),
        command_kind(),
        fingerprint("turn-7"),
        DaemonEpoch::FIRST,
        at(30),
    )
}

fn completed_command() -> MutationCommand {
    received_command()
        .apply(&MutationCommandEvent::ResultCommitted {
            result: SafeMutationResult::Applied {
                reference: Some(token("turn-7")),
            },
            at: at(31),
        })
        .expect("committing a result is legal from received")
        .advanced()
        .expect("committing advances")
}

// ---------------------------------------------------------- lease fixtures --

fn resource() -> ResourceIdentity {
    ResourceIdentity::canonical(
        ResourceNamespace::new(token("session")),
        "/Volumes/Data/state/session-a",
    )
}

fn incarnation(slot: u32, start: &str) -> ProcessIncarnation {
    ProcessIncarnation::new(ProcessSlot::new(slot), ProcessStartRef::new(token(start)))
}

fn holder_proof(
    token_bytes: &LeaseToken,
    slot: u32,
    start: &str,
    epoch: DaemonEpoch,
) -> LeaseHolderProof {
    LeaseHolderProof {
        token: token_bytes.clone(),
        incarnation: incarnation(slot, start),
        daemon_epoch: epoch,
    }
}

/// A resource held by process 4242/`start-a` under daemon epoch two.
fn held_resource() -> (ResourceOwnership, LeaseToken) {
    let mut rng = StreamRng::seeded(1);
    let granted = ResourceOwnership::unowned(resource())
        .acquire(
            &LeaseRequest {
                holder: actor(1),
                incarnation: incarnation(4242, "start-a"),
                daemon_epoch: DaemonEpoch::new(2),
                ttl: DurationMs::from_millis(30_000),
            },
            lease_id(1),
            &mut rng,
            at(100),
        )
        .expect("an unowned resource is acquirable");
    (granted.ownership, granted.token)
}

// ----------------------------------------------------- obligation fixtures --

fn obligation_id() -> ObligationId {
    ObligationId::from_uuid(Uuid::from_u128(0x0b11))
}

fn claim_id() -> ClaimId {
    ClaimId::from_uuid(Uuid::from_u128(100))
}

fn confirmed_completion() -> ConfirmedFinalResult {
    let evidence = ManagedRunEvidence::new()
        .with_final_result(FinalResultReceipt {
            run_ref: token("run-1"),
            complete: true,
            outcome: ManagedRunOutcome::Success,
        })
        .with_child_exit(ChildExitReceipt {
            run_ref: token("run-1"),
            status: ChildExitStatus::Success,
        });
    match evidence.classify() {
        WorkerOutcome::ConfirmedCompletion(proof) => proof,
        other => panic!("expected confirmed completion, got {other:?}"),
    }
}

fn advance_obligation(obligation: &Obligation, event: &ObligationEvent) -> Obligation {
    obligation
        .apply(event)
        .expect("fixture obligation transitions are legal")
        .advanced()
        .expect("fixture obligation transitions advance")
}

/// An obligation in `completed_unprocessed` with a durable artifact.
fn completed_obligation() -> Obligation {
    let created = Obligation::created(
        obligation_id(),
        TaskId::from_uuid(Uuid::from_u128(1)),
        Some(TurnId::from_uuid(Uuid::from_u128(2))),
        ObligationKind::WorkerTurn,
        source("cg.internal", "obl-1", "created"),
        IncarnationGeneration::FIRST,
    );
    let running = advance_obligation(
        &created,
        &ObligationEvent::WorkerStarted {
            source: source("claude.init", "run-1", "start"),
            incarnation: IncarnationGeneration::FIRST,
            at: at(1),
        },
    );
    advance_obligation(
        &running,
        &ObligationEvent::ResultPublished {
            source: source("claude.result", "run-1", "final"),
            incarnation: IncarnationGeneration::FIRST,
            proof: confirmed_completion(),
            artifact: ResultArtifactId::from_uuid(Uuid::from_u128(0xa471)),
            at: at(2),
        },
    )
}

fn bound_ledger() -> BindingLedger {
    BindingLedger::unbound()
        .apply(&BindingEvent::Bound {
            target: Box::new(VerifiedBindingTarget {
                id: ForemanBindingId::from_uuid(Uuid::from_u128(1)),
                conversation: ConversationRef::new(token("conv-A")),
                profile: BrowserProfileRef::new(token("cg-profile")),
                connector_abi: ConnectorAbi::new(token("command-governor-foreman.v1")),
                capability_epoch: 1,
                write_capability: WriteCapabilityState::Proven,
            }),
            at: at(0),
        })
        .expect("first bind is legal")
        .advanced()
        .expect("bind advances")
}

/// An accepted browser wake — ACK layer 2's transport half.
fn accepted_wake(rng: &mut StreamRng, obligation: &Obligation) -> BrowserWake {
    let wake = BrowserWake::create(
        rng,
        WakeTarget::snapshot(obligation),
        ForemanBindingId::from_uuid(Uuid::from_u128(1)),
        BindingGeneration::FIRST,
        3,
    );
    let claimed = wake
        .apply(&DeliveryEvent::AttemptClaimed { at: at(10) })
        .expect("claim is legal")
        .advanced()
        .expect("claim advances");
    let armed = claimed
        .apply(&DeliveryEvent::ActivationArmed {
            attempt: AttemptNo::FIRST,
            at: at(11),
        })
        .expect("arming is legal")
        .advanced()
        .expect("arming advances");
    armed
        .apply(&DeliveryEvent::AttemptAccepted {
            attempt: AttemptNo::FIRST,
            evidence: AcceptedWakeEvidence::new(
                ConversationRef::new(token("conv-A")),
                ProviderMessageRef::new(token("msg-1")),
            ),
            at: at(12),
        })
        .expect("acceptance is legal from armed")
        .advanced()
        .expect("acceptance advances")
}

// ------------------------------------------------------------- item 1 and 3 --

/// Review acceptance item 1, pure half: an adapter cannot be handed a permit
/// until the store has accepted a durable intent.
#[test]
fn no_execute_permit_exists_before_a_durable_intent_is_accepted() {
    let recorded = ExternalAttempt::<u8>::record_intent(
        attempt_id(1),
        ExternalEffectClass::NonIdempotentWrite,
        destination("turn-7"),
        effect_source(),
        DaemonEpoch::FIRST,
        at(1),
    );

    // Before `accept_committed`, the projection exists and refuses to execute.
    let err = recorded
        .attempt()
        .decide(None, |value: &u8| *value)
        .expect_err("an unaccepted intent authorises nothing");
    assert_eq!(err.code(), "execute_requires_durable_intent");
    assert_eq!(
        recorded.attempt().state(),
        ExternalAttemptState::IntentRecorded
    );
    assert!(!recorded.attempt().dispatched(), "zero I/O has happened");

    // The acceptance is the *only* way through, and it is consumed by value.
    let (attempt, acceptance) = recorded.accept_committed();
    assert_eq!(acceptance.attempt(), attempt.id());
    let decision = attempt
        .decide(Some(acceptance), |value: &u8| *value)
        .expect("a committed intent may execute");
    let permit = decision.permit().expect("execute carries a permit");
    assert_eq!(permit.attempt(), attempt.id());
    assert_eq!(permit.destination(), &destination("turn-7"));
    assert_eq!(permit.source(), &effect_source());
    assert_eq!(permit.daemon_epoch(), DaemonEpoch::FIRST);

    // A second decision on the same projection has no acceptance left to
    // present, so it refuses again. `ExternalExecutionPermit` is neither Clone
    // nor Copy and has no public constructor, so the permit above cannot be
    // duplicated either — that is a compile-time property, not a runtime one.
    let err = attempt
        .decide(None, |value: &u8| *value)
        .expect_err("one durable intent, one permit");
    assert_eq!(err.code(), "execute_requires_durable_intent");
}

/// Review acceptance item 3, pure half: an attempt whose fate was lost never
/// decides to execute, on replay or otherwise.
#[test]
fn an_ambiguous_attempt_never_decides_to_execute() {
    // Table: every class, both crash windows, one answer.
    let windows: [(&str, bool, EffectAmbiguityReason); 2] = [
        (
            "killed after intent, before dispatch",
            false,
            EffectAmbiguityReason::OrphanedByRestart,
        ),
        (
            "killed after dispatch, before outcome",
            true,
            EffectAmbiguityReason::OrphanedByRestart,
        ),
    ];
    let classes = [
        ExternalEffectClass::Read,
        idempotent("k-1"),
        ExternalEffectClass::NonIdempotentWrite,
    ];

    for (label, dispatched, reason) in windows {
        for class in &classes {
            let mut attempt = intent_only(class.clone());
            if dispatched {
                attempt = advance_attempt(
                    &attempt,
                    &ExternalAttemptEvent::CallDispatched { at: at(2) },
                );
            }
            let ambiguous = advance_attempt(
                &attempt,
                &ExternalAttemptEvent::OutcomeUnknown { reason, at: at(3) },
            );
            assert_eq!(
                ambiguous.state(),
                ExternalAttemptState::Ambiguous,
                "{label}: an unknown fate is never success"
            );

            let decision = ambiguous
                .decide(None, |value: &u8| *value)
                .expect("an ambiguous attempt always decides");
            assert!(
                !decision.is_execute(),
                "{label}: ambiguity must not authorise I/O"
            );
            let EffectDecision::Reconcile(required) = decision else {
                panic!("{label}: expected reconciliation for {class:?}");
            };
            assert_eq!(required.attempt(), ambiguous.id());
            assert_eq!(required.class(), class, "the exact key travels with it");
            assert_eq!(required.destination(), &destination("turn-7"));
            assert_eq!(required.reason(), reason);
        }
    }
}

/// The same rule expressed as retry policy: only a recorded contract makes a
/// second attempt admissible, and there is no "probably safe" class to abuse.
#[test]
fn a_retry_after_ambiguity_needs_the_recorded_contract_and_exact_key() {
    let ambiguous = |class: ExternalEffectClass| {
        let dispatched = advance_attempt(
            &intent_only(class),
            &ExternalAttemptEvent::CallDispatched { at: at(2) },
        );
        advance_attempt(
            &dispatched,
            &ExternalAttemptEvent::OutcomeUnknown {
                reason: EffectAmbiguityReason::ResponseLost,
                at: at(3),
            },
        )
    };

    let non_idempotent = ambiguous(ExternalEffectClass::NonIdempotentWrite);
    assert_eq!(
        non_idempotent.retry_admissibility(),
        RetryAdmissibility::RequiresReconciliation
    );
    assert_eq!(
        non_idempotent
            .admit_retry(
                &destination("turn-7"),
                &ExternalEffectClass::NonIdempotentWrite
            )
            .unwrap_err()
            .code(),
        "retry_requires_idempotency_contract"
    );

    let idempotent_attempt = ambiguous(idempotent("k-1"));
    idempotent_attempt
        .admit_retry(&destination("turn-7"), &idempotent("k-1"))
        .expect("the exact recorded contract and key are reproduced");

    // Table of near-misses. Each is refused with the same code, so no caller
    // can talk its way into a repeat by varying one field.
    let near_misses: [(&str, DestinationRef, ExternalEffectClass); 4] = [
        ("different key", destination("turn-7"), idempotent("k-2")),
        (
            "different contract",
            destination("turn-7"),
            ExternalEffectClass::IdempotentWrite {
                contract: IdempotencyContract::ConditionalOnDestinationFence,
                key: IdempotencyKey::new(token("k-1")),
            },
        ),
        (
            "class downgraded to a bare write",
            destination("turn-7"),
            ExternalEffectClass::NonIdempotentWrite,
        ),
        (
            "same key, different destination",
            destination("turn-9"),
            idempotent("k-1"),
        ),
    ];
    for (label, target, class) in near_misses {
        let err = idempotent_attempt
            .admit_retry(&target, &class)
            .expect_err(label);
        assert_eq!(
            err.code(),
            "retry_requires_idempotency_contract",
            "{label} must be refused with the contract code"
        );
    }
}

// ------------------------------------------------------------ items 4 to 6 --

/// Review acceptance item 4: the same identity returns the recorded result,
/// with no adapter invocation and no new permit anywhere in reach.
#[test]
fn a_completed_mutation_retry_returns_the_recorded_result() {
    let mut journal = MutationJournal::new();
    journal.record(completed_command());

    let disposition = journal
        .resolve(actor(1), command_id(9), fingerprint("turn-7"))
        .expect("an exact retry of a completed identity resolves");
    assert_eq!(
        disposition,
        MutationDisposition::RecordedResult(SafeMutationResult::Applied {
            reference: Some(token("turn-7")),
        })
    );

    // `MutationDisposition` has two variants and neither is an execution
    // permit: replaying a mutation result cannot reach consequential I/O.
    match disposition {
        MutationDisposition::RecordedResult(_) | MutationDisposition::NewOperation => {}
    }
}

/// Review acceptance item 5: the same identity is uncertain and is never
/// redispatched — `resolve` cannot express a dispatch decision at all.
#[test]
fn a_pending_mutation_retry_is_uncertain_and_never_dispatches() {
    let uncertain_after_recovery = received_command()
        .apply(&MutationCommandEvent::MarkedUncertain { at: at(50) })
        .expect("startup recovery may record the uncertainty")
        .advanced()
        .expect("marking advances");

    for row in [received_command(), uncertain_after_recovery] {
        let mut journal = MutationJournal::new();
        let status = row.status();
        journal.record(row);
        let err = journal
            .resolve(actor(1), command_id(9), fingerprint("turn-7"))
            .expect_err("a pending identity has no result to return");
        assert_eq!(
            err.code(),
            "mutation_result_uncertain",
            "status {status:?} must surface uncertainty"
        );
    }
}

/// Review acceptance item 6: a different identity is genuinely new work and
/// must pass normal policy rather than being deduplicated by accident.
#[test]
fn a_different_command_identity_is_a_new_operation() {
    let mut journal = MutationJournal::new();
    journal.record(completed_command());

    let cases: [(&str, ActorId, MutationCommandId, MutationFingerprint); 3] = [
        (
            "different command id",
            actor(1),
            command_id(10),
            fingerprint("turn-7"),
        ),
        (
            "same command id, different actor",
            actor(2),
            command_id(9),
            fingerprint("turn-7"),
        ),
        (
            "unseen identity entirely",
            actor(3),
            command_id(11),
            fingerprint("turn-8"),
        ),
    ];
    for (label, who, which, print) in cases {
        assert_eq!(
            journal.resolve(who, which, print).expect("resolves"),
            MutationDisposition::NewOperation,
            "{label} must not be deduplicated"
        );
    }

    // The converse: reusing one identity for a *different* operation is not an
    // exact retry, so the recorded result is withheld rather than misapplied.
    assert_eq!(
        journal
            .resolve(actor(1), command_id(9), fingerprint("turn-8"))
            .unwrap_err()
            .code(),
        "mutation_command_mismatch"
    );
}

// ------------------------------------------------------------ items 7 and 8 --

/// Review acceptance item 7: a recycled process number with a different start
/// identity is a different incarnation and owns nothing.
#[test]
fn a_reused_process_slot_cannot_own_or_release_the_old_lease() {
    let (ownership, token_bytes) = held_resource();
    // The impostor even holds the right token — it read the persisted lease
    // row. The process-start identity is what stops it.
    let impostor = holder_proof(&token_bytes, 4242, "start-b", DaemonEpoch::new(2));

    let renew = ownership
        .renew(&impostor, DurationMs::from_millis(30_000), at(200))
        .expect_err("a recycled process number is a different incarnation");
    let release = ownership
        .release(&impostor, at(200))
        .expect_err("and it cannot release either");
    for err in [renew, release] {
        assert_eq!(err.code(), "stale_process_incarnation");
    }
    assert!(
        ownership.is_held_at(at(200)),
        "zero mutation: the real holder still owns the resource"
    );
    assert_eq!(
        ownership
            .current()
            .expect("the lease is still there")
            .incarnation(),
        &incarnation(4242, "start-a")
    );
    assert_eq!(
        incarnation(4242, "start-a").classify(&incarnation(4242, "start-b")),
        Some(IncarnationMismatch::SlotReused)
    );
}

/// Review acceptance item 8: a stale token or a superseded daemon epoch cannot
/// mutate or release current ownership.
#[test]
fn stale_lease_fences_cannot_mutate_current_ownership() {
    let (ownership, token_bytes) = held_resource();
    let forged = LeaseToken::from_persisted_bytes([0xAB; 32]);

    let cases: [(&str, LeaseHolderProof, &str); 3] = [
        (
            "a token the holder never had",
            holder_proof(&forged, 4242, "start-a", DaemonEpoch::new(2)),
            "stale_lease_token",
        ),
        (
            "an unrelated process with the right token",
            holder_proof(&token_bytes, 5151, "start-c", DaemonEpoch::new(2)),
            "stale_process_incarnation",
        ),
        (
            "the right holder from a superseded daemon lifetime",
            holder_proof(&token_bytes, 4242, "start-a", DaemonEpoch::FIRST),
            "stale_daemon_epoch",
        ),
    ];

    for (label, proof, expected) in cases {
        let renew = ownership
            .renew(&proof, DurationMs::from_millis(1_000), at(200))
            .unwrap_err();
        let release = ownership.release(&proof, at(200)).unwrap_err();
        assert_eq!(renew.code(), expected, "{label} must not renew");
        assert_eq!(release.code(), expected, "{label} must not release");
        assert_eq!(
            ownership.current().expect("still leased").state(),
            LeaseState::Held,
            "{label} changed nothing"
        );
    }

    // And a superseded daemon cannot take the resource even once it expires.
    let mut rng = StreamRng::seeded(90);
    let err = ownership
        .acquire(
            &LeaseRequest {
                holder: actor(2),
                incarnation: incarnation(6000, "start-d"),
                daemon_epoch: DaemonEpoch::FIRST,
                ttl: DurationMs::from_millis(1_000),
            },
            lease_id(2),
            &mut rng,
            at(999_999),
        )
        .expect_err("an older daemon lifetime never wins the resource");
    assert_eq!(err.code(), "stale_daemon_epoch");

    // The exact holder, with every fence intact, still works.
    let holder = holder_proof(&token_bytes, 4242, "start-a", DaemonEpoch::new(2));
    ownership
        .renew(&holder, DurationMs::from_millis(1_000), at(200))
        .expect("the real holder is unaffected by any of the above");
}

/// A lease token is drawn from the CSPRNG port and from nothing else, so it is
/// not a function of resource, process, epoch, or holder.
#[test]
fn a_lease_token_cannot_be_derived_from_lease_metadata() {
    let mut rng = StreamRng::seeded(0);
    let a = LeaseToken::generate(&mut rng);
    let b = LeaseToken::generate(&mut rng);
    assert_ne!(a, b);
    assert_eq!(format!("{a:?}"), "LeaseToken(<redacted>)");

    // Two acquisitions of the *same* resource, by the *same* process, under the
    // *same* epoch, produce different tokens.
    let mut first_rng = StreamRng::seeded(1);
    let mut second_rng = StreamRng::seeded(50);
    let request = || LeaseRequest {
        holder: actor(1),
        incarnation: incarnation(4242, "start-a"),
        daemon_epoch: DaemonEpoch::new(2),
        ttl: DurationMs::from_millis(1_000),
    };
    let first = ResourceOwnership::unowned(resource())
        .acquire(&request(), lease_id(1), &mut first_rng, at(1))
        .expect("acquire");
    let second = first
        .ownership
        .acquire(&request(), lease_id(2), &mut second_rng, at(5_000))
        .expect("takeover after expiry");
    assert_ne!(first.token, second.token);
}

// ----------------------------------------------------------- items 9 and 10 --

/// Review acceptance item 9: a receipt ACK permits journal retention and
/// nothing else. It cannot close a worker-result obligation.
#[test]
fn a_receipt_ack_cannot_close_a_result_obligation() {
    let obligation = completed_obligation();
    assert_eq!(obligation.state(), ObligationState::CompletedUnprocessed);

    let ack = ReceiptAck::new(actor(1), command_id(9), at(31));
    let acked = completed_command()
        .apply(&MutationCommandEvent::ReceiptAcknowledged(ack))
        .expect("acking a committed result is legal")
        .advanced()
        .expect("acking advances");

    // What the ACK actually bought: retention eligibility, and only after the
    // policy age as well.
    assert_eq!(
        acked.compaction_eligibility(at(32), DurationMs::from_millis(1_000)),
        CompactionEligibility::Retained
    );
    assert_eq!(
        acked.compaction_eligibility(at(5_000), DurationMs::from_millis(1_000)),
        CompactionEligibility::Eligible
    );

    // What it did not buy. `ReceiptAck` holds an actor and a command identity;
    // it has no obligation, no claim, no binding generation and no disposition,
    // and no `From`/`Into` toward `AckRequest`. Closing an obligation requires
    // `claim::acknowledge(&AckRequest, ...)`, and there is no way to build one
    // from the value below — this line is the whole demonstration:
    assert_eq!(ack.actor(), actor(1));
    assert_eq!(ack.command(), command_id(9));

    // The obligation is exactly where it was.
    assert_eq!(obligation.state(), ObligationState::CompletedUnprocessed);
    assert!(obligation.is_open());
}

/// Review acceptance item 10: every transport-level receipt and notification
/// may be acknowledged while the obligation stays open, and only a valid fenced
/// `foreman_ack` closes it.
#[test]
fn every_transport_ack_may_land_while_the_obligation_stays_open() {
    let obligation = completed_obligation();
    let bindings = bound_ledger();

    // Layer 1: the mutation-command receipt ACK.
    let acked_command = completed_command()
        .apply(&MutationCommandEvent::ReceiptAcknowledged(ReceiptAck::new(
            actor(1),
            command_id(9),
            at(31),
        )))
        .expect("layer 1 ACK is legal")
        .advanced()
        .expect("layer 1 ACK advances");
    assert_eq!(
        acked_command.compaction_eligibility(at(5_000), DurationMs::from_millis(1_000)),
        CompactionEligibility::Eligible
    );
    assert_eq!(
        obligation.state(),
        ObligationState::CompletedUnprocessed,
        "a receipt ACK is not a disposition"
    );

    // Layer 1b: an external attempt that provably landed. A completed
    // consequential effect is still not a review.
    let dispatched = advance_attempt(
        &intent_only(idempotent("k-1")),
        &ExternalAttemptEvent::CallDispatched { at: at(13) },
    );
    let landed = advance_attempt(
        &dispatched,
        &ExternalAttemptEvent::Completed {
            evidence: 7,
            at: at(14),
        },
    );
    assert_eq!(landed.state(), ExternalAttemptState::Completed);
    assert_eq!(obligation.state(), ObligationState::CompletedUnprocessed);

    // Layer 2: the browser wake was accepted, and a foreman claimed it. The
    // obligation is now held — but held is not closed.
    let mut rng = StreamRng::seeded(0);
    let wake = accepted_wake(&mut rng, &obligation);
    let minted = mint_claim(
        &ResumeRequest {
            obligation: obligation.id(),
            presented_delivery_id: wake.delivery_id().clone(),
            binding_generation: BindingGeneration::FIRST,
            expected_version: obligation.version(),
            expected_source: obligation.source().clone(),
        },
        &bindings,
        &wake,
        &obligation,
        claim_id(),
        at(20),
        DurationMs::from_millis(60_000),
    )
    .expect("a fully fenced resume mints a claim");
    assert_eq!(minted.obligation.state(), ObligationState::ClaimedByForeman);
    assert!(minted.obligation.is_open(), "claiming is not closing");

    let processing = advance_obligation(
        &minted.obligation,
        &ObligationEvent::HandoffDelivered {
            claim: claim_id(),
            at: at(21),
        },
    );
    assert!(processing.is_open(), "handing over is not closing");

    // Layer 3, and only layer 3, closes it.
    let closed = close_with(&bindings, &minted.claim, &processing);
    assert_eq!(closed.state(), ObligationState::Acknowledged);
    assert!(!closed.is_open());

    // Everything before that point left the obligation open, and the input
    // values are all untouched.
    assert_eq!(obligation.state(), ObligationState::CompletedUnprocessed);
    assert_eq!(processing.state(), ObligationState::Processing);
}

fn close_with(
    bindings: &BindingLedger,
    claim: &ForemanClaim,
    obligation: &Obligation,
) -> Obligation {
    let request = AckRequest {
        obligation: obligation.id(),
        expected_version: obligation.version(),
        expected_source: obligation.source().clone(),
        binding_generation: BindingGeneration::FIRST,
        claim: claim_id(),
        disposition: Disposition::Accepted,
        at: at(22),
    };
    match acknowledge(&request, bindings, claim, obligation, at(22))
        .expect("a fully fenced semantic ACK is legal")
    {
        AckOutcome::Committed(committed) => committed.obligation,
        AckOutcome::AlreadyCommitted => panic!("the first ACK must commit"),
    }
}

// ------------------------------------------------------- supporting proofs --

/// Proof classes are exactly that: there is no weak variant, and the dispatch
/// fence decides which of them still establishes absence.
#[test]
fn failure_before_effect_always_requires_a_proof_that_fits_the_window() {
    let cases: [(NoEffectClass, bool, bool); 8] = [
        // (class, dispatched, accepted)
        (NoEffectClass::NotAttempted, false, true),
        (NoEffectClass::NotAttempted, true, false),
        (NoEffectClass::RejectedBeforeDispatch, false, true),
        (NoEffectClass::RejectedBeforeDispatch, true, true),
        (
            NoEffectClass::DestinationRefusedWithoutApplying,
            false,
            false,
        ),
        (NoEffectClass::DestinationRefusedWithoutApplying, true, true),
        (
            NoEffectClass::PreconditionRejectedAtDestination,
            false,
            false,
        ),
        (NoEffectClass::PreconditionRejectedAtDestination, true, true),
    ];

    for (proof, dispatched, accepted) in cases {
        let mut attempt = intent_only(ExternalEffectClass::NonIdempotentWrite);
        if dispatched {
            attempt = advance_attempt(
                &attempt,
                &ExternalAttemptEvent::CallDispatched { at: at(2) },
            );
        }
        let outcome = attempt.apply(&ExternalAttemptEvent::FailedBeforeEffect { proof, at: at(3) });
        match (accepted, outcome) {
            (true, Ok(transition)) => {
                let failed = transition.advanced().expect("proof advances");
                assert_eq!(failed.state(), ExternalAttemptState::FailedBeforeEffect);
                assert_eq!(failed.no_effect(), Some(proof));
            }
            (false, Err(err)) => {
                assert_eq!(
                    err.code(),
                    "effect_not_proven_absent",
                    "{proof:?} dispatched={dispatched}"
                );
                assert_eq!(attempt.state(), ExternalAttemptState::IntentRecorded);
            }
            (expected, actual) => {
                panic!(
                    "{proof:?} dispatched={dispatched}: expected accepted={expected}, got {actual:?}"
                );
            }
        }
    }
}

/// A completed effect replays without going near a permit, and the replay is
/// the only thing a completed attempt will ever produce.
#[test]
fn a_completed_effect_replays_and_never_executes_again() {
    let dispatched = advance_attempt(
        &intent_only(idempotent("k-1")),
        &ExternalAttemptEvent::CallDispatched { at: at(2) },
    );
    let completed = advance_attempt(
        &dispatched,
        &ExternalAttemptEvent::Completed {
            evidence: 21,
            at: at(3),
        },
    );

    let decision = completed
        .decide(None, |value: &u8| u32::from(*value) * 2)
        .expect("a completed attempt always replays");
    assert_eq!(decision.replayed(), Some(42));

    assert_eq!(
        completed
            .admit_retry(&destination("turn-7"), &idempotent("k-1"))
            .unwrap_err()
            .code(),
        "attempt_already_completed"
    );
}

// The added `ConflictKind` codes are checked for stability and uniqueness
// alongside every pre-existing one, in a single enumeration:
// `state_machine_invariants.rs::conflict_codes_are_stable_and_unique`. Keeping
// a second list here would let the two drift.

/// The forbidden-content shapes cannot enter the new durable types either:
/// every string-shaped field is still a `SafeToken`, and a resource's canonical
/// name is reduced to a digest before it is recorded.
#[test]
fn the_new_durable_types_have_nowhere_to_put_forbidden_content() {
    for forbidden in [
        "rm -rf /Users/peter/project",
        "/Users/peter/.claude/projects/x/transcript.jsonl",
        "{\"tool_input\":{\"command\":\"cat secrets\"}}",
        "sk-ant-api03-REDACTED-LOOKING-KEY/with/slashes",
    ] {
        // Idempotency keys, destinations, command kinds, resource namespaces
        // and process-start identities are all `SafeToken`-shaped.
        assert!(
            SafeToken::new(forbidden).is_err(),
            "{forbidden} must not be representable"
        );
    }

    // A resource's canonical name is the one place a real path is *accepted*,
    // and it is hashed on the way in: the identity carries 32 bytes and the
    // namespace, and the path is not recoverable from the value.
    let identity = ResourceIdentity::canonical(
        ResourceNamespace::new(token("session")),
        "/Users/peter/.claude/projects/secret",
    );
    assert_eq!(identity.digest().len(), 32);
    let rendered = format!("{identity:?}");
    assert!(
        !rendered.contains("peter") && !rendered.contains("claude"),
        "the canonical name must not survive into the record: {rendered}"
    );
}
