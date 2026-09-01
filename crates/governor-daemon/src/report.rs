//! Rendering daemon state as stable, greppable, safe lines.
//!
//! # One format, two consumers
//!
//! These lines are both the IPC payload and what the command line prints. There
//! is deliberately no second rendering: a status line the operator reads is
//! byte-for-byte the line the daemon produced, so there is nowhere for a
//! presentation layer to add a field the safe-diagnostics rule forbids.
//!
//! Every line is `key=value` pairs separated by single spaces, where a value is
//! an opaque identity, a `snake_case` class, a counter, a duration in
//! milliseconds, or a boolean (`docs/threat-model.md`, "Threat: diagnostics
//! become exfiltration"). No task title, no repository reference, no artifact
//! content, no path.
//!
//! # Why the state labels live here
//!
//! `governor-store-sqlite` encodes obligation states into its own columns and
//! keeps that encoding private, which is correct: a storage encoding is not a
//! public contract. These labels are the *daemon's* wire contract, and they are
//! produced by a total match with no wildcard arm, so adding a lifecycle state
//! to `governor-core` fails this crate's build rather than silently rendering
//! the new state as something else.

use std::collections::BTreeMap;

use governor_core::health::{HealthConditionKind, HealthScope};
use governor_core::obligation::{ObligationKind, ObligationState};
use governor_core::time::Timestamp;
use governor_store_sqlite::{OpenCondition, OpenObligation};

/// The wire label of one obligation lifecycle state.
///
/// Total by construction: no `_` arm, so a new state is a compile error here.
#[must_use]
pub const fn state_code(state: ObligationState) -> &'static str {
    match state {
        ObligationState::Created => "created",
        ObligationState::Running => "running",
        ObligationState::NeedsInput => "needs_input",
        ObligationState::Failed => "failed",
        ObligationState::CompletedUnprocessed => "completed_unprocessed",
        ObligationState::ClaimedByForeman => "claimed_by_foreman",
        ObligationState::Processing => "processing",
        ObligationState::Acknowledged => "acknowledged",
        ObligationState::CancelledByUser => "cancelled_by_user",
        ObligationState::Superseded => "superseded",
    }
}

/// Every lifecycle state, so a status line can report a zero rather than omit
/// the key. A missing key and a zero count are different facts to a script.
const ALL_STATES: &[ObligationState] = &[
    ObligationState::Created,
    ObligationState::Running,
    ObligationState::NeedsInput,
    ObligationState::Failed,
    ObligationState::CompletedUnprocessed,
    ObligationState::ClaimedByForeman,
    ObligationState::Processing,
    ObligationState::Acknowledged,
    ObligationState::CancelledByUser,
    ObligationState::Superseded,
];

/// The wire label of one obligation kind.
#[must_use]
pub const fn kind_code(kind: ObligationKind) -> &'static str {
    match kind {
        ObligationKind::WorkerTurn => "worker_turn",
        // `ObligationKind` is `#[non_exhaustive]`, so a wildcard is required.
        // An unknown kind is rendered as unknown rather than guessed at.
        _ => "unknown",
    }
}

/// One line per open obligation: identity, class, fence, age.
///
/// Sorted by age, oldest first, because the oldest open obligation is the one
/// an operator is looking for.
#[must_use]
pub fn obligation_lines(obligations: &[OpenObligation], now: Timestamp) -> Vec<String> {
    let mut rows: Vec<&OpenObligation> = obligations.iter().collect();
    rows.sort_by_key(|row| row.created_at);
    rows.into_iter()
        .map(|row| {
            format!(
                "obligation id={} kind={} state={} version={} age_ms={} artifact={}",
                row.id,
                kind_code(row.kind),
                state_code(row.state),
                row.version.get(),
                now.saturating_elapsed_since(row.created_at).as_millis(),
                match &row.result_artifact {
                    Some(artifact) => artifact.id().to_string(),
                    None => "none".to_owned(),
                }
            )
        })
        .collect()
}

/// Aggregate counts over the open obligations.
///
/// Counts by state, and separately by *attention* state, because "how much work
/// is open" and "how much work is waiting for the foreman" are the two
/// questions status exists to answer and they are not the same number.
#[must_use]
pub fn obligation_summary_lines(obligations: &[OpenObligation]) -> Vec<String> {
    let mut by_state: BTreeMap<&'static str, usize> = ALL_STATES
        .iter()
        .filter(|state| state.is_open())
        .map(|state| (state_code(*state), 0))
        .collect();
    let mut attention = 0_usize;

    for row in obligations {
        *by_state.entry(state_code(row.state)).or_default() += 1;
        if row.state.attention().is_some() {
            attention += 1;
        }
    }

    let mut lines = vec![
        format!("obligations.open={}", obligations.len()),
        format!("obligations.attention={attention}"),
    ];
    lines.extend(
        by_state
            .into_iter()
            .map(|(state, count)| format!("obligations.state.{state}={count}")),
    );
    lines
}

/// Aggregate lines for the open health conditions.
///
/// One count per kind, including the kinds at zero, plus one scoped line per
/// open condition so an operator can see *which* obligation is affected without
/// being told anything about it.
#[must_use]
pub fn health_lines(conditions: &[OpenCondition]) -> Vec<String> {
    let mut by_kind: BTreeMap<&'static str, usize> = ALL_HEALTH_KINDS
        .iter()
        .map(|kind| (kind.code(), 0))
        .collect();
    for condition in conditions {
        *by_kind.entry(condition.kind.code()).or_default() += 1;
    }

    let mut lines = vec![format!("health.open={}", conditions.len())];
    lines.extend(
        by_kind
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(kind, count)| format!("health.kind.{kind}={count}")),
    );
    lines.extend(conditions.iter().map(|condition| {
        format!(
            "health kind={} {}",
            condition.kind.code(),
            scope(condition.scope)
        )
    }));
    lines
}

/// Every attention class the health ledger can raise.
const ALL_HEALTH_KINDS: &[HealthConditionKind] = &[
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

fn scope(scope: HealthScope) -> String {
    let mut fields = Vec::new();
    if let Some(task) = scope.task {
        fields.push(format!("scope.task={task}"));
    }
    if let Some(turn) = scope.turn {
        fields.push(format!("scope.turn={turn}"));
    }
    if let Some(obligation) = scope.obligation {
        fields.push(format!("scope.obligation={obligation}"));
    }
    if let Some(attempt) = scope.external_attempt {
        fields.push(format!("scope.external_attempt={attempt}"));
    }
    if fields.is_empty() {
        return "scope=global".to_owned();
    }
    fields.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor_core::artifact::{ArtifactDigest, ResultArtifact};
    use governor_core::fence::{ObligationVersion, SafeToken};
    use governor_core::id::{ObligationId, ResultArtifactId};
    use uuid::Uuid;

    fn obligation(state: ObligationState, created_ms: i64, with_artifact: bool) -> OpenObligation {
        let id = ObligationId::from_uuid(Uuid::from_u128(u128::try_from(created_ms).unwrap_or(1)));
        OpenObligation {
            id,
            kind: ObligationKind::WorkerTurn,
            state,
            version: ObligationVersion::new(3),
            created_at: Timestamp::from_unix_millis(created_ms),
            result_artifact: with_artifact.then(|| {
                ResultArtifact::new(
                    ResultArtifactId::from_uuid(Uuid::from_u128(9)),
                    SafeToken::new("ra-000000000000").expect("token"),
                    ArtifactDigest::from_bytes([0; 32]),
                    12,
                    Timestamp::from_unix_millis(created_ms),
                )
            }),
        }
    }

    #[test]
    fn every_lifecycle_state_has_a_distinct_label() {
        let mut codes: Vec<&str> = ALL_STATES.iter().map(|state| state_code(*state)).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total);
    }

    #[test]
    fn an_obligation_line_carries_only_identities_classes_and_counters() {
        let now = Timestamp::from_unix_millis(5_000);
        let lines = obligation_lines(
            &[obligation(
                ObligationState::CompletedUnprocessed,
                1_000,
                true,
            )],
            now,
        );
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.contains("kind=worker_turn"));
        assert!(line.contains("state=completed_unprocessed"));
        assert!(line.contains("version=3"));
        assert!(line.contains("age_ms=4000"));
        for field in line.split_whitespace().skip(1) {
            let (_, value) = field.split_once('=').expect("every field is key=value");
            assert!(
                !value.contains('/') && !value.is_empty(),
                "a value must not look like a path: {value}"
            );
        }
    }

    #[test]
    fn obligation_lines_are_oldest_first() {
        let now = Timestamp::from_unix_millis(9_000);
        let lines = obligation_lines(
            &[
                obligation(ObligationState::Running, 3_000, false),
                obligation(ObligationState::Created, 1_000, false),
            ],
            now,
        );
        assert!(lines[0].contains("age_ms=8000"));
        assert!(lines[1].contains("age_ms=6000"));
    }

    #[test]
    fn a_summary_reports_zero_rather_than_omitting_a_state() {
        let lines = obligation_summary_lines(&[obligation(ObligationState::Failed, 1, false)]);
        assert!(lines.contains(&"obligations.open=1".to_owned()));
        assert!(lines.contains(&"obligations.attention=1".to_owned()));
        assert!(lines.contains(&"obligations.state.failed=1".to_owned()));
        assert!(lines.contains(&"obligations.state.running=0".to_owned()));
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with("obligations.state.acknowledged")),
            "a closed state is not open work"
        );
    }

    #[test]
    fn health_lines_name_the_kind_and_the_scope_and_nothing_else() {
        let obligation_id = ObligationId::from_uuid(Uuid::from_u128(42));
        let lines = health_lines(&[OpenCondition {
            kind: HealthConditionKind::ResultArtifactMissing,
            scope: HealthScope {
                task: None,
                turn: None,
                obligation: Some(obligation_id),
                external_attempt: None,
            },
        }]);
        assert!(lines.contains(&"health.open=1".to_owned()));
        assert!(lines.contains(&"health.kind.result_artifact_missing=1".to_owned()));
        assert!(lines.iter().any(|line| {
            line == &format!("health kind=result_artifact_missing scope.obligation={obligation_id}")
        }));
    }

    #[test]
    fn no_open_conditions_still_reports_the_count() {
        assert_eq!(health_lines(&[]), vec!["health.open=0".to_owned()]);
    }
}
