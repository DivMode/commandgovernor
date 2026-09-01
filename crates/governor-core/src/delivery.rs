//! Browser wake delivery: two identities with two different jobs.
//!
//! ```text
//! delivery_key = H("command-governor/wake-key/v1",
//!                  obligation_id, binding_generation, delivery_revision)
//! delivery_id  = CSPRNG(>= 192 bits)
//! ```
//!
//! [`DeliveryKey`] is a deterministic, **non-secret** idempotency key. It makes
//! duplicate scheduling of one logical revision converge on one durable row and
//! it never authorises anything.
//!
//! [`DeliveryId`] is an opaque random correlation value carried in the wake and
//! required by `foreman_resume` as a possession fence. They are separate Rust
//! types precisely so that no code path can confuse them, and `DeliveryId` has
//! no constructor that takes scheduling metadata — invariant 17 is enforced by
//! the shape of the API, not by review.

use core::fmt;

use sha2::{Digest, Sha256};

use crate::binding::ConversationRef;
use crate::digest::{absorb, absorb_u32, absorb_u64, absorb_uuid};
use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{BindingGeneration, DeliveryRevision, ObligationVersion, SourceRef};
use crate::foreman_turn::ProviderMessageRef;
use crate::id::{ForemanBindingId, ObligationId};
use crate::obligation::Obligation;
use crate::outbound::{Delivery, DeliveryEvent, DeliveryState};
use crate::random::SecureRandom;

/// Domain-separation label for the deterministic wake key.
///
/// Changing this string changes every derived key and is a protocol break.
pub const WAKE_KEY_DOMAIN: &str = "command-governor/wake-key/v1";

/// Width of the random wake correlation ID, in bytes.
///
/// 256 bits, comfortably above the 192-bit floor the architecture requires.
pub const DELIVERY_ID_BYTES: usize = 32;

/// Deterministic, non-secret idempotency key for one wake revision.
///
/// Knowing this value grants nothing. It exists so two schedulers racing on the
/// same logical revision converge on one durable delivery instead of creating
/// two physical wakes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeliveryKey([u8; 32]);

impl DeliveryKey {
    /// Derives the key for one obligation, binding generation and revision.
    ///
    /// The pre-image is domain-separated and length-prefixed, so no two
    /// distinct input tuples can encode to the same byte string.
    #[must_use]
    pub fn derive(
        obligation: ObligationId,
        generation: BindingGeneration,
        revision: DeliveryRevision,
    ) -> Self {
        let mut hasher = Sha256::new();
        absorb(&mut hasher, WAKE_KEY_DOMAIN.as_bytes());
        absorb_uuid(&mut hasher, obligation.as_uuid());
        absorb_u64(&mut hasher, generation.get());
        absorb_u32(&mut hasher, revision.get());
        Self(hasher.finalize().into())
    }

    /// Returns the key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the lowercase hex form used for the durable unique index.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex_encode(&self.0)
    }
}

impl fmt::Display for DeliveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Opaque random correlation ID for one durable wake.
///
/// # Why this type has the API it has
///
/// - It is generated **only** through [`DeliveryId::generate`], which takes a
///   [`SecureRandom`] port and nothing else. There is no constructor anywhere
///   that accepts an obligation ID, a generation, a revision, or a
///   [`DeliveryKey`], so no deterministic function of scheduling metadata can
///   produce one.
/// - [`DeliveryId::from_persisted_bytes`] rehydrates a value this crate already
///   generated. It is not a derivation path: the caller must already hold the
///   32 secret bytes.
/// - `Debug` is redacted and `Display` is not implemented, so the value cannot
///   reach a log line by accident. Exposing it is an explicit call to
///   [`DeliveryId::expose_hex`] or [`DeliveryId::expose_bytes`].
#[derive(Clone)]
pub struct DeliveryId([u8; DELIVERY_ID_BYTES]);

impl DeliveryId {
    /// Draws a new correlation ID from the injected CSPRNG.
    ///
    /// This is the only way to mint one. Note what it does *not* take: no
    /// obligation, no generation, no revision, no key.
    #[must_use]
    pub fn generate(rng: &mut dyn SecureRandom) -> Self {
        let mut bytes = [0u8; DELIVERY_ID_BYTES];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Rehydrates a correlation ID the store previously persisted.
    #[must_use]
    pub const fn from_persisted_bytes(bytes: [u8; DELIVERY_ID_BYTES]) -> Self {
        Self(bytes)
    }

    /// Parses a correlation ID from its persisted lowercase hex form.
    ///
    /// # Errors
    ///
    /// Returns [`MalformedDeliveryId`] when the text is not exactly
    /// `2 * DELIVERY_ID_BYTES` hex digits. The rejected text is never echoed.
    pub fn parse_persisted(text: &str) -> Result<Self, MalformedDeliveryId> {
        if text.len() != DELIVERY_ID_BYTES * 2 {
            return Err(MalformedDeliveryId);
        }
        let mut bytes = [0u8; DELIVERY_ID_BYTES];
        for (index, slot) in bytes.iter_mut().enumerate() {
            let pair = text
                .get(index * 2..index * 2 + 2)
                .ok_or(MalformedDeliveryId)?;
            *slot = u8::from_str_radix(pair, 16).map_err(|_| MalformedDeliveryId)?;
        }
        Ok(Self(bytes))
    }

    /// Reveals the raw bytes, for persistence and for building the wake.
    ///
    /// Named `expose_` so every call site reads as a deliberate disclosure.
    #[must_use]
    pub const fn expose_bytes(&self) -> &[u8; DELIVERY_ID_BYTES] {
        &self.0
    }

    /// Reveals the hex form, for persistence and for building the wake.
    #[must_use]
    pub fn expose_hex(&self) -> String {
        hex_encode(&self.0)
    }
}

impl PartialEq for DeliveryId {
    /// Compares in constant time with respect to the byte contents.
    ///
    /// The loop always runs to completion and accumulates differences instead
    /// of returning early, so a caller probing correlation IDs learns nothing
    /// from how long the comparison took. This is a best-effort measure in safe
    /// Rust: it is not a hardware-level guarantee, and the correlation ID is an
    /// anti-confusion fence layered on connector authentication, never sole
    /// authentication.
    fn eq(&self, other: &Self) -> bool {
        let mut difference = 0u8;
        for (left, right) in self.0.iter().zip(other.0.iter()) {
            difference |= left ^ right;
        }
        difference == 0
    }
}

impl Eq for DeliveryId {}

impl core::hash::Hash for DeliveryId {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for DeliveryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeliveryId(<redacted>)")
    }
}

/// A persisted correlation ID could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed delivery correlation id")]
pub struct MalformedDeliveryId;

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Exact evidence that the intended wake was submitted.
///
/// Both fields are required, and there is no other constructor. A weak UI
/// signal ([`WeakBrowserSignal`]) has no path to this type, which is how
/// [`docs/state-machines.md`] "Accepted evidence" is enforced at compile time.
///
/// [`docs/state-machines.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/state-machines.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedWakeEvidence {
    conversation: ConversationRef,
    message: ProviderMessageRef,
}

impl AcceptedWakeEvidence {
    /// Records exact conversation and provider user-message identity.
    #[must_use]
    pub const fn new(conversation: ConversationRef, message: ProviderMessageRef) -> Self {
        Self {
            conversation,
            message,
        }
    }

    /// Conversation the message was observed in.
    #[must_use]
    pub const fn conversation(&self) -> &ConversationRef {
        &self.conversation
    }

    /// Provider-native identity of the submitted user message.
    #[must_use]
    pub const fn message(&self) -> &ProviderMessageRef {
        &self.message
    }
}

/// Browser observations that are *never* sufficient to prove acceptance.
///
/// This enum exists to be un-convertible into [`AcceptedWakeEvidence`]. An
/// adapter that can only observe these must record `ambiguous`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WeakBrowserSignal {
    /// The composer went empty.
    ComposerEmptied,
    /// The page URL changed.
    UrlChanged,
    /// A Stop button appeared.
    StopButtonAppeared,
    /// The assistant began producing output.
    AssistantStarted,
    /// The wake text was found somewhere in the DOM.
    WakeTextInDom,
}

/// The obligation snapshot a wake is targeted at.
///
/// A wake is aimed at the *exact* obligation version and source fact that
/// existed when it was scheduled. If either moves before Send, the wake is
/// stale and must not submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTarget {
    /// Obligation the wake is about.
    pub obligation: ObligationId,
    /// Obligation version at scheduling time.
    pub version: ObligationVersion,
    /// Source fact backing the obligation at scheduling time.
    pub source: SourceRef,
}

impl WakeTarget {
    /// Snapshots an obligation's current target facts.
    #[must_use]
    pub fn snapshot(obligation: &Obligation) -> Self {
        Self {
            obligation: obligation.id(),
            version: obligation.version(),
            source: obligation.source().clone(),
        }
    }

    /// Reports whether the obligation still matches this snapshot.
    #[must_use]
    pub fn still_current(&self, obligation: &Obligation) -> bool {
        obligation.id() == self.obligation
            && obligation.version() == self.version
            && obligation.source() == &self.source
    }
}

/// The persisted parts of one browser wake revision.
///
/// Only [`BrowserWake::rehydrate`] consumes this, and it validates the parts
/// against each other before producing a wake. It exists because a wake cannot
/// be rebuilt by replay: [`BrowserWake::create`] draws its correlation ID from
/// the CSPRNG port, so replaying the creation would mint a *different*
/// [`DeliveryId`] than the one the browser already carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedWake {
    /// The correlation ID that was generated once and persisted.
    pub delivery_id: DeliveryId,
    /// The deterministic key recorded alongside it.
    pub delivery_key: DeliveryKey,
    /// Obligation snapshot the wake was targeted at.
    pub target: WakeTarget,
    /// Binding record the wake belongs to.
    pub binding: ForemanBindingId,
    /// Binding generation the wake belongs to.
    pub binding_generation: BindingGeneration,
    /// Revision number within the obligation and binding generation.
    pub revision: DeliveryRevision,
    /// Attempt machine, rebuilt by folding the persisted attempt events.
    pub delivery: Delivery<AcceptedWakeEvidence>,
}

/// A persisted wake's recorded key did not match its scheduling tuple.
///
/// The stored row is corrupt or was written by a different key derivation. Fail
/// closed: a wake whose identity cannot be re-derived must not authorise a
/// claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("persisted delivery key does not match its obligation, generation and revision")]
pub struct WakeKeyMismatch;

/// One durable browser wake revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserWake {
    delivery_id: DeliveryId,
    delivery_key: DeliveryKey,
    target: WakeTarget,
    binding: ForemanBindingId,
    binding_generation: BindingGeneration,
    revision: DeliveryRevision,
    delivery: Delivery<AcceptedWakeEvidence>,
}

impl BrowserWake {
    /// Creates the first wake revision for an obligation under a binding.
    ///
    /// The deterministic key is derived from the scheduling tuple; the random
    /// correlation ID is drawn from `rng` and shares no input with it.
    #[must_use]
    pub fn create(
        rng: &mut dyn SecureRandom,
        target: WakeTarget,
        binding: ForemanBindingId,
        binding_generation: BindingGeneration,
        attempt_budget: u32,
    ) -> Self {
        Self::at_revision(
            rng,
            target,
            binding,
            binding_generation,
            DeliveryRevision::FIRST,
            attempt_budget,
        )
    }

    fn at_revision(
        rng: &mut dyn SecureRandom,
        target: WakeTarget,
        binding: ForemanBindingId,
        binding_generation: BindingGeneration,
        revision: DeliveryRevision,
        attempt_budget: u32,
    ) -> Self {
        Self {
            delivery_id: DeliveryId::generate(rng),
            delivery_key: DeliveryKey::derive(target.obligation, binding_generation, revision),
            target,
            binding,
            binding_generation,
            revision,
            delivery: Delivery::pending(attempt_budget),
        }
    }

    /// Creates the next revision for a still-open obligation.
    ///
    /// A later bounded resume is a **new** revision with a new deterministic
    /// key and an independently random correlation ID. The old revision is left
    /// exactly as it is; it is never replayed.
    #[must_use]
    pub fn next_revision(
        &self,
        rng: &mut dyn SecureRandom,
        target: WakeTarget,
        binding: ForemanBindingId,
        binding_generation: BindingGeneration,
        attempt_budget: u32,
    ) -> Self {
        let revision = if binding_generation == self.binding_generation {
            self.revision.next()
        } else {
            // A new binding generation restarts revision numbering; the key's
            // generation component keeps the two families distinct anyway.
            DeliveryRevision::FIRST
        };
        Self::at_revision(
            rng,
            target,
            binding,
            binding_generation,
            revision,
            attempt_budget,
        )
    }

    /// Rebuilds a wake revision the store previously persisted.
    ///
    /// This is a *validating* loader, not a field-wise constructor: the
    /// deterministic key is re-derived from the obligation, generation and
    /// revision in `parts` and must equal the persisted one. A row whose
    /// identity does not re-derive is refused rather than trusted.
    ///
    /// It is not a derivation path for [`DeliveryId`] either — the caller must
    /// already hold the persisted correlation ID, exactly as
    /// [`DeliveryId::from_persisted_bytes`] requires.
    ///
    /// # Errors
    ///
    /// Returns [`WakeKeyMismatch`] when the recorded key does not match.
    pub fn rehydrate(parts: PersistedWake) -> Result<Self, WakeKeyMismatch> {
        let expected = DeliveryKey::derive(
            parts.target.obligation,
            parts.binding_generation,
            parts.revision,
        );
        if expected != parts.delivery_key {
            return Err(WakeKeyMismatch);
        }
        Ok(Self {
            delivery_id: parts.delivery_id,
            delivery_key: parts.delivery_key,
            target: parts.target,
            binding: parts.binding,
            binding_generation: parts.binding_generation,
            revision: parts.revision,
            delivery: parts.delivery,
        })
    }

    /// Deterministic idempotency key for this revision.
    #[must_use]
    pub const fn delivery_key(&self) -> DeliveryKey {
        self.delivery_key
    }

    /// Random correlation ID carried in the wake.
    ///
    /// Never returned by bootstrap or status.
    #[must_use]
    pub const fn delivery_id(&self) -> &DeliveryId {
        &self.delivery_id
    }

    /// Obligation snapshot this wake targets.
    #[must_use]
    pub const fn target(&self) -> &WakeTarget {
        &self.target
    }

    /// Binding record this wake belongs to.
    #[must_use]
    pub const fn binding(&self) -> ForemanBindingId {
        self.binding
    }

    /// Binding generation this wake belongs to.
    #[must_use]
    pub const fn binding_generation(&self) -> BindingGeneration {
        self.binding_generation
    }

    /// Revision number within the obligation and binding generation.
    #[must_use]
    pub const fn revision(&self) -> DeliveryRevision {
        self.revision
    }

    /// Aggregate delivery projection.
    #[must_use]
    pub const fn state(&self) -> DeliveryState {
        self.delivery.state()
    }

    /// The underlying attempt machine.
    #[must_use]
    pub const fn delivery(&self) -> &Delivery<AcceptedWakeEvidence> {
        &self.delivery
    }

    /// Reports whether a presented correlation ID matches this wake.
    #[must_use]
    pub fn correlates_with(&self, presented: &DeliveryId) -> bool {
        &self.delivery_id == presented
    }

    /// Verifies that the wake may still submit against `obligation`.
    ///
    /// Called immediately before composer mutation and again immediately before
    /// Send activation.
    ///
    /// # Errors
    ///
    /// Returns [`Conflict::StaleDeliveryTarget`] when the obligation moved on.
    pub fn require_current_target(&self, obligation: &Obligation) -> Result<(), Conflict> {
        if self.target.still_current(obligation) {
            Ok(())
        } else {
            Err(Conflict::StaleDeliveryTarget {
                revision: self.revision,
            })
        }
    }

    /// Applies a delivery event to this wake revision.
    ///
    /// # Errors
    ///
    /// Returns the [`Conflict`] from the underlying attempt machine.
    pub fn apply(&self, event: &DeliveryEvent<AcceptedWakeEvidence>) -> Outcome<Self> {
        match self.delivery.apply(event)? {
            Transition::Duplicate => Ok(Transition::Duplicate),
            Transition::Advanced(delivery) => {
                let mut next = self.clone();
                next.delivery = delivery;
                Ok(Transition::Advanced(next))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fence::SafeToken;
    use crate::id::ForemanBindingId;
    use crate::obligation::test_support as obligation_support;
    use crate::outbound::AttemptState;
    use crate::time::Timestamp;
    use uuid::Uuid;

    /// Deterministic counter "CSPRNG" for tests: never acceptable in a daemon,
    /// exactly right for proving the port is the only way in.
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

    fn obligation_id(n: u128) -> ObligationId {
        ObligationId::from_uuid(Uuid::from_u128(n))
    }

    fn binding_id() -> ForemanBindingId {
        ForemanBindingId::from_uuid(Uuid::from_u128(500))
    }

    fn wake(rng: &mut StreamRng, revision: DeliveryRevision) -> BrowserWake {
        let obligation = obligation_support::completed();
        BrowserWake::at_revision(
            rng,
            WakeTarget::snapshot(&obligation),
            binding_id(),
            BindingGeneration::FIRST,
            revision,
            3,
        )
    }

    #[test]
    fn delivery_key_is_deterministic() {
        let key_a = DeliveryKey::derive(
            obligation_id(1),
            BindingGeneration::FIRST,
            DeliveryRevision::FIRST,
        );
        let key_b = DeliveryKey::derive(
            obligation_id(1),
            BindingGeneration::FIRST,
            DeliveryRevision::FIRST,
        );
        assert_eq!(key_a, key_b);
        assert_eq!(key_a.to_hex().len(), 64);
    }

    #[test]
    fn delivery_key_distinguishes_every_input() {
        let base = DeliveryKey::derive(
            obligation_id(1),
            BindingGeneration::FIRST,
            DeliveryRevision::FIRST,
        );
        let others = [
            DeliveryKey::derive(
                obligation_id(2),
                BindingGeneration::FIRST,
                DeliveryRevision::FIRST,
            ),
            DeliveryKey::derive(
                obligation_id(1),
                BindingGeneration::new(2),
                DeliveryRevision::FIRST,
            ),
            DeliveryKey::derive(
                obligation_id(1),
                BindingGeneration::FIRST,
                DeliveryRevision::new(2),
            ),
        ];
        for other in others {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn delivery_id_is_drawn_only_from_the_port() {
        // Identical RNG streams give identical IDs even though the scheduling
        // metadata differs entirely: the metadata is not an input at all.
        let mut rng_a = StreamRng { next: 0 };
        let mut rng_b = StreamRng { next: 0 };
        let a = wake(&mut rng_a, DeliveryRevision::FIRST);
        let b = wake(&mut rng_b, DeliveryRevision::new(97));
        assert_eq!(a.delivery_id(), b.delivery_id());
        assert_ne!(
            a.delivery_key(),
            b.delivery_key(),
            "the deterministic key does depend on the metadata"
        );

        // Advancing the stream gives a different ID for identical metadata.
        let mut rng_c = StreamRng { next: 40 };
        let c = wake(&mut rng_c, DeliveryRevision::FIRST);
        assert_ne!(a.delivery_id(), c.delivery_id());
    }

    #[test]
    fn delivery_id_is_at_least_192_bits_and_redacted() {
        let mut rng = StreamRng { next: 3 };
        let id = DeliveryId::generate(&mut rng);
        const { assert!(DELIVERY_ID_BYTES * 8 >= 192) };
        assert_eq!(id.expose_bytes().len(), DELIVERY_ID_BYTES);
        assert_eq!(format!("{id:?}"), "DeliveryId(<redacted>)");
        assert!(!format!("{id:?}").contains(&id.expose_hex()[0..4]));
    }

    #[test]
    fn delivery_id_round_trips_through_persistence() {
        let mut rng = StreamRng { next: 17 };
        let id = DeliveryId::generate(&mut rng);
        assert_eq!(
            DeliveryId::parse_persisted(&id.expose_hex()),
            Ok(id.clone())
        );
        assert_eq!(
            DeliveryId::from_persisted_bytes(*id.expose_bytes()),
            id.clone()
        );
        assert_eq!(
            DeliveryId::parse_persisted("not-hex"),
            Err(MalformedDeliveryId)
        );
    }

    #[test]
    fn next_revision_mints_a_fresh_key_and_correlation_id() {
        let mut rng = StreamRng { next: 0 };
        let first = wake(&mut rng, DeliveryRevision::FIRST);
        let obligation = obligation_support::completed();
        let second = first.next_revision(
            &mut rng,
            WakeTarget::snapshot(&obligation),
            binding_id(),
            BindingGeneration::FIRST,
            3,
        );
        assert_eq!(second.revision(), DeliveryRevision::new(2));
        assert_ne!(first.delivery_key(), second.delivery_key());
        assert_ne!(first.delivery_id(), second.delivery_id());
        assert_eq!(
            first.state(),
            DeliveryState::Pending,
            "the old revision is untouched"
        );
    }

    #[test]
    fn weak_signals_cannot_construct_acceptance_evidence() {
        // The assertion here is that no `From`/`TryFrom` exists; the runtime
        // check just pins the discriminants so the list cannot silently shrink.
        let weak = [
            WeakBrowserSignal::ComposerEmptied,
            WeakBrowserSignal::UrlChanged,
            WeakBrowserSignal::StopButtonAppeared,
            WeakBrowserSignal::AssistantStarted,
            WeakBrowserSignal::WakeTextInDom,
        ];
        assert_eq!(weak.len(), 5);

        // Acceptance needs exact conversation *and* message identity.
        let evidence = AcceptedWakeEvidence::new(
            ConversationRef::new(SafeToken::new("conv-A").unwrap()),
            ProviderMessageRef::new(SafeToken::new("msg-1").unwrap()),
        );
        assert_eq!(evidence.conversation().as_token().as_str(), "conv-A");
    }

    #[test]
    fn stale_target_blocks_submission() {
        let mut rng = StreamRng { next: 0 };
        let obligation = obligation_support::completed();
        let wake = BrowserWake::create(
            &mut rng,
            WakeTarget::snapshot(&obligation),
            binding_id(),
            BindingGeneration::FIRST,
            3,
        );
        assert!(wake.require_current_target(&obligation).is_ok());

        let moved = obligation_support::cancelled(&obligation);
        let err = wake.require_current_target(&moved).unwrap_err();
        assert_eq!(err.code(), "stale_delivery_target");
    }

    #[test]
    fn rehydration_round_trips_a_persisted_wake() {
        let mut rng = StreamRng { next: 0 };
        let original = wake(&mut rng, DeliveryRevision::new(4));
        let restored = BrowserWake::rehydrate(PersistedWake {
            delivery_id: original.delivery_id().clone(),
            delivery_key: original.delivery_key(),
            target: original.target().clone(),
            binding: original.binding(),
            binding_generation: original.binding_generation(),
            revision: original.revision(),
            delivery: original.delivery().clone(),
        })
        .expect("a faithfully persisted wake re-derives its key");
        assert_eq!(restored, original);
    }

    #[test]
    fn rehydration_refuses_a_key_that_does_not_re_derive() {
        let mut rng = StreamRng { next: 0 };
        let original = wake(&mut rng, DeliveryRevision::FIRST);
        let err = BrowserWake::rehydrate(PersistedWake {
            delivery_id: original.delivery_id().clone(),
            delivery_key: original.delivery_key(),
            target: original.target().clone(),
            binding: binding_id(),
            binding_generation: BindingGeneration::FIRST,
            // The key was derived for revision one; claiming revision two must
            // not be accepted just because the caller says so.
            revision: DeliveryRevision::new(2),
            delivery: original.delivery().clone(),
        })
        .expect_err("a mismatched key fails closed");
        assert_eq!(err, WakeKeyMismatch);
    }

    #[test]
    fn wake_drives_the_shared_attempt_discipline() {
        let mut rng = StreamRng { next: 0 };
        let mut wake = wake(&mut rng, DeliveryRevision::FIRST);
        assert!(wake.delivery().io_permit().is_none());

        wake = wake
            .apply(&DeliveryEvent::AttemptClaimed {
                at: Timestamp::from_unix_millis(1),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert!(wake.delivery().io_permit().is_some());
        assert!(wake.delivery().send_activation().is_none());

        wake = wake
            .apply(&DeliveryEvent::ActivationArmed {
                attempt: crate::fence::AttemptNo::FIRST,
                at: Timestamp::from_unix_millis(2),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert!(wake.delivery().send_activation().is_some());
        assert_eq!(
            wake.delivery().attempts()[0].state(),
            AttemptState::ActivationArmed
        );
    }
}
