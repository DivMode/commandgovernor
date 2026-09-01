//! Foreman claims: minting from an accepted wake, and the ACK that closes work.
//!
//! These are the two cross-aggregate operations, so they are functions rather
//! than methods: minting a claim needs the binding ledger, the accepted wake,
//! and the obligation to agree simultaneously.
//!
//! A claim is minted only from an **accepted, current-generation** delivery
//! whose random `delivery_id` the caller presented, and whose target obligation
//! version and source fact are still current. Every failure of that check
//! reports the same [`Conflict::UnknownDeliveryId`], so a connector in another
//! conversation cannot use the error to learn whether a delivery exists.

use crate::binding::BindingLedger;
use crate::delivery::{BrowserWake, DeliveryId};
use crate::error::{Conflict, Transition};
use crate::fence::{BindingGeneration, ObligationVersion, SourceRef};
use crate::id::{ClaimId, ObligationId};
use crate::obligation::{AckRequest, Obligation, ObligationEvent};
use crate::outbound::DeliveryState;
use crate::time::{DurationMs, Timestamp};

/// Lifecycle of a bounded foreman claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaimState {
    /// The claim currently holds the obligation.
    Live,
    /// The claim's bound lifetime elapsed.
    Expired,
    /// The claim closed the obligation with a disposition.
    Closed,
}

/// A bounded claim over one obligation under one binding generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForemanClaim {
    id: ClaimId,
    obligation: ObligationId,
    version_at_claim: ObligationVersion,
    binding_generation: BindingGeneration,
    wake_delivery: DeliveryId,
    state: ClaimState,
    created_at: Timestamp,
    expires_at: Timestamp,
}

impl ForemanClaim {
    /// Claim identity.
    #[must_use]
    pub const fn id(&self) -> ClaimId {
        self.id
    }

    /// Obligation held.
    #[must_use]
    pub const fn obligation(&self) -> ObligationId {
        self.obligation
    }

    /// Obligation version at the moment of claiming.
    #[must_use]
    pub const fn version_at_claim(&self) -> ObligationVersion {
        self.version_at_claim
    }

    /// Binding generation the claim was minted under.
    #[must_use]
    pub const fn binding_generation(&self) -> BindingGeneration {
        self.binding_generation
    }

    /// The accepted wake the claim was minted from.
    #[must_use]
    pub const fn wake_delivery(&self) -> &DeliveryId {
        &self.wake_delivery
    }

    /// Current claim state.
    #[must_use]
    pub const fn state(&self) -> ClaimState {
        self.state
    }

    /// Instant the claim was minted.
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Instant the claim stops authorising mutations.
    #[must_use]
    pub const fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Reports whether the claim has passed its expiry at `now`.
    #[must_use]
    pub const fn is_expired_at(&self, now: Timestamp) -> bool {
        now.as_unix_millis() >= self.expires_at.as_unix_millis()
    }

    /// Marks an elapsed claim expired.
    ///
    /// Expiry is internal coordination. It never closes work and never
    /// releases a pinned artifact; the obligation returns to its prior
    /// attention state through [`ObligationEvent::ClaimExpired`].
    #[must_use]
    pub fn expire(&self) -> Self {
        let mut next = self.clone();
        next.state = ClaimState::Expired;
        next
    }

    /// Verifies the claim can still authorise a mutation at `now`.
    ///
    /// # Errors
    ///
    /// Returns [`Conflict::ExpiredClaim`] for an elapsed or already-finished
    /// claim.
    pub const fn require_live(&self, now: Timestamp) -> Result<(), Conflict> {
        if !matches!(self.state, ClaimState::Live) || self.is_expired_at(now) {
            return Err(Conflict::ExpiredClaim { claim: self.id });
        }
        Ok(())
    }
}

/// The persisted parts of one foreman claim.
///
/// Only [`ForemanClaim::rehydrate`] consumes this, and it re-proves the parts
/// against the wake that minted them before producing a claim. It exists
/// because a claim cannot be rebuilt by replaying [`mint_claim`]: minting
/// advances the obligation, so replaying it against the *current* obligation
/// would either fail or mint a different claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedClaim {
    /// Claim identity.
    pub id: ClaimId,
    /// Obligation the claim holds.
    pub obligation: ObligationId,
    /// Obligation version at the moment of claiming.
    pub version_at_claim: ObligationVersion,
    /// Binding generation the claim was minted under.
    pub binding_generation: BindingGeneration,
    /// Random correlation ID of the accepted wake it was minted from.
    pub wake_delivery: DeliveryId,
    /// Recorded lifecycle state.
    pub state: ClaimState,
    /// Instant the claim was minted.
    pub created_at: Timestamp,
    /// Instant the claim stops authorising mutations.
    pub expires_at: Timestamp,
}

/// A persisted claim could not be re-proved against its originating wake.
///
/// The stored row is corrupt, or was written against a different wake. Fail
/// closed: a claim whose provenance cannot be re-established must not authorise
/// an ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("persisted claim does not match the accepted wake it records")]
pub struct ClaimProvenanceMismatch;

impl ForemanClaim {
    /// Rebuilds a claim the store previously persisted.
    ///
    /// A *validating* loader, not a field-wise constructor: it re-runs the wake
    /// half of [`mint_claim`]'s admission test against `wake`, which is frozen
    /// once accepted and therefore still checkable at any later time. The
    /// obligation half is deliberately not re-checked — minting advanced the
    /// obligation past `version_at_claim`, so the current version is *expected*
    /// to differ.
    ///
    /// # Errors
    ///
    /// Returns [`ClaimProvenanceMismatch`] when the wake is not an accepted,
    /// same-generation wake for this obligation, when the recorded correlation
    /// ID is not the wake's, or when the recorded lifetime runs backwards.
    pub fn rehydrate(
        parts: PersistedClaim,
        wake: &BrowserWake,
    ) -> Result<Self, ClaimProvenanceMismatch> {
        let provenance = wake.state() == DeliveryState::Accepted
            && wake.binding_generation() == parts.binding_generation
            && wake.target().obligation == parts.obligation
            && wake.correlates_with(&parts.wake_delivery)
            && parts.expires_at >= parts.created_at;
        if !provenance {
            return Err(ClaimProvenanceMismatch);
        }
        Ok(Self {
            id: parts.id,
            obligation: parts.obligation,
            version_at_claim: parts.version_at_claim,
            binding_generation: parts.binding_generation,
            wake_delivery: parts.wake_delivery,
            state: parts.state,
            created_at: parts.created_at,
            expires_at: parts.expires_at,
        })
    }
}

/// The fences `foreman_resume` must present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRequest {
    /// Obligation the caller wants to claim.
    pub obligation: ObligationId,
    /// The random correlation ID from the accepted browser wake.
    pub presented_delivery_id: DeliveryId,
    /// Current binding generation.
    pub binding_generation: BindingGeneration,
    /// Obligation version the caller believes is current.
    pub expected_version: ObligationVersion,
    /// Source fact the caller believes is current.
    pub expected_source: SourceRef,
}

/// A successfully minted claim and the obligation it now holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimMinted {
    /// The new claim.
    pub claim: ForemanClaim,
    /// The obligation, advanced to `claimed_by_foreman`.
    pub obligation: Obligation,
}

/// Mints a claim from an accepted current-generation wake.
///
/// # Errors
///
/// - [`Conflict::NoActiveBinding`] / [`Conflict::StaleBindingGeneration`] /
///   [`Conflict::UnknownBindingGeneration`] from the binding fence;
/// - [`Conflict::UnknownDeliveryId`] when the wake is not an accepted,
///   current-generation delivery for this obligation, or the presented
///   correlation ID does not match it;
/// - [`Conflict::StaleDeliveryTarget`] when the wake's snapshot went stale;
/// - [`Conflict::StaleObligationVersion`] / [`Conflict::StaleSourceFence`] when
///   the caller's own fences are stale;
/// - [`Conflict::ObligationAlreadyClaimed`] when another claim holds it.
///
/// Every one of these leaves both inputs untouched.
pub fn mint_claim(
    request: &ResumeRequest,
    bindings: &BindingLedger,
    wake: &BrowserWake,
    obligation: &Obligation,
    claim_id: ClaimId,
    now: Timestamp,
    lifetime: DurationMs,
) -> Result<ClaimMinted, Conflict> {
    bindings.fence(request.binding_generation)?;

    // The wake must be this obligation's accepted wake for this generation,
    // and the caller must actually hold its random correlation ID. All four
    // failures report the same conflict on purpose.
    let wake_is_usable = wake.binding_generation() == request.binding_generation
        && wake.target().obligation == request.obligation
        && obligation.id() == request.obligation
        && wake.state() == DeliveryState::Accepted
        && wake.correlates_with(&request.presented_delivery_id);
    if !wake_is_usable {
        return Err(Conflict::UnknownDeliveryId);
    }

    wake.require_current_target(obligation)?;

    let claimed = obligation
        .apply(&ObligationEvent::ForemanClaimed {
            claim: claim_id,
            binding_generation: request.binding_generation,
            expected_version: request.expected_version,
            expected_source: request.expected_source.clone(),
            at: now,
        })?
        .or_unchanged(obligation.clone());

    Ok(ClaimMinted {
        claim: ForemanClaim {
            id: claim_id,
            obligation: request.obligation,
            version_at_claim: obligation.version(),
            binding_generation: request.binding_generation,
            wake_delivery: request.presented_delivery_id.clone(),
            state: ClaimState::Live,
            created_at: now,
            expires_at: now.saturating_add(lifetime),
        },
        obligation: claimed,
    })
}

/// The new state produced by an ACK that closed an obligation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckCommitted {
    /// The closed obligation.
    pub obligation: Obligation,
    /// The claim, now finished.
    pub claim: ForemanClaim,
}

/// The result of a fenced ACK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckOutcome {
    /// The ACK closed the obligation.
    Committed(Box<AckCommitted>),
    /// An exact repeat of an already-committed ACK. Idempotent success.
    AlreadyCommitted,
}

/// Closes an obligation with a fully fenced ACK.
///
/// # Errors
///
/// Returns the first failing fence: binding generation, claim liveness, claim
/// identity, obligation version, source fence, or disposition validity. Every
/// one leaves the obligation and the claim untouched.
pub fn acknowledge(
    request: &AckRequest,
    bindings: &BindingLedger,
    claim: &ForemanClaim,
    obligation: &Obligation,
    now: Timestamp,
) -> Result<AckOutcome, Conflict> {
    // An exact repeat of the ACK that already closed this obligation is
    // idempotent success. It is checked first because by then the claim is
    // finished and the liveness fence below would otherwise reject a request
    // that has, in fact, already succeeded. Every fence must match for this to
    // fire, so it is not a way around any of them.
    if obligation
        .committed_ack()
        .is_some_and(|committed| committed.matches(request))
    {
        return Ok(AckOutcome::AlreadyCommitted);
    }

    bindings.fence(request.binding_generation)?;
    if claim.id != request.claim || claim.obligation != request.obligation {
        return Err(Conflict::StaleClaim {
            presented: request.claim,
            obligation: request.obligation,
        });
    }
    claim.require_live(now)?;

    match obligation.apply(&ObligationEvent::ForemanAcked(Box::new(request.clone())))? {
        Transition::Duplicate => Ok(AckOutcome::AlreadyCommitted),
        Transition::Advanced(closed) => {
            let mut finished = claim.clone();
            finished.state = ClaimState::Closed;
            Ok(AckOutcome::Committed(Box::new(AckCommitted {
                obligation: closed,
                claim: finished,
            })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::test_support as binding_support;
    use crate::delivery::{AcceptedWakeEvidence, WakeTarget};
    use crate::fence::{AttemptNo, SafeToken};
    use crate::foreman_turn::ProviderMessageRef;
    use crate::id::ForemanBindingId;
    use crate::obligation::{Disposition, ObligationState, test_support as obligation_support};
    use crate::outbound::DeliveryEvent;
    use crate::random::SecureRandom;
    use uuid::Uuid;

    struct StreamRng {
        next: u8,
    }

    impl SecureRandom for StreamRng {
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for slot in dest.iter_mut() {
                *slot = self.next;
                self.next = self.next.wrapping_add(1);
            }
        }
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    fn claim_id() -> ClaimId {
        ClaimId::from_uuid(Uuid::from_u128(100))
    }

    fn accepted_wake(rng: &mut StreamRng, obligation: &Obligation) -> BrowserWake {
        let wake = BrowserWake::create(
            rng,
            WakeTarget::snapshot(obligation),
            ForemanBindingId::from_uuid(Uuid::from_u128(1)),
            BindingGeneration::FIRST,
            3,
        );
        wake.apply(&DeliveryEvent::AttemptClaimed { at: at(1) })
            .unwrap()
            .advanced()
            .unwrap()
            .apply(&DeliveryEvent::ActivationArmed {
                attempt: AttemptNo::FIRST,
                at: at(2),
            })
            .unwrap()
            .advanced()
            .unwrap()
            .apply(&DeliveryEvent::AttemptAccepted {
                attempt: AttemptNo::FIRST,
                evidence: AcceptedWakeEvidence::new(
                    crate::binding::ConversationRef::new(SafeToken::new("conv-A").unwrap()),
                    ProviderMessageRef::new(SafeToken::new("msg-1").unwrap()),
                ),
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap()
    }

    fn resume_request(wake: &BrowserWake, obligation: &Obligation) -> ResumeRequest {
        ResumeRequest {
            obligation: obligation.id(),
            presented_delivery_id: wake.delivery_id().clone(),
            binding_generation: BindingGeneration::FIRST,
            expected_version: obligation.version(),
            expected_source: obligation.source().clone(),
        }
    }

    #[test]
    fn an_accepted_wake_with_the_right_correlation_id_mints_one_claim() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = accepted_wake(&mut rng, &obligation);
        let bindings = binding_support::bound("conv-A");

        let minted = mint_claim(
            &resume_request(&wake, &obligation),
            &bindings,
            &wake,
            &obligation,
            claim_id(),
            at(10),
            DurationMs::from_millis(60_000),
        )
        .expect("a fully fenced resume mints a claim");

        assert_eq!(minted.claim.state(), ClaimState::Live);
        assert_eq!(minted.claim.expires_at(), at(60_010));
        assert_eq!(minted.obligation.state(), ObligationState::ClaimedByForeman);
        assert_eq!(
            obligation.state(),
            ObligationState::CompletedUnprocessed,
            "the input value is untouched"
        );
    }

    #[test]
    fn deterministic_metadata_alone_cannot_claim() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = accepted_wake(&mut rng, &obligation);
        let bindings = binding_support::bound("conv-A");

        // The attacker knows the obligation ID, the generation, the revision
        // and therefore the whole deterministic delivery key -- and presents a
        // correlation ID derived from all of it.
        let mut derived = [0u8; crate::delivery::DELIVERY_ID_BYTES];
        derived.copy_from_slice(wake.delivery_key().as_bytes());
        let forged = DeliveryId::from_persisted_bytes(derived);

        let request = ResumeRequest {
            presented_delivery_id: forged,
            ..resume_request(&wake, &obligation)
        };
        let err = mint_claim(
            &request,
            &bindings,
            &wake,
            &obligation,
            claim_id(),
            at(10),
            DurationMs::from_millis(60_000),
        )
        .unwrap_err();
        assert_eq!(err.code(), "unknown_delivery_id");
    }

    #[test]
    fn an_unaccepted_wake_cannot_mint_a_claim() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let pending = BrowserWake::create(
            &mut rng,
            WakeTarget::snapshot(&obligation),
            ForemanBindingId::from_uuid(Uuid::from_u128(1)),
            BindingGeneration::FIRST,
            3,
        );
        let bindings = binding_support::bound("conv-A");
        let err = mint_claim(
            &resume_request(&pending, &obligation),
            &bindings,
            &pending,
            &obligation,
            claim_id(),
            at(10),
            DurationMs::from_millis(60_000),
        )
        .unwrap_err();
        assert_eq!(err.code(), "unknown_delivery_id");
    }

    #[test]
    fn a_stale_binding_generation_cannot_resume() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = accepted_wake(&mut rng, &obligation);
        let rebound = binding_support::bound("conv-A")
            .apply(&crate::binding::BindingEvent::Bound {
                target: Box::new(binding_support::target("conv-B", 2)),
                at: at(5),
            })
            .unwrap()
            .advanced()
            .unwrap();

        let err = mint_claim(
            &resume_request(&wake, &obligation),
            &rebound,
            &wake,
            &obligation,
            claim_id(),
            at(10),
            DurationMs::from_millis(60_000),
        )
        .unwrap_err();
        assert_eq!(err.code(), "stale_binding_generation");
    }

    fn minted() -> (BindingLedger, ForemanClaim, Obligation) {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = accepted_wake(&mut rng, &obligation);
        let bindings = binding_support::bound("conv-A");
        let minted = mint_claim(
            &resume_request(&wake, &obligation),
            &bindings,
            &wake,
            &obligation,
            claim_id(),
            at(10),
            DurationMs::from_millis(60_000),
        )
        .expect("resume succeeds");
        let processing = minted
            .obligation
            .apply(&ObligationEvent::HandoffDelivered {
                claim: claim_id(),
                at: at(11),
            })
            .unwrap()
            .advanced()
            .unwrap();
        (bindings, minted.claim, processing)
    }

    fn ack_for(obligation: &Obligation, disposition: Disposition) -> AckRequest {
        AckRequest {
            obligation: obligation.id(),
            expected_version: obligation.version(),
            expected_source: obligation.source().clone(),
            binding_generation: BindingGeneration::FIRST,
            claim: claim_id(),
            disposition,
            at: at(12),
        }
    }

    #[test]
    fn rehydration_round_trips_a_persisted_claim() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = accepted_wake(&mut rng, &obligation);
        let bindings = binding_support::bound("conv-A");
        let minted = mint_claim(
            &resume_request(&wake, &obligation),
            &bindings,
            &wake,
            &obligation,
            claim_id(),
            at(10),
            DurationMs::from_millis(60_000),
        )
        .unwrap();
        let original = minted.claim;

        let restored = ForemanClaim::rehydrate(
            PersistedClaim {
                id: original.id(),
                obligation: original.obligation(),
                version_at_claim: original.version_at_claim(),
                binding_generation: original.binding_generation(),
                wake_delivery: original.wake_delivery().clone(),
                state: original.state(),
                created_at: original.created_at(),
                expires_at: original.expires_at(),
            },
            &wake,
        )
        .expect("a faithfully persisted claim re-proves against its wake");
        assert_eq!(restored, original);
        // And it is still usable for the fence it exists for.
        assert!(restored.require_live(at(11)).is_ok());
    }

    #[test]
    fn rehydration_refuses_a_claim_whose_wake_does_not_prove_it() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = accepted_wake(&mut rng, &obligation);
        let parts = |wake_delivery: DeliveryId| PersistedClaim {
            id: claim_id(),
            obligation: obligation.id(),
            version_at_claim: obligation.version(),
            binding_generation: BindingGeneration::FIRST,
            wake_delivery,
            state: ClaimState::Live,
            created_at: at(10),
            expires_at: at(60_010),
        };

        // A correlation ID this wake does not carry.
        let mut other = StreamRng { next: 200 };
        let forged = DeliveryId::generate(&mut other);
        assert_eq!(
            ForemanClaim::rehydrate(parts(forged), &wake),
            Err(ClaimProvenanceMismatch)
        );

        // A wake that was never accepted cannot have minted a claim.
        let pending = BrowserWake::create(
            &mut rng,
            WakeTarget::snapshot(&obligation),
            ForemanBindingId::from_uuid(Uuid::from_u128(1)),
            BindingGeneration::FIRST,
            3,
        );
        assert_eq!(
            ForemanClaim::rehydrate(parts(pending.delivery_id().clone()), &pending),
            Err(ClaimProvenanceMismatch)
        );

        // A lifetime that runs backwards is not a lifetime.
        let backwards = PersistedClaim {
            expires_at: at(9),
            ..parts(wake.delivery_id().clone())
        };
        assert_eq!(
            ForemanClaim::rehydrate(backwards, &wake),
            Err(ClaimProvenanceMismatch)
        );
    }

    #[test]
    fn a_fully_fenced_ack_closes_the_obligation_exactly_once() {
        let (bindings, claim, processing) = minted();
        let request = ack_for(&processing, Disposition::Accepted);
        let outcome =
            acknowledge(&request, &bindings, &claim, &processing, at(12)).expect("ACK is legal");
        let AckOutcome::Committed(committed) = outcome else {
            panic!("expected a committed ACK");
        };
        let AckCommitted { obligation, claim } = *committed;
        assert_eq!(obligation.state(), ObligationState::Acknowledged);
        assert_eq!(claim.state(), ClaimState::Closed);

        let repeat = acknowledge(&request, &bindings, &claim, &obligation, at(13))
            .expect("an exact repeat is idempotent");
        assert_eq!(repeat, AckOutcome::AlreadyCommitted);
    }

    #[test]
    fn an_expired_claim_cannot_ack() {
        let (bindings, claim, processing) = minted();
        let request = ack_for(&processing, Disposition::Accepted);
        let err = acknowledge(&request, &bindings, &claim, &processing, at(60_011)).unwrap_err();
        assert_eq!(err.code(), "expired_claim");
        assert_eq!(processing.state(), ObligationState::Processing);
    }

    #[test]
    fn another_claim_cannot_ack() {
        let (bindings, claim, processing) = minted();
        let request = AckRequest {
            claim: ClaimId::from_uuid(Uuid::from_u128(777)),
            ..ack_for(&processing, Disposition::Accepted)
        };
        let err = acknowledge(&request, &bindings, &claim, &processing, at(12)).unwrap_err();
        assert_eq!(err.code(), "stale_claim");
    }

    #[test]
    fn a_rebind_after_claiming_blocks_the_ack() {
        let (bindings, claim, processing) = minted();
        let rebound = bindings
            .apply(&crate::binding::BindingEvent::Bound {
                target: Box::new(binding_support::target("conv-B", 2)),
                at: at(11),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let request = ack_for(&processing, Disposition::Accepted);
        let err = acknowledge(&request, &rebound, &claim, &processing, at(12)).unwrap_err();
        assert_eq!(err.code(), "stale_binding_generation");
        assert_eq!(
            processing.state(),
            ObligationState::Processing,
            "the artifact stays pinned and nothing closed"
        );
    }
}
