//! Opaque typed domain identities.
//!
//! Every identity in the durable model is a distinct Rust type. Two identities
//! never unify by accident, an obligation ID cannot be passed where a claim ID
//! is expected, and — per [`docs/data-model.md`], *correctness never depends on
//! parsing an ID*. The inner [`Uuid`] is an opaque 128-bit value: this crate
//! reads no field of it, derives no meaning from its version or timestamp bits,
//! and compares identities only for equality and ordering.
//!
//! Generation is a *port*, never ambient. [`IdSource`] is the only way to mint a
//! new identity, so this crate never touches a clock or an entropy source; a
//! UUIDv7 implementation lives in an outer crate that is allowed to.
//!
//! [`docs/data-model.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/data-model.md

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

use uuid::Uuid;

/// Marker trait implemented by the zero-sized tag of each identity family.
///
/// The tag exists only to keep [`Id`] values of different families from
/// unifying. `LABEL` is used for diagnostics and never for correctness.
pub trait IdKind: Copy + Clone + fmt::Debug + Eq + Ord + Hash + 'static {
    /// Human-readable family name, used only in `Debug` output.
    const LABEL: &'static str;
}

/// An opaque, family-tagged domain identity.
///
/// The type parameter is a phantom tag; the runtime representation is exactly
/// one [`Uuid`]. Nothing in this crate inspects the bytes.
pub struct Id<K: IdKind> {
    value: Uuid,
    kind: PhantomData<fn() -> K>,
}

impl<K: IdKind> Id<K> {
    /// Wraps an already-generated opaque value.
    ///
    /// Callers obtain the value from an [`IdSource`] or from durable storage;
    /// this crate never invents one.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self {
            value,
            kind: PhantomData,
        }
    }

    /// Returns the opaque value, for persistence and transport only.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.value
    }

    /// Rehydrates an identity from its canonical persisted text form.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError`] when the text is not a canonical UUID. A store
    /// row that fails here is corrupt and must fail closed rather than be
    /// coerced into some other identity.
    pub fn parse(text: &str) -> Result<Self, IdParseError> {
        Uuid::parse_str(text)
            .map(Self::from_uuid)
            .map_err(|_| IdParseError {
                family: K::LABEL,
                len: text.len(),
            })
    }
}

/// A persisted identity could not be rehydrated.
///
/// Deliberately carries no copy of the offending text: a malformed value from
/// an untrusted surface must not be echoed into a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed {family} identity of {len} bytes")]
pub struct IdParseError {
    /// Identity family that was expected.
    pub family: &'static str,
    /// Length of the rejected text, retained as a bounded diagnostic.
    pub len: usize,
}

// Manual trait impls: deriving them would add a spurious `K: Clone` style bound
// on the phantom tag, which the tag types have no reason to satisfy.
impl<K: IdKind> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: IdKind> Copy for Id<K> {}

impl<K: IdKind> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<K: IdKind> Eq for Id<K> {}

impl<K: IdKind> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: IdKind> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

impl<K: IdKind> Hash for Id<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<K: IdKind> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", K::LABEL, self.value)
    }
}

impl<K: IdKind> fmt::Display for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.value, f)
    }
}

/// Declares an identity family: a zero-sized tag plus its public alias.
macro_rules! id_families {
    ($( $(#[$doc:meta])* $tag:ident => $alias:ident ),* $(,)?) => {
        /// Zero-sized tags that distinguish identity families at compile time.
        pub mod kind {
            $(
                #[doc = concat!("Tag for [`", stringify!($alias), "`](super::", stringify!($alias), ").")]
                #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
                pub struct $tag;

                impl super::IdKind for $tag {
                    const LABEL: &'static str = stringify!($alias);
                }
            )*
        }

        $(
            $(#[$doc])*
            pub type $alias = Id<kind::$tag>;
        )*
    };
}

id_families! {
    /// A source-host project reference (provenance, not repository content).
    Project => ProjectId,
    /// A unit of delegated engineering work.
    Task => TaskId,
    /// A logical worker session. Its display name is never an identity fence.
    Session => SessionId,
    /// One concrete incarnation of a session; replacement starts a new one.
    SessionIncarnation => SessionIncarnationId,
    /// One worker turn within a session incarnation.
    Turn => TurnId,
    /// An immutable domain event appended to the ledger.
    Event => EventId,
    /// The bounded, immutable final worker result required for review.
    ResultArtifact => ResultArtifactId,
    /// A durable obligation: work still owed to the foreman.
    Obligation => ObligationId,
    /// A durable request for input owed by the foreman or the user.
    InputRequest => InputRequestId,
    /// One foreman binding record (one canonical conversation, one generation).
    ForemanBinding => ForemanBindingId,
    /// A bounded foreman claim over an obligation.
    Claim => ClaimId,
    /// One physical assistant turn on the bound foreman surface.
    ForemanTurn => ForemanTurnId,
    /// One attempt at a browser wake delivery.
    DeliveryAttempt => DeliveryAttemptId,
    /// A continuation (answer/resume) owed to a worker.
    WorkerCommand => WorkerCommandId,
    /// One attempt at delivering a worker command.
    WorkerCommandAttempt => WorkerCommandAttemptId,
    /// An open or resolved health/reconciliation condition.
    HealthCondition => HealthConditionId,
    /// A recorded verified-progress heartbeat.
    Progress => ProgressId,
    /// A principal that issues mutations: a daemon, a CLI, an MCP connector.
    ///
    /// Semantic identity, deliberately not a process or a connection: a
    /// transport reconnect keeps the same actor, which is what makes
    /// `MutationCommandId` a stable retry identity in [`crate::mutation`].
    Actor => ActorId,
    /// One logical daemon/IPC/MCP write, stable across transport reconnect.
    MutationCommand => MutationCommandId,
    /// One attempt at one consequential external effect.
    ExternalAttempt => ExternalAttemptId,
    /// One lease over an exclusively-owned resource.
    ResourceLease => ResourceLeaseId,
    /// One immutable resolved worker launch/resume profile.
    WorkerLoadout => WorkerLoadoutId,
    /// One immutable whitelist of worker capabilities.
    CapabilityProfile => CapabilityProfileId,
    /// One immutable recursive-delegation whitelist.
    DelegationPolicy => DelegationPolicyId,
    /// One immutable model-selection/policy snapshot.
    ModelPolicy => ModelPolicyId,
    /// One private immutable managed worker-configuration artifact.
    ManagedConfigArtifact => ManagedConfigArtifactId,
}

/// Injectable source of opaque identities.
///
/// Minting a UUIDv7 needs a clock and entropy, and `governor-core` has neither.
/// The daemon supplies a real implementation; tests supply a deterministic one,
/// which is what makes every state machine in this crate replayable.
pub trait IdSource {
    /// Produces the next opaque identity value.
    fn next_uuid(&mut self) -> Uuid;

    /// Produces the next identity of a specific family.
    fn next_id<K: IdKind>(&mut self) -> Id<K>
    where
        Self: Sized,
    {
        Id::from_uuid(self.next_uuid())
    }
}

impl<T: IdSource + ?Sized> IdSource for &mut T {
    fn next_uuid(&mut self) -> Uuid {
        (**self).next_uuid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_of_different_families_are_distinct_types() {
        // This is the compile-time property under test; the runtime assertion
        // only pins the shared representation.
        let raw = Uuid::from_u128(7);
        let obligation = ObligationId::from_uuid(raw);
        let claim = ClaimId::from_uuid(raw);
        assert_eq!(obligation.as_uuid(), claim.as_uuid());
        // `obligation == claim` does not compile: different types.
    }

    #[test]
    fn debug_names_the_family_and_display_is_canonical() {
        let id = ObligationId::from_uuid(Uuid::from_u128(1));
        assert_eq!(
            format!("{id:?}"),
            "ObligationId(00000000-0000-0000-0000-000000000001)"
        );
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn round_trips_through_persisted_text() {
        let id = TaskId::from_uuid(Uuid::from_u128(0x1234_5678));
        assert_eq!(TaskId::parse(&id.to_string()), Ok(id));
    }

    #[test]
    fn malformed_text_fails_closed_without_echoing_it() {
        let err = TaskId::parse("../../etc/passwd").unwrap_err();
        assert_eq!(err.family, "TaskId");
        assert!(!err.to_string().contains("passwd"));
    }

    #[test]
    fn id_source_is_the_only_mint() {
        struct Counter(u128);
        impl IdSource for Counter {
            fn next_uuid(&mut self) -> Uuid {
                self.0 += 1;
                Uuid::from_u128(self.0)
            }
        }

        let mut ids = Counter(0);
        let a: ObligationId = ids.next_id();
        let b: ObligationId = ids.next_id();
        assert_ne!(a, b);
    }
}
