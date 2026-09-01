//! Encoding domain values as columns, and decoding them back.
//!
//! Two rules hold throughout:
//!
//! - **Closed label sets.** Every enum column has an explicit, hand-written
//!   label map. Labels are not derived from Rust variant names at runtime, so
//!   renaming a variant cannot silently rewrite the on-disk format, and a label
//!   the map does not contain is a corrupt row rather than a default.
//! - **Nothing free-form.** Text columns that carry provider-supplied values are
//!   decoded through [`SafeToken`], whose charset excludes whitespace and path
//!   separators. A prompt, a shell command, a cwd or a transcript path cannot
//!   survive a round trip through this module.

use governor_core::binding::WriteCapabilityState;
use governor_core::delivery::{DeliveryId, DeliveryKey, MalformedDeliveryId};
use governor_core::effect::{
    EffectAmbiguityReason, ExternalAttemptState, ExternalEffectClass, IdempotencyContract,
    IdempotencyKey, NoEffectClass,
};
use governor_core::error::ConflictKind;
use governor_core::fence::{SafeToken, SourceRef};
use governor_core::health::{HealthConditionKind, HealthConditionState};
use governor_core::id::{Id, IdKind};
use governor_core::lease::LeaseState;
use governor_core::mutation::{MutationCommandStatus, SafeMutationResult};
use governor_core::obligation::{Disposition, ObligationKind, ObligationState};
use governor_core::outbound::{AmbiguityReason, AttemptState, DeliveryState, FailureClass};
use governor_core::time::{DurationMs, Timestamp};
use governor_core::worker_evidence::WorkerFailureClass;

use crate::error::{CorruptReason, CorruptValue, StoreError, StoreResult};

/// Who caused an obligation transition. Store-level classification.
///
/// `docs/data-model.md` requires `obligation_events.actor_class` but does not
/// enumerate it; this is the closed Phase 1 set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ActorClass {
    /// A verified worker/runtime fact.
    Worker,
    /// The bound foreman, under a current claim.
    Foreman,
    /// The local user, through the CLI.
    User,
    /// Command Governor itself: recovery, expiry, supersession.
    Daemon,
}

/// Projection of a turn's lifecycle.
///
/// `docs/data-model.md` requires `turns.lifecycle_state` but does not enumerate
/// it, and `governor-core` has no turn machine; this is the closed Phase 1 set,
/// derived from accepted worker events and never copied from runtime status
/// text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TurnLifecycle {
    /// A worker turn is verified to be running.
    Running,
    /// A confirmed final result was published for this turn.
    Completed,
    /// A verified terminal worker failure ended this turn.
    Failed,
}

/// Retention of a result artifact, as projected onto the metadata row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RetentionLabel {
    /// At least one open obligation requires it.
    Pinned,
    /// No open obligation requires it; policy delay then applies.
    Eligible,
}

/// Lifecycle of a persisted foreman claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ClaimLifecycle {
    /// The claim currently holds its obligation.
    Live,
    /// The claim's bound lifetime elapsed.
    Expired,
    /// The claim closed its obligation with a disposition.
    Closed,
}

fn unsupported(table: &'static str, column: &'static str) -> StoreError {
    CorruptValue::new(table, column, CorruptReason::UnknownLabel).into()
}

/// Generates a label codec for an enum whose variants this crate covers fully.
macro_rules! closed_labels {
    (
        $ty:ty, $column:literal,
        encode = $enc:ident, decode = $dec:ident,
        { $( $variant:path => $label:literal ),* $(,)? }
    ) => {
        #[doc = concat!("Encodes a value for the `", $column, "` column.")]
        pub(crate) const fn $enc(value: $ty) -> &'static str {
            match value {
                $( $variant => $label, )*
            }
        }

        #[doc = concat!("Decodes a `", $column, "` column value.")]
        pub(crate) fn $dec(text: &str, table: &'static str) -> StoreResult<$ty> {
            match text {
                $( $label => Ok($variant), )*
                _ => Err(unsupported(table, $column)),
            }
        }
    };
}

/// Generates an encoder for a column nothing in this crate reads back.
///
/// `obligation_events.actor_class` is written for operators and for audit; no
/// code path branches on it, so generating a decoder would leave dead code
/// pretending to be a contract.
macro_rules! encode_labels {
    (
        $ty:ty, $column:literal,
        encode = $enc:ident,
        { $( $variant:path => $label:literal ),* $(,)? }
    ) => {
        #[doc = concat!("Encodes a value for the `", $column, "` column.")]
        pub(crate) const fn $enc(value: $ty) -> &'static str {
            match value {
                $( $variant => $label, )*
            }
        }
    };
}

/// Generates a label codec that *delegates* to the domain type's own `code()`.
///
/// Preferred wherever `governor-core` publishes a stable `snake_case` code: the
/// codes are part of the contract with the CLI and the acceptance suite, and
/// delegating makes it impossible for the on-disk label to drift away from the
/// one a caller branches on. Decoding scans the declared variant list, so a
/// variant added upstream and not listed here is a fail-closed corrupt row.
macro_rules! code_labels {
    (
        $ty:ty, $column:literal,
        encode = $enc:ident, decode = $dec:ident,
        all = [ $( $variant:path ),* $(,)? ]
    ) => {
        #[doc = concat!("Encodes a value for the `", $column, "` column.")]
        pub(crate) const fn $enc(value: $ty) -> &'static str {
            value.code()
        }

        #[doc = concat!("Decodes a `", $column, "` column value.")]
        pub(crate) fn $dec(text: &str, table: &'static str) -> StoreResult<$ty> {
            const ALL: &[$ty] = &[ $( $variant ),* ];
            ALL.iter()
                .copied()
                .find(|value| value.code() == text)
                .ok_or_else(|| unsupported(table, $column))
        }
    };
}

/// Generates a label codec for a `#[non_exhaustive]` enum.
///
/// A variant added upstream that this crate has no label for is a fail-closed
/// corruption error, never a silent default.
macro_rules! open_labels {
    (
        $ty:ty, $column:literal,
        encode = $enc:ident, decode = $dec:ident,
        { $( $variant:path => $label:literal ),* $(,)? }
    ) => {
        #[doc = concat!("Encodes a value for the `", $column, "` column.")]
        pub(crate) fn $enc(value: $ty, table: &'static str) -> StoreResult<&'static str> {
            match value {
                $( $variant => Ok($label), )*
                _ => Err(unsupported(table, $column)),
            }
        }

        #[doc = concat!("Decodes a `", $column, "` column value.")]
        pub(crate) fn $dec(text: &str, table: &'static str) -> StoreResult<$ty> {
            match text {
                $( $label => Ok($variant), )*
                _ => Err(unsupported(table, $column)),
            }
        }
    };
}

closed_labels! {
    ObligationState, "state",
    encode = encode_obligation_state, decode = decode_obligation_state,
    {
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

closed_labels! {
    DeliveryState, "state",
    encode = encode_delivery_state, decode = decode_delivery_state,
    {
        DeliveryState::Pending => "pending",
        DeliveryState::Claimed => "claimed",
        DeliveryState::Accepted => "accepted",
        DeliveryState::Failed => "failed",
        DeliveryState::Ambiguous => "ambiguous",
    }
}

closed_labels! {
    AttemptState, "state",
    encode = encode_attempt_state, decode = decode_attempt_state,
    {
        AttemptState::Claimed => "claimed",
        AttemptState::ActivationArmed => "activation_armed",
        AttemptState::Accepted => "accepted",
        AttemptState::Failed => "failed",
        AttemptState::Ambiguous => "ambiguous",
    }
}

encode_labels! {
    ActorClass, "actor_class",
    encode = encode_actor_class,
    {
        ActorClass::Worker => "worker",
        ActorClass::Foreman => "foreman",
        ActorClass::User => "user",
        ActorClass::Daemon => "daemon",
    }
}

closed_labels! {
    TurnLifecycle, "lifecycle_state",
    encode = encode_turn_lifecycle, decode = decode_turn_lifecycle,
    {
        TurnLifecycle::Running => "running",
        TurnLifecycle::Completed => "completed",
        TurnLifecycle::Failed => "failed",
    }
}

closed_labels! {
    RetentionLabel, "retention_state",
    encode = encode_retention, decode = decode_retention,
    {
        RetentionLabel::Pinned => "pinned",
        RetentionLabel::Eligible => "eligible",
    }
}

closed_labels! {
    ClaimLifecycle, "state",
    encode = encode_claim_state, decode = decode_claim_state,
    {
        ClaimLifecycle::Live => "live",
        ClaimLifecycle::Expired => "expired",
        ClaimLifecycle::Closed => "closed",
    }
}

open_labels! {
    ObligationKind, "obligation_kind",
    encode = encode_obligation_kind, decode = decode_obligation_kind,
    {
        ObligationKind::WorkerTurn => "worker_turn",
    }
}

open_labels! {
    Disposition, "disposition",
    encode = encode_disposition, decode = decode_disposition,
    {
        Disposition::Accepted => "accepted",
        Disposition::RejectedNeedsRework => "rejected_needs_rework",
        Disposition::FailureAcknowledged => "failure_acknowledged",
        Disposition::Abandoned => "abandoned",
    }
}

open_labels! {
    FailureClass, "failure_class",
    encode = encode_failure_class, decode = decode_failure_class,
    {
        FailureClass::TargetNotFound => "target_not_found",
        FailureClass::StaleTarget => "stale_target",
        FailureClass::WrongConversation => "wrong_conversation",
        FailureClass::AppNotSelected => "app_not_selected",
        FailureClass::ComposerNotReady => "composer_not_ready",
        FailureClass::NavigationBlocked => "navigation_blocked",
        FailureClass::ActivationRefused => "activation_refused",
        FailureClass::TransportRejectedBeforeSend => "transport_rejected_before_send",
    }
}

open_labels! {
    AmbiguityReason, "evidence_class",
    encode = encode_ambiguity, decode = decode_ambiguity,
    {
        AmbiguityReason::OrphanedByRestart => "orphaned_by_restart",
        AmbiguityReason::ObservationLost => "observation_lost",
        AmbiguityReason::EvidenceInconclusive => "evidence_inconclusive",
        AmbiguityReason::ActivationTimedOut => "activation_timed_out",
    }
}

open_labels! {
    WorkerFailureClass, "failure_class",
    encode = encode_worker_failure, decode = decode_worker_failure,
    {
        WorkerFailureClass::StructuredError => "structured_error",
        WorkerFailureClass::StopFailure => "stop_failure",
        WorkerFailureClass::Interrupted => "interrupted",
    }
}

closed_labels! {
    HealthConditionState, "state",
    encode = encode_health_state, decode = decode_health_state,
    {
        HealthConditionState::Open => "open",
        HealthConditionState::Resolved => "resolved",
    }
}

open_labels! {
    WriteCapabilityState, "write_capability_state",
    encode = encode_write_capability, decode = decode_write_capability,
    {
        WriteCapabilityState::Unknown => "unknown",
        WriteCapabilityState::Proven => "proven",
        WriteCapabilityState::ReadFetchOnlyUnsupported => "read_fetch_only_unsupported",
        WriteCapabilityState::Lost => "lost",
        WriteCapabilityState::BlockedByConfirmation => "blocked_by_confirmation",
    }
}

code_labels! {
    HealthConditionKind, "kind",
    encode = encode_health_kind, decode = decode_health_kind,
    all = [
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
        HealthConditionKind::LoadoutUnverifiable,
        HealthConditionKind::ManagedConfigMissing,
        HealthConditionKind::LineageBroken,
    ]
}

code_labels! {
    MutationCommandStatus, "status",
    encode = encode_mutation_status, decode = decode_mutation_status,
    all = [
        MutationCommandStatus::Received,
        MutationCommandStatus::Completed,
        MutationCommandStatus::Uncertain,
        MutationCommandStatus::Acked,
    ]
}

code_labels! {
    ExternalAttemptState, "state",
    encode = encode_attempt_effect_state, decode = decode_attempt_effect_state,
    all = [
        ExternalAttemptState::IntentRecorded,
        ExternalAttemptState::Completed,
        ExternalAttemptState::FailedBeforeEffect,
        ExternalAttemptState::Ambiguous,
    ]
}

code_labels! {
    NoEffectClass, "no_effect_class",
    encode = encode_no_effect, decode = decode_no_effect,
    all = [
        NoEffectClass::NotAttempted,
        NoEffectClass::RejectedBeforeDispatch,
        NoEffectClass::DestinationRefusedWithoutApplying,
        NoEffectClass::PreconditionRejectedAtDestination,
    ]
}

code_labels! {
    EffectAmbiguityReason, "ambiguity_reason",
    encode = encode_effect_ambiguity, decode = decode_effect_ambiguity,
    all = [
        EffectAmbiguityReason::OrphanedByRestart,
        EffectAmbiguityReason::ResponseLost,
        EffectAmbiguityReason::DeadlineElapsed,
        EffectAmbiguityReason::EvidenceInconclusive,
    ]
}

code_labels! {
    LeaseState, "state",
    encode = encode_lease_state, decode = decode_lease_state,
    all = [LeaseState::Held, LeaseState::Released]
}

code_labels! {
    ConflictKind, "safe_result_conflict",
    encode = encode_conflict_kind, decode = decode_conflict_kind,
    all = [
        ConflictKind::StaleBindingGeneration,
        ConflictKind::UnknownBindingGeneration,
        ConflictKind::NoActiveBinding,
        ConflictKind::StaleObligationVersion,
        ConflictKind::StaleSourceFence,
        ConflictKind::StaleClaim,
        ConflictKind::NoCurrentClaim,
        ConflictKind::ExpiredClaim,
        ConflictKind::ObligationAlreadyClaimed,
        ConflictKind::UnknownDeliveryId,
        ConflictKind::StaleSessionIncarnation,
        ConflictKind::StaleDeliveryTarget,
        ConflictKind::IllegalObligationTransition,
        ConflictKind::ObligationClosed,
        ConflictKind::DeliveryRevisionFrozen,
        ConflictKind::DeliveryRevisionStillLive,
        ConflictKind::DeliveryRevisionSuperseded,
        ConflictKind::IllegalDeliveryTransition,
        ConflictKind::UnknownAttempt,
        ConflictKind::RetryBudgetExhausted,
        ConflictKind::RetryAfterAmbiguityFence,
        ConflictKind::FailureNotProven,
        ConflictKind::ForemanTurnNotQuiescent,
        ConflictKind::ConflictingInputAnswer,
        ConflictKind::IllegalInputTransition,
        ConflictKind::StaleCommandRevision,
        ConflictKind::InvalidDisposition,
        ConflictKind::ExecuteRequiresDurableIntent,
        ConflictKind::IllegalAttemptTransition,
        ConflictKind::AttemptAlreadyCompleted,
        ConflictKind::AttemptAlreadyDispatched,
        ConflictKind::AttemptPermitMismatch,
        ConflictKind::EffectNotProvenAbsent,
        ConflictKind::RetryRequiresIdempotencyContract,
        ConflictKind::MutationResultUncertain,
        ConflictKind::MutationCommandMismatch,
        ConflictKind::IllegalMutationTransition,
        ConflictKind::MutationNotCompleted,
        ConflictKind::StaleLeaseToken,
        ConflictKind::StaleProcessIncarnation,
        ConflictKind::StaleDaemonEpoch,
        ConflictKind::ResourceAlreadyLeased,
        ConflictKind::NoCurrentLease,
        ConflictKind::IllegalLeaseTransition,
    ]
}

// --- Composite domain values ------------------------------------------------

/// The `mutation_commands` result columns, as a group.
///
/// Three columns rather than one blob. `SafeMutationResult` has exactly three
/// shapes and one variable part each, and a single free-form column would be a
/// place for a response body — and therefore a prompt or a credential — to
/// accumulate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredMutationResult {
    pub(crate) kind: &'static str,
    pub(crate) reference: Option<String>,
    pub(crate) conflict: Option<&'static str>,
}

/// Encodes a bounded safe mutation result into its three columns.
///
/// # Errors
///
/// `SafeMutationResult` is `#[non_exhaustive]`: a shape this crate has no
/// columns for is refused rather than written as a weaker known one.
pub(crate) fn encode_mutation_result(
    result: &SafeMutationResult,
) -> StoreResult<StoredMutationResult> {
    match result {
        SafeMutationResult::Applied { reference } => Ok(StoredMutationResult {
            kind: result.kind_code(),
            reference: reference.as_ref().map(|token| token.as_str().to_owned()),
            conflict: None,
        }),
        SafeMutationResult::AlreadySatisfied => Ok(StoredMutationResult {
            kind: result.kind_code(),
            reference: None,
            conflict: None,
        }),
        SafeMutationResult::Refused { conflict } => Ok(StoredMutationResult {
            kind: result.kind_code(),
            reference: None,
            conflict: Some(encode_conflict_kind(*conflict)),
        }),
        _ => Err(unsupported("mutation_commands", "safe_result_kind")),
    }
}

/// Rehydrates a bounded safe mutation result from its three columns.
///
/// # Errors
///
/// Returns a corrupt-row error when the columns do not form one of the three
/// shapes, which the table's `CHECK` constraints also refuse to store.
pub(crate) fn decode_mutation_result(
    kind: &str,
    reference: Option<&str>,
    conflict: Option<&str>,
) -> StoreResult<SafeMutationResult> {
    const TABLE: &str = "mutation_commands";
    match (kind, reference, conflict) {
        ("applied", reference, None) => Ok(SafeMutationResult::Applied {
            reference: reference
                .map(|text| parse_token(text, "mutation_commands", "safe_result_ref"))
                .transpose()?,
        }),
        ("already_satisfied", None, None) => Ok(SafeMutationResult::AlreadySatisfied),
        ("refused", None, Some(code)) => Ok(SafeMutationResult::Refused {
            conflict: decode_conflict_kind(code, TABLE)?,
        }),
        _ => Err(unsupported(TABLE, "safe_result_kind")),
    }
}

/// The `external_attempts` effect-class columns, as a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredEffectClass {
    pub(crate) class: &'static str,
    pub(crate) contract: Option<&'static str>,
    pub(crate) window_ms: Option<i64>,
    pub(crate) key: Option<String>,
}

/// Encodes an effect class into its four columns.
///
/// The contract and the exact key travel together, because a retry may rest
/// only on a recorded contract *and* the key that contract keys on.
///
/// # Errors
///
/// `IdempotencyContract` is `#[non_exhaustive]`: a mechanism this crate has no
/// column for is refused on write rather than recorded as a weaker known one.
/// An unrepresentable window is refused for the same reason.
pub(crate) fn encode_effect_class(class: &ExternalEffectClass) -> StoreResult<StoredEffectClass> {
    const TABLE: &str = "external_attempts";
    let (contract, window_ms) = match class.idempotency_contract() {
        Some(IdempotencyContract::DeduplicatedByKey { window }) => (
            Some("deduplicated_by_key"),
            Some(store_u64(
                window.as_millis(),
                TABLE,
                "idempotency_window_ms",
            )?),
        ),
        Some(IdempotencyContract::ConditionalOnDestinationFence) => {
            (Some("conditional_on_destination_fence"), None)
        }
        Some(_) => return Err(unsupported(TABLE, "idempotency_contract")),
        None => (None, None),
    };
    Ok(StoredEffectClass {
        class: class.code(),
        contract,
        window_ms,
        key: class
            .idempotency_key()
            .map(|key| key.as_token().as_str().to_owned()),
    })
}

/// Rehydrates an effect class from its four columns.
///
/// # Errors
///
/// Returns a corrupt-row error for any combination the class cannot represent.
pub(crate) fn decode_effect_class(
    class: &str,
    contract: Option<&str>,
    window_ms: Option<i64>,
    key: Option<&str>,
) -> StoreResult<ExternalEffectClass> {
    const TABLE: &str = "external_attempts";
    match (class, contract, key) {
        ("read", None, None) => Ok(ExternalEffectClass::Read),
        ("non_idempotent_write", None, None) => Ok(ExternalEffectClass::NonIdempotentWrite),
        ("idempotent_write", Some(mechanism), Some(key)) => {
            let contract = match (mechanism, window_ms) {
                ("deduplicated_by_key", Some(window)) => IdempotencyContract::DeduplicatedByKey {
                    window: DurationMs::from_millis(parse_u64(
                        window,
                        TABLE,
                        "idempotency_window_ms",
                    )?),
                },
                ("conditional_on_destination_fence", None) => {
                    IdempotencyContract::ConditionalOnDestinationFence
                }
                _ => return Err(unsupported(TABLE, "idempotency_contract")),
            };
            Ok(ExternalEffectClass::IdempotentWrite {
                contract,
                key: IdempotencyKey::new(parse_token(key, TABLE, "idempotency_key")?),
            })
        }
        _ => Err(unsupported(TABLE, "effect_class")),
    }
}

// --- Scalars ----------------------------------------------------------------

/// Renders an opaque identity for persistence.
pub(crate) fn id_text<K: IdKind>(id: Id<K>) -> String {
    id.to_string()
}

/// Rehydrates an opaque identity from a column.
pub(crate) fn parse_id<K: IdKind>(
    text: &str,
    table: &'static str,
    column: &'static str,
) -> StoreResult<Id<K>> {
    Id::parse(text)
        .map_err(|_| CorruptValue::new(table, column, CorruptReason::MalformedIdentity).into())
}

/// Rehydrates a redaction-safe token from a column.
pub(crate) fn parse_token(
    text: &str,
    table: &'static str,
    column: &'static str,
) -> StoreResult<SafeToken> {
    SafeToken::new(text)
        .map_err(|_| CorruptValue::new(table, column, CorruptReason::UnsafeToken).into())
}

/// Rehydrates a browser wake correlation ID from its persisted hex form.
pub(crate) fn parse_delivery_id(
    text: &str,
    table: &'static str,
    column: &'static str,
) -> StoreResult<DeliveryId> {
    DeliveryId::parse_persisted(text).map_err(|MalformedDeliveryId| {
        CorruptValue::new(table, column, CorruptReason::MalformedIdentity).into()
    })
}

/// Re-derives a delivery key and checks it against the persisted column.
///
/// `governor-core` deliberately offers no byte-wise constructor for
/// [`DeliveryKey`]: a key means nothing except as a function of its scheduling
/// tuple. So the store never rebuilds one from stored bytes. It derives the key
/// from the obligation, generation and revision in the very same row and
/// requires the stored hex to match. A row that has been edited, or written by
/// a different derivation, fails closed here rather than authorising a claim.
///
/// # Errors
///
/// Returns a corrupt-row error when the stored hex does not match.
pub(crate) fn rederive_delivery_key(
    stored_hex: &str,
    obligation: governor_core::id::ObligationId,
    generation: governor_core::fence::BindingGeneration,
    revision: governor_core::fence::DeliveryRevision,
) -> StoreResult<DeliveryKey> {
    let derived = DeliveryKey::derive(obligation, generation, revision);
    if derived.to_hex() == stored_hex {
        Ok(derived)
    } else {
        Err(CorruptValue::new(
            "browser_deliveries",
            "delivery_key",
            CorruptReason::MalformedIdentity,
        )
        .into())
    }
}

/// Renders 32 bytes as the lowercase hex form used by every digest column.
pub(crate) fn hex32(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Rehydrates 32 bytes from a lowercase hex digest column.
///
/// # Errors
///
/// Returns a corrupt-row error when the text is not exactly 64 hex digits. The
/// rejected text is never echoed.
pub(crate) fn parse_hex32(
    text: &str,
    table: &'static str,
    column: &'static str,
) -> StoreResult<[u8; 32]> {
    let malformed = || CorruptValue::new(table, column, CorruptReason::MalformedIdentity);
    if text.len() != 64 {
        return Err(malformed().into());
    }
    let mut bytes = [0u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let pair = text.get(index * 2..index * 2 + 2).ok_or_else(malformed)?;
        *slot = u8::from_str_radix(pair, 16).map_err(|_| malformed())?;
    }
    Ok(bytes)
}

/// Rehydrates a fixed-width possession token from a `BLOB` column.
///
/// # Errors
///
/// Returns a corrupt-row error when the blob is the wrong width.
pub(crate) fn parse_token_bytes<const N: usize>(
    blob: &[u8],
    table: &'static str,
    column: &'static str,
) -> StoreResult<[u8; N]> {
    <[u8; N]>::try_from(blob)
        .map_err(|_| CorruptValue::new(table, column, CorruptReason::MalformedIdentity).into())
}

/// Narrows a SQLite signed integer to an unsigned counter.
pub(crate) fn parse_u64(value: i64, table: &'static str, column: &'static str) -> StoreResult<u64> {
    u64::try_from(value)
        .map_err(|_| CorruptValue::new(table, column, CorruptReason::IntegerOutOfRange).into())
}

/// Narrows a SQLite signed integer to a bounded counter.
pub(crate) fn parse_u32(value: i64, table: &'static str, column: &'static str) -> StoreResult<u32> {
    u32::try_from(value)
        .map_err(|_| CorruptValue::new(table, column, CorruptReason::IntegerOutOfRange).into())
}

/// Widens an unsigned counter for storage, refusing a value SQLite cannot hold.
///
/// Silent truncation here would corrupt a durability fence, so it is an error.
pub(crate) fn store_u64(value: u64, table: &'static str, column: &'static str) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| CorruptValue::new(table, column, CorruptReason::IntegerOutOfRange).into())
}

/// Renders an instant for a `*_at_ms` column.
pub(crate) const fn store_time(at: Timestamp) -> i64 {
    at.as_unix_millis()
}

/// Rehydrates an instant from a `*_at_ms` column.
pub(crate) const fn parse_time(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis)
}

/// Rehydrates the source identity triple from an `events` row.
pub(crate) fn parse_source(namespace: &str, event: &str, fence: &str) -> StoreResult<SourceRef> {
    Ok(SourceRef::new(
        parse_token(namespace, "events", "source_namespace")?,
        parse_token(event, "events", "source_event_id")?,
        parse_token(fence, "events", "source_event_fence")?,
    ))
}
