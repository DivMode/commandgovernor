//! The single active foreman binding and its monotonic generation.
//!
//! V1 has exactly one active binding ([`docs/adr/0004-foreman-mcp-and-binding.md`]).
//! Every foreman mutation presents a [`BindingGeneration`], and a rebind makes
//! every older generation permanently unable to touch current work. "Current
//! tab", "most recent conversation" and history are never authority.
//!
//! [`docs/adr/0004-foreman-mcp-and-binding.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/adr/0004-foreman-mcp-and-binding.md

use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{BindingGeneration, SafeToken};
use crate::id::ForemanBindingId;
use crate::time::Timestamp;

/// Opaque canonical conversation identity of the bound foreman surface.
///
/// This is the `/c/<id>` identity only. The route, the tab, and any session
/// material stay in the browser profile and never enter the control ledger.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationRef(SafeToken);

impl ConversationRef {
    /// Wraps a canonical conversation identity.
    #[must_use]
    pub const fn new(id: SafeToken) -> Self {
        Self(id)
    }

    /// Returns the opaque identity.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// Opaque identity of the dedicated browser profile.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowserProfileRef(SafeToken);

impl BrowserProfileRef {
    /// Wraps a profile identity.
    #[must_use]
    pub const fn new(id: SafeToken) -> Self {
        Self(id)
    }
}

/// The published connector ABI a binding was established against.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectorAbi(SafeToken);

impl ConnectorAbi {
    /// Wraps an ABI identifier such as `command-governor-foreman.v1`.
    #[must_use]
    pub const fn new(id: SafeToken) -> Self {
        Self(id)
    }
}

/// Feature-tested state of the bound surface's state-changing MCP capability.
///
/// This records what the account/workspace actually did, never what its plan
/// name suggests. Nothing here ever relaxes the ACK requirement: a surface that
/// cannot mutate simply keeps obligations open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WriteCapabilityState {
    /// Not yet feature-tested.
    Unknown,
    /// A synthetic mutation succeeded on the real surface.
    Proven,
    /// The surface is documented or observed read/fetch-only.
    ReadFetchOnlyUnsupported,
    /// Writes worked before and have since stopped working.
    Lost,
    /// Writes require a confirmation the model cannot legitimately complete.
    BlockedByConfirmation,
}

impl WriteCapabilityState {
    /// Reports whether a truthful `foreman_ack` is currently believed possible.
    #[must_use]
    pub const fn permits_mutation(self) -> bool {
        matches!(self, Self::Proven)
    }
}

/// One binding record: an exact conversation at an exact generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForemanBinding {
    id: ForemanBindingId,
    conversation: ConversationRef,
    profile: BrowserProfileRef,
    connector_abi: ConnectorAbi,
    generation: BindingGeneration,
    capability_epoch: u64,
    write_capability: WriteCapabilityState,
    bound_at: Timestamp,
}

impl ForemanBinding {
    /// Binding record identity.
    #[must_use]
    pub const fn id(&self) -> ForemanBindingId {
        self.id
    }

    /// Exact canonical conversation this binding points at.
    #[must_use]
    pub const fn conversation(&self) -> &ConversationRef {
        &self.conversation
    }

    /// Dedicated browser profile that holds the session.
    #[must_use]
    pub const fn profile(&self) -> &BrowserProfileRef {
        &self.profile
    }

    /// Connector ABI in force for this binding.
    #[must_use]
    pub const fn connector_abi(&self) -> &ConnectorAbi {
        &self.connector_abi
    }

    /// Monotonic generation of this binding.
    #[must_use]
    pub const fn generation(&self) -> BindingGeneration {
        self.generation
    }

    /// Capability epoch observed when the binding was established.
    #[must_use]
    pub const fn capability_epoch(&self) -> u64 {
        self.capability_epoch
    }

    /// Feature-tested write capability of the bound surface.
    #[must_use]
    pub const fn write_capability(&self) -> WriteCapabilityState {
        self.write_capability
    }

    /// Instant the binding was committed.
    #[must_use]
    pub const fn bound_at(&self) -> Timestamp {
        self.bound_at
    }
}

/// Everything needed to commit a new binding, already verified by the adapter.
///
/// The adapter proves the resolved conversation is exactly the requested one
/// *before* building this; the core only records and fences the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBindingTarget {
    /// Identity to record for the new binding row.
    pub id: ForemanBindingId,
    /// Exact resolved canonical conversation.
    pub conversation: ConversationRef,
    /// Dedicated browser profile.
    pub profile: BrowserProfileRef,
    /// Connector ABI proven present on the surface.
    pub connector_abi: ConnectorAbi,
    /// Capability epoch observed during verification.
    pub capability_epoch: u64,
    /// Feature-tested write capability.
    pub write_capability: WriteCapabilityState,
}

/// An event applied to the binding ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BindingEvent {
    /// A verified target was committed, superseding any older generation.
    Bound {
        /// The verified target.
        target: Box<VerifiedBindingTarget>,
        /// Observation instant.
        at: Timestamp,
    },
    /// A later capability observation for the active binding.
    CapabilityObserved {
        /// Generation the observation applies to.
        generation: BindingGeneration,
        /// Newly observed capability epoch.
        capability_epoch: u64,
        /// Newly observed write capability.
        write_capability: WriteCapabilityState,
    },
    /// The bound surface was displaced (logged out, deleted, wrong route).
    ///
    /// The binding is deactivated; obligations stay open and nothing closes.
    Displaced {
        /// Generation that was displaced.
        generation: BindingGeneration,
        /// Observation instant.
        at: Timestamp,
    },
}

/// The binding ledger: at most one active binding, ever-increasing generations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingLedger {
    active: Option<ForemanBinding>,
    highest_generation: Option<BindingGeneration>,
}

impl BindingLedger {
    /// Creates an unbound ledger.
    #[must_use]
    pub fn unbound() -> Self {
        Self::default()
    }

    /// The active binding, if any.
    #[must_use]
    pub const fn active(&self) -> Option<&ForemanBinding> {
        self.active.as_ref()
    }

    /// The highest generation ever issued, active or superseded.
    #[must_use]
    pub const fn highest_generation(&self) -> Option<BindingGeneration> {
        self.highest_generation
    }

    /// Verifies that `presented` is the active binding generation.
    ///
    /// This is the fence every foreman mutation crosses first.
    ///
    /// # Errors
    ///
    /// - [`Conflict::NoActiveBinding`] when nothing is bound;
    /// - [`Conflict::StaleBindingGeneration`] for a superseded generation;
    /// - [`Conflict::UnknownBindingGeneration`] for one that was never issued.
    pub fn fence(&self, presented: BindingGeneration) -> Result<&ForemanBinding, Conflict> {
        let active = self.active.as_ref().ok_or(Conflict::NoActiveBinding)?;
        match presented.get().cmp(&active.generation.get()) {
            core::cmp::Ordering::Equal => Ok(active),
            core::cmp::Ordering::Less => Err(Conflict::StaleBindingGeneration {
                presented,
                active: active.generation,
            }),
            core::cmp::Ordering::Greater => Err(Conflict::UnknownBindingGeneration {
                presented,
                active: active.generation,
            }),
        }
    }

    /// Applies an event, returning a new ledger or a typed conflict.
    ///
    /// # Errors
    ///
    /// Returns the [`Conflict`] describing why the event cannot apply.
    pub fn apply(&self, event: &BindingEvent) -> Outcome<Self> {
        match event {
            BindingEvent::Bound { target, at } => {
                let generation = self
                    .highest_generation
                    .map_or(BindingGeneration::FIRST, BindingGeneration::next);
                let binding = ForemanBinding {
                    id: target.id,
                    conversation: target.conversation.clone(),
                    profile: target.profile.clone(),
                    connector_abi: target.connector_abi.clone(),
                    generation,
                    capability_epoch: target.capability_epoch,
                    write_capability: target.write_capability,
                    bound_at: *at,
                };
                Ok(Transition::Advanced(Self {
                    active: Some(binding),
                    highest_generation: Some(generation),
                }))
            }
            BindingEvent::CapabilityObserved {
                generation,
                capability_epoch,
                write_capability,
            } => {
                let active = self.fence(*generation)?;
                if active.capability_epoch == *capability_epoch
                    && active.write_capability == *write_capability
                {
                    return Ok(Transition::Duplicate);
                }
                let mut next = self.clone();
                let binding = next
                    .active
                    .as_mut()
                    .expect("fence returned an active binding");
                binding.capability_epoch = *capability_epoch;
                binding.write_capability = *write_capability;
                Ok(Transition::Advanced(next))
            }
            BindingEvent::Displaced { generation, at } => {
                let _ = at;
                self.fence(*generation)?;
                Ok(Transition::Advanced(Self {
                    active: None,
                    highest_generation: self.highest_generation,
                }))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{
        BindingEvent, BindingLedger, BrowserProfileRef, ConnectorAbi, ConversationRef,
        VerifiedBindingTarget, WriteCapabilityState,
    };
    use crate::fence::SafeToken;
    use crate::id::ForemanBindingId;
    use crate::time::Timestamp;
    use uuid::Uuid;

    pub(crate) fn token(value: &str) -> SafeToken {
        SafeToken::new(value).expect("test token is safe")
    }

    pub(crate) fn target(conversation: &str, nth: u128) -> VerifiedBindingTarget {
        VerifiedBindingTarget {
            id: ForemanBindingId::from_uuid(Uuid::from_u128(nth)),
            conversation: ConversationRef::new(token(conversation)),
            profile: BrowserProfileRef::new(token("cg-profile")),
            connector_abi: ConnectorAbi::new(token("command-governor-foreman.v1")),
            capability_epoch: 1,
            write_capability: WriteCapabilityState::Proven,
        }
    }

    /// A ledger bound once to `conversation`, at generation 1.
    pub(crate) fn bound(conversation: &str) -> BindingLedger {
        BindingLedger::unbound()
            .apply(&BindingEvent::Bound {
                target: Box::new(target(conversation, 1)),
                at: Timestamp::from_unix_millis(0),
            })
            .expect("first bind is legal")
            .advanced()
            .expect("bind advances")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{bound, target};
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    #[test]
    fn unbound_ledger_fences_everything() {
        let ledger = BindingLedger::unbound();
        assert!(ledger.active().is_none());
        let err = ledger.fence(BindingGeneration::FIRST).unwrap_err();
        assert_eq!(err.code(), "no_active_binding");
    }

    #[test]
    fn first_bind_issues_generation_one() {
        let ledger = bound("conv-A");
        let active = ledger.active().expect("bound");
        assert_eq!(active.generation(), BindingGeneration::FIRST);
        assert!(ledger.fence(BindingGeneration::FIRST).is_ok());
    }

    #[test]
    fn rebind_supersedes_and_stale_generation_is_rejected() {
        let first = bound("conv-A");
        let second = first
            .apply(&BindingEvent::Bound {
                target: Box::new(target("conv-B", 2)),
                at: at(10),
            })
            .expect("rebind is legal")
            .advanced()
            .expect("rebind advances");

        let active = second.active().expect("bound");
        assert_eq!(active.generation(), BindingGeneration::new(2));
        assert_eq!(active.conversation().as_token().as_str(), "conv-B");

        let err = second.fence(BindingGeneration::FIRST).unwrap_err();
        assert_eq!(err.code(), "stale_binding_generation");

        // The old ledger value is untouched: a conflict mutates nothing.
        assert_eq!(
            first.active().expect("bound").generation(),
            BindingGeneration::FIRST
        );
    }

    #[test]
    fn generation_never_reused_after_displacement() {
        let ledger = bound("conv-A");
        let displaced = ledger
            .apply(&BindingEvent::Displaced {
                generation: BindingGeneration::FIRST,
                at: at(5),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert!(displaced.active().is_none());

        let rebound = displaced
            .apply(&BindingEvent::Bound {
                target: Box::new(target("conv-A", 3)),
                at: at(6),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(
            rebound.active().expect("bound").generation(),
            BindingGeneration::new(2),
            "a displaced generation is never reissued"
        );
    }

    #[test]
    fn future_generation_is_unknown_not_stale() {
        let ledger = bound("conv-A");
        let err = ledger.fence(BindingGeneration::new(99)).unwrap_err();
        assert_eq!(err.code(), "unknown_binding_generation");
    }

    #[test]
    fn capability_loss_is_recorded_without_closing_anything() {
        let ledger = bound("conv-A");
        let lost = ledger
            .apply(&BindingEvent::CapabilityObserved {
                generation: BindingGeneration::FIRST,
                capability_epoch: 2,
                write_capability: WriteCapabilityState::Lost,
            })
            .unwrap()
            .advanced()
            .unwrap();
        let active = lost.active().expect("still bound");
        assert!(!active.write_capability().permits_mutation());
        assert_eq!(active.generation(), BindingGeneration::FIRST);
    }

    #[test]
    fn repeated_identical_capability_observation_is_idempotent() {
        let ledger = bound("conv-A");
        let repeat = ledger
            .apply(&BindingEvent::CapabilityObserved {
                generation: BindingGeneration::FIRST,
                capability_epoch: 1,
                write_capability: WriteCapabilityState::Proven,
            })
            .unwrap();
        assert!(repeat.is_duplicate());
    }

    #[test]
    fn stale_generation_cannot_record_capability() {
        let second = bound("conv-A")
            .apply(&BindingEvent::Bound {
                target: Box::new(target("conv-B", 2)),
                at: at(10),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let err = second
            .apply(&BindingEvent::CapabilityObserved {
                generation: BindingGeneration::FIRST,
                capability_epoch: 9,
                write_capability: WriteCapabilityState::ReadFetchOnlyUnsupported,
            })
            .unwrap_err();
        assert_eq!(err.code(), "stale_binding_generation");
        assert_eq!(
            second.active().expect("bound").capability_epoch(),
            1,
            "zero mutation"
        );
    }
}
