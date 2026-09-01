//! Health and reconciliation conditions.
//!
//! A health condition is **attention**, never terminal worker state. Per
//! [`docs/data-model.md`], "a health condition never pretends to be worker
//! completion": nothing in this module can close an obligation, release an
//! artifact, or move a turn, and the ledger has no API that would let it.
//!
//! [`docs/data-model.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/data-model.md

use crate::error::{Outcome, Transition};
use crate::id::{ExternalAttemptId, HealthConditionId, ObligationId, TaskId, TurnId};
use crate::time::Timestamp;

/// The initial condition kinds from the data model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum HealthConditionKind {
    /// No verified progress beyond the watchdog threshold.
    SuspectedStall,
    /// The automatic wake budget is spent and the foreman cannot be reached.
    ForemanUnreachable,
    /// The bound surface cannot perform state-changing MCP operations.
    McpWriteCapabilityMissing,
    /// The bound browser surface was displaced, logged out, or deleted.
    BrowserBindingDisplaced,
    /// An artifact required by an open obligation is missing or corrupt.
    ResultArtifactMissing,
    /// Replayed projections disagree with committed state.
    ProjectionMismatch,
    /// Runtime transport disagrees with confirmed worker evidence.
    RuntimeStateConflict,
    /// A deferred question's detail cannot be recovered from the provider.
    InputDetailUnavailable,
    /// A defer shape the provider cannot durably pause, such as multi-tool.
    WorkerDeferShapeUnsupported,
    /// A consequential external effect has an unknown fate.
    ///
    /// Raised for an [`crate::effect::ExternalAttempt`] whose intent is durable
    /// but whose outcome was never proven — the crash window
    /// [`crate::effect::ExternalAttemptState::Ambiguous`] describes. It is
    /// attention, exactly like every other kind here: it authorises no replay,
    /// and resolving it is an explicit human or reconciliation decision. Its
    /// scope carries [`HealthScope::external_attempt`] so the resolver can find
    /// the exact recorded attempt, class, destination and idempotency key.
    ReconciliationRequired,
}

impl HealthConditionKind {
    /// Returns the stable `snake_case` code used in storage and diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SuspectedStall => "suspected_stall",
            Self::ForemanUnreachable => "foreman_unreachable",
            Self::McpWriteCapabilityMissing => "mcp_write_capability_missing",
            Self::BrowserBindingDisplaced => "browser_binding_displaced",
            Self::ResultArtifactMissing => "result_artifact_missing",
            Self::ProjectionMismatch => "projection_mismatch",
            Self::RuntimeStateConflict => "runtime_state_conflict",
            Self::InputDetailUnavailable => "input_detail_unavailable",
            Self::WorkerDeferShapeUnsupported => "worker_defer_shape_unsupported",
            Self::ReconciliationRequired => "reconciliation_required",
        }
    }
}

/// Whether a condition is currently demanding attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HealthConditionState {
    /// The condition is outstanding.
    Open,
    /// The condition was resolved by later verified evidence.
    Resolved,
}

/// What a condition is about. All fields are optional and opaque.
///
/// Scope is part of a condition's identity: [`HealthLedger::raise`] deduplicates
/// on `(kind, scope)`, so two ambiguous external attempts raise two conditions
/// rather than collapsing into one. That is why
/// [`HealthConditionKind::ReconciliationRequired`] needed its own scope field
/// instead of borrowing the obligation one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HealthScope {
    /// Task the condition concerns, if any.
    pub task: Option<TaskId>,
    /// Turn the condition concerns, if any.
    pub turn: Option<TurnId>,
    /// Obligation the condition concerns, if any.
    pub obligation: Option<ObligationId>,
    /// Consequential external attempt the condition concerns, if any.
    pub external_attempt: Option<ExternalAttemptId>,
}

impl HealthScope {
    /// Scopes a condition to nothing in particular.
    #[must_use]
    pub const fn global() -> Self {
        Self {
            task: None,
            turn: None,
            obligation: None,
            external_attempt: None,
        }
    }

    /// Scopes a condition to one obligation.
    #[must_use]
    pub const fn obligation(id: ObligationId) -> Self {
        Self {
            obligation: Some(id),
            ..Self::global()
        }
    }

    /// Scopes a condition to one turn.
    #[must_use]
    pub const fn turn(id: TurnId) -> Self {
        Self {
            turn: Some(id),
            ..Self::global()
        }
    }

    /// Scopes a condition to one consequential external attempt.
    #[must_use]
    pub const fn external_attempt(id: ExternalAttemptId) -> Self {
        Self {
            external_attempt: Some(id),
            ..Self::global()
        }
    }
}

/// One open or resolved condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCondition {
    id: HealthConditionId,
    kind: HealthConditionKind,
    scope: HealthScope,
    state: HealthConditionState,
    opened_at: Timestamp,
    resolved_at: Option<Timestamp>,
}

impl HealthCondition {
    /// Condition identity.
    #[must_use]
    pub const fn id(&self) -> HealthConditionId {
        self.id
    }

    /// Condition kind.
    #[must_use]
    pub const fn kind(&self) -> HealthConditionKind {
        self.kind
    }

    /// What the condition is about.
    #[must_use]
    pub const fn scope(&self) -> HealthScope {
        self.scope
    }

    /// Whether the condition is still outstanding.
    #[must_use]
    pub const fn state(&self) -> HealthConditionState {
        self.state
    }

    /// When the condition was raised.
    #[must_use]
    pub const fn opened_at(&self) -> Timestamp {
        self.opened_at
    }

    /// When the condition was resolved, if it was.
    #[must_use]
    pub const fn resolved_at(&self) -> Option<Timestamp> {
        self.resolved_at
    }
}

/// The set of conditions raised so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthLedger {
    conditions: Vec<HealthCondition>,
}

impl HealthLedger {
    /// Creates an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// All recorded conditions, oldest first.
    #[must_use]
    pub fn conditions(&self) -> &[HealthCondition] {
        &self.conditions
    }

    /// Reports whether a matching condition is currently open.
    #[must_use]
    pub fn is_open(&self, kind: HealthConditionKind, scope: HealthScope) -> bool {
        self.find_open(kind, scope).is_some()
    }

    /// Every currently open condition.
    pub fn open(&self) -> impl Iterator<Item = &HealthCondition> {
        self.conditions
            .iter()
            .filter(|condition| condition.state == HealthConditionState::Open)
    }

    fn find_open(&self, kind: HealthConditionKind, scope: HealthScope) -> Option<usize> {
        self.conditions.iter().position(|condition| {
            condition.state == HealthConditionState::Open
                && condition.kind == kind
                && condition.scope == scope
        })
    }

    /// Raises a condition, or reports a duplicate if one is already open.
    ///
    /// # Errors
    ///
    /// Infallible today; the signature matches the other machines so callers
    /// can treat every transition uniformly.
    pub fn raise(
        &self,
        id: HealthConditionId,
        kind: HealthConditionKind,
        scope: HealthScope,
        at: Timestamp,
    ) -> Outcome<Self> {
        if self.find_open(kind, scope).is_some() {
            return Ok(Transition::Duplicate);
        }
        let mut next = self.clone();
        next.conditions.push(HealthCondition {
            id,
            kind,
            scope,
            state: HealthConditionState::Open,
            opened_at: at,
            resolved_at: None,
        });
        Ok(Transition::Advanced(next))
    }

    /// Resolves an open condition, or reports a duplicate if none is open.
    ///
    /// # Errors
    ///
    /// Infallible today, for the same reason as [`HealthLedger::raise`].
    pub fn resolve(
        &self,
        kind: HealthConditionKind,
        scope: HealthScope,
        at: Timestamp,
    ) -> Outcome<Self> {
        let Some(index) = self.find_open(kind, scope) else {
            return Ok(Transition::Duplicate);
        };
        let mut next = self.clone();
        next.conditions[index].state = HealthConditionState::Resolved;
        next.conditions[index].resolved_at = Some(at);
        Ok(Transition::Advanced(next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn condition_id(n: u128) -> HealthConditionId {
        HealthConditionId::from_uuid(Uuid::from_u128(n))
    }

    fn obligation_scope() -> HealthScope {
        HealthScope::obligation(ObligationId::from_uuid(Uuid::from_u128(1)))
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    #[test]
    fn every_documented_kind_has_a_stable_code() {
        let kinds = [
            HealthConditionKind::SuspectedStall,
            HealthConditionKind::ForemanUnreachable,
            HealthConditionKind::McpWriteCapabilityMissing,
            HealthConditionKind::BrowserBindingDisplaced,
            HealthConditionKind::ResultArtifactMissing,
            HealthConditionKind::ProjectionMismatch,
            HealthConditionKind::RuntimeStateConflict,
            HealthConditionKind::InputDetailUnavailable,
            HealthConditionKind::WorkerDeferShapeUnsupported,
            HealthConditionKind::ReconciliationRequired,
        ];
        let mut codes: Vec<&str> = kinds.iter().map(|kind| kind.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), kinds.len(), "codes must be unique");
    }

    #[test]
    fn each_ambiguous_attempt_gets_its_own_reconciliation_condition() {
        let first = HealthScope::external_attempt(ExternalAttemptId::from_uuid(Uuid::from_u128(1)));
        let second =
            HealthScope::external_attempt(ExternalAttemptId::from_uuid(Uuid::from_u128(2)));
        assert_ne!(first, second);

        let ledger = HealthLedger::new()
            .raise(
                condition_id(1),
                HealthConditionKind::ReconciliationRequired,
                first,
                at(1),
            )
            .unwrap()
            .advanced()
            .unwrap()
            .raise(
                condition_id(2),
                HealthConditionKind::ReconciliationRequired,
                second,
                at(2),
            )
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(ledger.open().count(), 2);

        // The attempt scope does not collide with an obligation scope.
        assert!(!ledger.is_open(
            HealthConditionKind::ReconciliationRequired,
            obligation_scope()
        ));
        assert!(ledger.is_open(HealthConditionKind::ReconciliationRequired, first));
    }

    #[test]
    fn raising_the_same_condition_twice_is_idempotent() {
        let ledger = HealthLedger::new()
            .raise(
                condition_id(1),
                HealthConditionKind::SuspectedStall,
                obligation_scope(),
                at(1),
            )
            .unwrap()
            .advanced()
            .unwrap();
        assert!(ledger.is_open(HealthConditionKind::SuspectedStall, obligation_scope()));

        let repeat = ledger
            .raise(
                condition_id(2),
                HealthConditionKind::SuspectedStall,
                obligation_scope(),
                at(2),
            )
            .unwrap();
        assert!(repeat.is_duplicate());
    }

    #[test]
    fn resolving_closes_only_the_matching_condition() {
        let ledger = HealthLedger::new()
            .raise(
                condition_id(1),
                HealthConditionKind::SuspectedStall,
                obligation_scope(),
                at(1),
            )
            .unwrap()
            .advanced()
            .unwrap()
            .raise(
                condition_id(2),
                HealthConditionKind::RuntimeStateConflict,
                obligation_scope(),
                at(2),
            )
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(ledger.open().count(), 2);

        let resolved = ledger
            .resolve(
                HealthConditionKind::SuspectedStall,
                obligation_scope(),
                at(3),
            )
            .unwrap()
            .advanced()
            .unwrap();
        assert_eq!(resolved.open().count(), 1);
        assert!(resolved.is_open(
            HealthConditionKind::RuntimeStateConflict,
            obligation_scope()
        ));
        assert_eq!(ledger.open().count(), 2, "the prior value is untouched");
    }

    #[test]
    fn resolving_an_absent_condition_is_a_no_op() {
        let ledger = HealthLedger::new();
        assert!(
            ledger
                .resolve(
                    HealthConditionKind::SuspectedStall,
                    obligation_scope(),
                    at(1)
                )
                .unwrap()
                .is_duplicate()
        );
    }
}
