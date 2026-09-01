//! `foreman_resume` claim minting, handoff, and the fenced ACK.
//!
//! Two of `docs/data-model.md`'s critical transaction boundaries live here, and
//! they are the two that decide whether work stays owed.
//!
//! **`foreman_resume`** verifies the accepted random wake `delivery_id`, the
//! target snapshot and the current generation, creates one current claim,
//! updates the obligation projection and version, and appends the claim event.
//! Reading the artifact happens *after* the transaction, and failing to read it
//! does not close the claim or the obligation — which is why this operation
//! touches no file and has no way to.
//!
//! **ACK** verifies obligation version, source event, binding generation, claim
//! and disposition, appends the explicit disposition event, and closes the
//! projection. It is the only normal path that closes an obligation, and it is
//! [`governor_core::claim::acknowledge`] that decides — this module supplies
//! the fenced state and persists the answer.
//!
//! Both delegate every rule to `governor-core`. A stale fence returns the typed
//! conflict and the transaction rolls back, so zero rows change.

use governor_core::claim::{
    AckOutcome, ClaimState, ForemanClaim, PersistedClaim, ResumeRequest, acknowledge, mint_claim,
};
use governor_core::delivery::DeliveryId;
use governor_core::error::Conflict;
use governor_core::fence::{BindingGeneration, ObligationVersion, SourceRef};
use governor_core::id::{ClaimId, EventId, ObligationId};
use governor_core::obligation::{AckRequest, Disposition, ObligationEvent};
use governor_core::time::{DurationMs, Timestamp};
use rusqlite::{OptionalExtension as _, params};

use crate::codec::{
    ActorClass, ClaimLifecycle, encode_claim_state, encode_disposition, id_text, parse_delivery_id,
    parse_id, parse_u64, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::load;
use crate::ops::worker::{ObligationAdvanced, advanced, unchanged};
use crate::ops::{internal_source, record_obligation_transition};
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// The fences `foreman_resume` presents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintClaimRequest {
    /// Obligation the caller wants to claim.
    pub obligation: ObligationId,
    /// The random correlation ID from the accepted browser wake.
    pub presented_delivery_id: DeliveryId,
    /// Binding generation the caller believes is current.
    pub binding_generation: BindingGeneration,
    /// Obligation version the caller believes is current.
    pub expected_version: ObligationVersion,
    /// Source fact the caller believes is current.
    pub expected_source: SourceRef,
    /// How long the claim authorises mutations for.
    pub lifetime: DurationMs,
}

/// The claim a resume minted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedClaim {
    /// The new claim.
    pub claim: ClaimId,
    /// Instant it stops authorising mutations.
    pub expires_at: Timestamp,
    /// The obligation, now `claimed_by_foreman`.
    pub obligation: ObligationAdvanced,
}

/// Mints one claim from an accepted current-generation wake.
pub(crate) struct MintForemanClaim {
    request: MintClaimRequest,
    claim: ClaimId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for MintForemanClaim {
    type Request = MintClaimRequest;
    type Committed = MintedClaim;
    type Output = MintedClaim;

    const NAME: &'static str = "mint_foreman_claim";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            claim: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let bindings = load::bindings(tx)?;
        let loaded = load::obligation(tx, self.request.obligation)?;
        let wake = load::wake_by_delivery_id(tx, &self.request.presented_delivery_id)?.wake;

        let minted = mint_claim(
            &ResumeRequest {
                obligation: self.request.obligation,
                presented_delivery_id: self.request.presented_delivery_id.clone(),
                binding_generation: self.request.binding_generation,
                expected_version: self.request.expected_version,
                expected_source: self.request.expected_source.clone(),
            },
            &bindings,
            &wake,
            &loaded.projection,
            self.claim,
            self.now,
            self.request.lifetime,
        )?;

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ForemanClaimMinted,
                source: internal_source(self.claim, "foreman_claim_minted")?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .id("claim_id", self.claim)
                    .int(
                        "binding_generation",
                        store_u64(
                            self.request.binding_generation.get(),
                            "events",
                            "binding_generation",
                        )?,
                    )
                    .int(
                        "expected_version",
                        store_u64(
                            self.request.expected_version.get(),
                            "events",
                            "expected_version",
                        )?,
                    ),
            },
        )?
        .seq();

        tx.conn().execute(
            "INSERT INTO foreman_claims (claim_id, obligation_id, obligation_version_at_claim,
                    binding_generation, wake_delivery_id, state, created_event_seq, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id_text(self.claim),
                id_text(self.request.obligation),
                store_u64(
                    minted.claim.version_at_claim().get(),
                    "foreman_claims",
                    "obligation_version_at_claim"
                )?,
                store_u64(
                    self.request.binding_generation.get(),
                    "foreman_claims",
                    "binding_generation"
                )?,
                self.request.presented_delivery_id.expose_hex(),
                encode_claim_state(ClaimLifecycle::Live),
                event::store_seq(seq)?,
                store_time(minted.claim.expires_at()),
            ],
        )?;

        record_obligation_transition(
            tx,
            &loaded.projection,
            &minted.obligation,
            seq,
            loaded.source_event_seq,
            ActorClass::Foreman,
            None,
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(MintedClaim {
            claim: self.claim,
            expires_at: minted.claim.expires_at(),
            obligation: advanced(&minted.obligation),
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Recording that the result or input request reached the claiming foreman.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverHandoffRequest {
    /// Obligation being handed over.
    pub obligation: ObligationId,
    /// Claim the handoff belongs to.
    pub claim: ClaimId,
}

/// Moves a claimed obligation to `processing`.
pub(crate) struct DeliverHandoff {
    request: DeliverHandoffRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for DeliverHandoff {
    type Request = DeliverHandoffRequest;
    type Committed = ObligationAdvanced;
    type Output = ObligationAdvanced;

    const NAME: &'static str = "deliver_handoff";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let loaded = load::obligation(tx, self.request.obligation)?;
        let before = loaded.projection;
        let transition = before.apply(&ObligationEvent::HandoffDelivered {
            claim: self.request.claim,
            at: self.now,
        })?;
        let Some(after) = transition.advanced() else {
            return Ok(unchanged(&before));
        };

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ForemanHandoffDelivered,
                source: internal_source(self.request.claim, "foreman_handoff_delivered")?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new().id("claim_id", self.request.claim),
            },
        )?
        .seq();

        record_obligation_transition(
            tx,
            &before,
            &after,
            seq,
            loaded.source_event_seq,
            ActorClass::Foreman,
            None,
        )?;
        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(advanced(&after))
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// The fences a semantic ACK must present.
///
/// Every field is checked, and a stale value is a typed conflict with zero
/// mutation. This is ACK layer 3 — the only one that closes work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeRequest {
    /// Obligation being closed.
    pub obligation: ObligationId,
    /// Exact current obligation version.
    pub expected_version: ObligationVersion,
    /// Exact current source fact.
    pub expected_source: SourceRef,
    /// Current binding generation.
    pub binding_generation: BindingGeneration,
    /// Current foreman claim.
    pub claim: ClaimId,
    /// Semantic decision.
    pub disposition: Disposition,
}

/// What an ACK did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acknowledged {
    /// The obligation, now closed.
    pub obligation: ObligationAdvanced,
    /// Whether this was an exact repeat of an ACK that already committed.
    pub already_committed: bool,
}

/// Closes an obligation with a fully fenced disposition.
pub(crate) struct AcknowledgeObligation {
    request: AcknowledgeRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for AcknowledgeObligation {
    type Request = AcknowledgeRequest;
    type Committed = Acknowledged;
    type Output = Acknowledged;

    const NAME: &'static str = "acknowledge_obligation";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let bindings = load::bindings(tx)?;
        let loaded = load::obligation(tx, self.request.obligation)?;
        let claim = rehydrate_claim(tx, self.request.obligation, self.request.claim)?;

        let ack = AckRequest {
            obligation: self.request.obligation,
            expected_version: self.request.expected_version,
            expected_source: self.request.expected_source.clone(),
            binding_generation: self.request.binding_generation,
            claim: self.request.claim,
            disposition: self.request.disposition,
            at: self.now,
        };
        let outcome = acknowledge(&ack, &bindings, &claim, &loaded.projection, self.now)?;
        let AckOutcome::Committed(committed) = outcome else {
            return Ok(Acknowledged {
                obligation: unchanged(&loaded.projection),
                already_committed: true,
            });
        };

        let seq = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ForemanAcked,
                source: internal_source(self.request.claim, "foreman_acked")?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    task: Some(loaded.identity.task),
                    turn: loaded.identity.turn,
                    obligation: Some(self.request.obligation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .id("claim_id", self.request.claim)
                    .int(
                        "binding_generation",
                        store_u64(
                            self.request.binding_generation.get(),
                            "events",
                            "binding_generation",
                        )?,
                    )
                    .int(
                        "expected_version",
                        store_u64(
                            self.request.expected_version.get(),
                            "events",
                            "expected_version",
                        )?,
                    )
                    .label(
                        "disposition",
                        encode_disposition(self.request.disposition, "events")?,
                    ),
            },
        )?
        .seq();

        tx.conn().execute(
            "UPDATE foreman_claims SET state = ?2, closed_event_seq = ?3 WHERE claim_id = ?1",
            params![
                id_text(self.request.claim),
                encode_claim_state(ClaimLifecycle::Closed),
                event::store_seq(seq)?,
            ],
        )?;
        record_obligation_transition(
            tx,
            &loaded.projection,
            &committed.obligation,
            seq,
            loaded.source_event_seq,
            ActorClass::Foreman,
            Some(self.request.disposition),
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(Acknowledged {
            obligation: advanced(&committed.obligation),
            already_committed: false,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Rebuilds a persisted claim, re-proving it against the wake that minted it.
///
/// Goes through [`ForemanClaim::rehydrate`] rather than assembling the value,
/// so a row whose provenance no longer checks out fails closed instead of
/// authorising a closure.
fn rehydrate_claim(
    tx: &Tx<'_>,
    obligation: ObligationId,
    claim: ClaimId,
) -> StoreResult<ForemanClaim> {
    const TABLE: &str = "foreman_claims";
    let row: Option<(String, i64, i64, String, String, i64, i64)> = tx
        .conn()
        .query_row(
            "SELECT c.obligation_id, c.obligation_version_at_claim, c.binding_generation,
                    c.wake_delivery_id, c.state, c.expires_at_ms, e.observed_at_ms
               FROM foreman_claims c
               JOIN events e ON e.seq = c.created_event_seq
              WHERE c.claim_id = ?1",
            params![id_text(claim)],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((held, version, generation, wake_hex, state, expires, created)) = row else {
        return Err(Conflict::StaleClaim {
            presented: claim,
            obligation,
        }
        .into());
    };
    let held: ObligationId = parse_id(&held, TABLE, "obligation_id")?;
    let wake_delivery = parse_delivery_id(&wake_hex, TABLE, "wake_delivery_id")?;
    let wake = load::wake_by_delivery_id(tx, &wake_delivery)?.wake;

    ForemanClaim::rehydrate(
        PersistedClaim {
            id: claim,
            obligation: held,
            version_at_claim: ObligationVersion::new(parse_u64(
                version,
                TABLE,
                "obligation_version_at_claim",
            )?),
            binding_generation: BindingGeneration::new(parse_u64(
                generation,
                TABLE,
                "binding_generation",
            )?),
            wake_delivery,
            state: match crate::codec::decode_claim_state(&state, TABLE)? {
                ClaimLifecycle::Live => ClaimState::Live,
                ClaimLifecycle::Expired => ClaimState::Expired,
                ClaimLifecycle::Closed => ClaimState::Closed,
            },
            created_at: Timestamp::from_unix_millis(created),
            expires_at: Timestamp::from_unix_millis(expires),
        },
        &wake,
    )
    .map_err(|_| {
        CorruptValue::new(TABLE, "wake_delivery_id", CorruptReason::UnprovableEvidence).into()
    })
}
