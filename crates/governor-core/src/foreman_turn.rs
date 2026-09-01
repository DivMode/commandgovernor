//! The physical ChatGPT assistant turn.
//!
//! This machine exists to be *separate* from everything else. Per
//! [`docs/architecture.md`] "ChatGPT settlement is not ACK", browser delivery
//! accepted, physical turn settled, and foreman ACK are three different facts.
//! Nothing in this module can close an obligation, and there is deliberately no
//! function here that returns one — the type system carries invariant 14.
//!
//! Its one operational job is [`ForemanTurnState::permits_new_wake`]: no wake
//! may activate while the bound surface is mid-turn or unobserved.
//!
//! [`docs/architecture.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/architecture.md

use crate::delivery::DeliveryId;
use crate::error::{Conflict, Outcome, Transition};
use crate::fence::{BindingGeneration, SafeToken};
use crate::id::ForemanTurnId;
use crate::time::Timestamp;

/// Observed lifecycle of one physical assistant turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForemanTurnState {
    /// Nothing is running, or nothing has been observed yet.
    IdleUnknown,
    /// A turn has been triggered but has not produced assistant output.
    Starting,
    /// The assistant is producing a response.
    Active,
    /// The physical turn appears finished. This means nothing about MCP.
    Settled,
    /// The observation channel was lost mid-turn; the real state is unknown.
    ObservationLost,
}

impl ForemanTurnState {
    /// Reports whether a new wake may be activated against this surface.
    ///
    /// `ObservationLost` deliberately blocks: an unobserved surface may still
    /// be mid-turn, and a second wake on top of it is exactly the duplicate the
    /// delivery discipline exists to prevent.
    #[must_use]
    pub const fn permits_new_wake(self) -> bool {
        matches!(self, Self::IdleUnknown | Self::Settled)
    }
}

/// Opaque provider-native reference to a message in the bound conversation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderMessageRef(SafeToken);

impl ProviderMessageRef {
    /// Wraps an opaque provider message identity.
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

/// Projection of one observed physical turn on the bound surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForemanTurn {
    id: ForemanTurnId,
    binding_generation: BindingGeneration,
    trigger: Option<DeliveryId>,
    state: ForemanTurnState,
    started_at: Option<Timestamp>,
    settled_at: Option<Timestamp>,
}

impl ForemanTurn {
    /// Creates an unobserved turn projection for a binding generation.
    #[must_use]
    pub const fn unobserved(id: ForemanTurnId, binding_generation: BindingGeneration) -> Self {
        Self {
            id,
            binding_generation,
            trigger: None,
            state: ForemanTurnState::IdleUnknown,
            started_at: None,
            settled_at: None,
        }
    }

    /// Turn projection identity.
    #[must_use]
    pub const fn id(&self) -> ForemanTurnId {
        self.id
    }

    /// Binding generation this observation belongs to.
    #[must_use]
    pub const fn binding_generation(&self) -> BindingGeneration {
        self.binding_generation
    }

    /// Current observed state.
    #[must_use]
    pub const fn state(&self) -> ForemanTurnState {
        self.state
    }

    /// Instant the turn was first seen starting.
    #[must_use]
    pub const fn started_at(&self) -> Option<Timestamp> {
        self.started_at
    }

    /// Instant the turn was seen to settle.
    #[must_use]
    pub const fn settled_at(&self) -> Option<Timestamp> {
        self.settled_at
    }

    /// Reports whether a new wake may be activated right now.
    #[must_use]
    pub const fn permits_new_wake(&self) -> bool {
        self.state.permits_new_wake()
    }

    /// Returns the surface-busy conflict when a wake must not activate.
    ///
    /// # Errors
    ///
    /// Returns [`Conflict::ForemanTurnNotQuiescent`] while the surface is
    /// `Starting`, `Active`, or `ObservationLost`.
    pub const fn require_quiescent(&self) -> Result<(), Conflict> {
        if self.state.permits_new_wake() {
            Ok(())
        } else {
            Err(Conflict::ForemanTurnNotQuiescent { state: self.state })
        }
    }

    /// Applies an observation, returning a new projection.
    ///
    /// # Errors
    ///
    /// Returns [`Conflict::StaleBindingGeneration`] for an observation from a
    /// superseded generation.
    pub fn apply(&self, event: &ForemanTurnEvent) -> Outcome<Self> {
        if event.binding_generation() != self.binding_generation {
            return Err(Conflict::StaleBindingGeneration {
                presented: event.binding_generation(),
                active: self.binding_generation,
            });
        }
        let mut next = self.clone();
        match event {
            ForemanTurnEvent::Started {
                trigger, at: when, ..
            } => {
                if self.state == ForemanTurnState::Starting {
                    return Ok(Transition::Duplicate);
                }
                next.state = ForemanTurnState::Starting;
                next.trigger.clone_from(trigger);
                next.started_at = Some(*when);
            }
            ForemanTurnEvent::BecameActive { .. } => {
                if self.state == ForemanTurnState::Active {
                    return Ok(Transition::Duplicate);
                }
                next.state = ForemanTurnState::Active;
            }
            ForemanTurnEvent::Settled { at: when, .. } => {
                if self.state == ForemanTurnState::Settled {
                    return Ok(Transition::Duplicate);
                }
                next.state = ForemanTurnState::Settled;
                next.settled_at = Some(*when);
            }
            ForemanTurnEvent::ObservationLost { .. } => {
                if self.state == ForemanTurnState::ObservationLost {
                    return Ok(Transition::Duplicate);
                }
                next.state = ForemanTurnState::ObservationLost;
            }
        }
        Ok(Transition::Advanced(next))
    }
}

/// An observation of the bound surface's physical turn.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForemanTurnEvent {
    /// A turn began, optionally correlated with the wake that triggered it.
    Started {
        /// Generation the observation belongs to.
        binding_generation: BindingGeneration,
        /// Wake that is believed to have triggered the turn.
        trigger: Option<DeliveryId>,
        /// Observation instant.
        at: Timestamp,
    },
    /// The assistant started producing output.
    BecameActive {
        /// Generation the observation belongs to.
        binding_generation: BindingGeneration,
        /// Observation instant.
        at: Timestamp,
    },
    /// The physical turn appears finished. Not ACK, not processing.
    Settled {
        /// Generation the observation belongs to.
        binding_generation: BindingGeneration,
        /// Observation instant.
        at: Timestamp,
    },
    /// Observation of the surface was lost.
    ObservationLost {
        /// Generation the observation belongs to.
        binding_generation: BindingGeneration,
        /// Observation instant.
        at: Timestamp,
    },
}

impl ForemanTurnEvent {
    const fn binding_generation(&self) -> BindingGeneration {
        match self {
            Self::Started {
                binding_generation, ..
            }
            | Self::BecameActive {
                binding_generation, ..
            }
            | Self::Settled {
                binding_generation, ..
            }
            | Self::ObservationLost {
                binding_generation, ..
            } => *binding_generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ForemanTurnId;
    use uuid::Uuid;

    fn turn() -> ForemanTurn {
        ForemanTurn::unobserved(
            ForemanTurnId::from_uuid(Uuid::from_u128(1)),
            BindingGeneration::FIRST,
        )
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    #[test]
    fn full_lifecycle_reaches_settled() {
        let started = turn()
            .apply(&ForemanTurnEvent::Started {
                binding_generation: BindingGeneration::FIRST,
                trigger: None,
                at: at(1),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(started.state(), ForemanTurnState::Starting);

        let active = started
            .apply(&ForemanTurnEvent::BecameActive {
                binding_generation: BindingGeneration::FIRST,
                at: at(2),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(active.state(), ForemanTurnState::Active);

        let settled = active
            .apply(&ForemanTurnEvent::Settled {
                binding_generation: BindingGeneration::FIRST,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(settled.state(), ForemanTurnState::Settled);
        assert_eq!(settled.settled_at(), Some(at(3)));
    }

    #[test]
    fn no_wake_while_active_or_unobserved() {
        for state in [
            ForemanTurnState::Starting,
            ForemanTurnState::Active,
            ForemanTurnState::ObservationLost,
        ] {
            assert!(!state.permits_new_wake(), "{state:?} must block a wake");
        }
        for state in [ForemanTurnState::IdleUnknown, ForemanTurnState::Settled] {
            assert!(state.permits_new_wake(), "{state:?} may accept a wake");
        }
    }

    #[test]
    fn require_quiescent_reports_a_typed_conflict() {
        let active = turn()
            .apply(&ForemanTurnEvent::BecameActive {
                binding_generation: BindingGeneration::FIRST,
                at: at(2),
            })
            .unwrap()
            .advanced()
            .unwrap();
        let err = active.require_quiescent().unwrap_err();
        assert_eq!(err.code(), "foreman_turn_not_quiescent");
    }

    #[test]
    fn observation_from_a_stale_generation_is_rejected() {
        let current = ForemanTurn::unobserved(
            ForemanTurnId::from_uuid(Uuid::from_u128(2)),
            BindingGeneration::new(2),
        );
        let err = current
            .apply(&ForemanTurnEvent::Settled {
                binding_generation: BindingGeneration::FIRST,
                at: at(9),
            })
            .unwrap_err();
        assert_eq!(err.code(), "stale_binding_generation");
        assert_eq!(current.state(), ForemanTurnState::IdleUnknown);
    }

    #[test]
    fn repeated_settlement_is_idempotent() {
        let settled = turn()
            .apply(&ForemanTurnEvent::Settled {
                binding_generation: BindingGeneration::FIRST,
                at: at(3),
            })
            .unwrap()
            .advanced()
            .unwrap();
        assert!(
            settled
                .apply(&ForemanTurnEvent::Settled {
                    binding_generation: BindingGeneration::FIRST,
                    at: at(4),
                })
                .unwrap()
                .is_duplicate()
        );
    }
}
