//! Projection replay equivalence, and the watermark that records it.
//!
//! `docs/testing.md` DB-001: *after every generated state-machine sequence,
//! rebuild materialized projections from events; semantic state must match.*
//! `docs/architecture.md` startup order goes further — a mismatch **fails
//! closed**, and the daemon enters repair rather than continuing.
//!
//! # What is compared
//!
//! Every obligation is rebuilt by folding its ledger slice through
//! [`governor_core::obligation::Obligation`], and every wake revision by folding
//! its attempt events through [`governor_core::outbound::Delivery`]. The result
//! is compared field by field with the stored projection row. Disagreements are
//! collected — all of them, not just the first — into [`RepairNeeded`], so an
//! operator sees the shape of the damage rather than one symptom.
//!
//! Each obligation transition is also checked against `obligation_events`: the
//! version the fold produces at each step must equal the version recorded for
//! that event. `UNIQUE(obligation_id, obligation_version)` means a
//! double-applied transition cannot even be recorded, and this check means a
//! *missing* one cannot hide either.
//!
//! # What is not compared, and why
//!
//! The mutation journal, external attempts and resource leases are not derived
//! from the ledger — their own row *is* the durable record (see the crate
//! docs). They are still re-proved on every read: each loader folds the row's
//! recorded history through the domain machine and refuses a row no legal
//! sequence of transitions can reach. That is the same property, enforced at a
//! different boundary.

use std::collections::{BTreeMap, BTreeSet};

use governor_core::artifact::{ArtifactDigest, ResultArtifact, RetentionState};
use governor_core::fence::EventSeq;
use governor_core::health::{HealthConditionKind, HealthConditionState, HealthScope};
use governor_core::id::ObligationId;
use governor_core::time::Timestamp;
use rusqlite::params;

use crate::codec::{
    RetentionLabel, TurnLifecycle, decode_attempt_state, decode_delivery_state, decode_health_kind,
    decode_health_state, decode_obligation_state, decode_retention, decode_turn_lifecycle,
    encode_attempt_state, encode_delivery_state, encode_health_kind, encode_health_state,
    encode_obligation_state, encode_retention, encode_turn_lifecycle, id_text, parse_delivery_id,
    parse_hex32, parse_id, parse_token, parse_u32, parse_u64,
};
use crate::error::{ProjectionMismatch, RepairNeeded, StoreResult};
use crate::event::{self, EventKind, LedgerEvent};
use crate::load;
use crate::tx::Tx;

/// The outcome of a successful replay verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedProjections {
    /// Obligations rebuilt and compared.
    pub obligations: usize,
    /// Wake revisions rebuilt and compared.
    pub deliveries: usize,
    /// Highest ledger sequence covered, recorded as the watermark.
    pub verified_through: Option<EventSeq>,
}

/// Rebuilds every projection from the ledger and compares it with the rows.
///
/// # Errors
///
/// - [`crate::StoreError::RepairNeeded`] when any projection disagrees with its
///   replay;
/// - a corrupt-row error when the ledger itself cannot be folded;
/// - a SQLite error.
pub(crate) fn verify(tx: &Tx<'_>) -> StoreResult<VerifiedProjections> {
    let events = event::read_all(tx)?;
    let mut by_obligation: BTreeMap<ObligationId, Vec<LedgerEvent>> = BTreeMap::new();
    for event in &events {
        if let Some(id) = obligation_of(tx, event)? {
            by_obligation.entry(id).or_default().push(event.clone());
        }
    }

    let mut mismatches = Vec::new();
    let mut obligations = 0;

    for (id, slice) in &by_obligation {
        let identity = load::obligation_identity(tx, *id)?;
        let replayed = load::fold_obligation(*id, identity, slice)?;
        obligations += 1;
        compare_obligation(tx, *id, &replayed, &mut mismatches)?;
        compare_transitions(tx, *id, slice, &mut mismatches)?;
    }

    compare_turns(tx, &by_obligation, &mut mismatches)?;
    compare_retention(tx, &by_obligation, &mut mismatches)?;
    compare_health(tx, &events, &mut mismatches)?;
    let deliveries = compare_deliveries(tx, &mut mismatches)?;

    if !mismatches.is_empty() {
        return Err(RepairNeeded { mismatches }.into());
    }

    let verified_through = event::highest_seq(tx)?;
    if let Some(seq) = verified_through {
        crate::meta::set_last_verified_projection_seq(tx.conn(), seq)?;
    }
    Ok(VerifiedProjections {
        obligations,
        deliveries,
        verified_through,
    })
}

fn obligation_of(tx: &Tx<'_>, event: &LedgerEvent) -> StoreResult<Option<ObligationId>> {
    let found: Option<Option<String>> = tx
        .conn()
        .query_row(
            "SELECT obligation_id FROM events WHERE seq = ?1",
            params![event::store_seq(event.seq)?],
            |row| row.get(0),
        )
        .ok();
    let Some(Some(text)) = found else {
        return Ok(None);
    };
    Ok(Some(parse_id(&text, "events", "obligation_id")?))
}

fn compare_obligation(
    tx: &Tx<'_>,
    id: ObligationId,
    replayed: &load::LoadedObligation,
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    let stored: (String, i64, Option<String>, Option<String>, i64) = tx.conn().query_row(
        "SELECT state, current_version, current_claim_id, result_artifact_id, source_event_seq
           FROM obligations WHERE obligation_id = ?1",
        params![id_text(id)],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let (state, version, claim, artifact, source_seq) = stored;
    let projection = &replayed.projection;
    let row = id.to_string();

    let mut record = |column, stored: String, expected: String| {
        if stored != expected {
            mismatches.push(ProjectionMismatch {
                table: "obligations",
                row: row.clone(),
                column,
                stored,
                replayed: expected,
            });
        }
    };
    record(
        "state",
        encode_obligation_state(decode_obligation_state(&state, "obligations")?).to_owned(),
        encode_obligation_state(projection.state()).to_owned(),
    );
    record(
        "current_version",
        version.to_string(),
        projection.version().get().to_string(),
    );
    record(
        "current_claim_id",
        claim.unwrap_or_default(),
        projection.claim().map(id_text).unwrap_or_default(),
    );
    record(
        "result_artifact_id",
        artifact.unwrap_or_default(),
        projection
            .result_artifact()
            .map(id_text)
            .unwrap_or_default(),
    );
    record(
        "source_event_seq",
        source_seq.to_string(),
        replayed.source_event_seq.get().to_string(),
    );
    Ok(())
}

/// Checks the fold against the immutable per-transition version ledger.
fn compare_transitions(
    tx: &Tx<'_>,
    id: ObligationId,
    slice: &[LedgerEvent],
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    let mut statement = tx.conn().prepare(
        "SELECT event_seq, obligation_version FROM obligation_events
          WHERE obligation_id = ?1 ORDER BY obligation_version",
    )?;
    let rows = statement.query_map(params![id_text(id)], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut recorded = BTreeMap::new();
    for row in rows {
        let (seq, version) = row?;
        recorded.insert(
            event::parse_seq(seq, "obligation_events", "event_seq")?,
            parse_u64(version, "obligation_events", "obligation_version")?,
        );
    }

    let identity = load::obligation_identity(tx, id)?;
    for step in 1..=slice.len() {
        let folded = load::fold_obligation(id, identity.clone(), &slice[..step])?;
        let seq = slice[step - 1].seq;
        let Some(expected) = recorded.get(&seq) else {
            continue;
        };
        let actual = folded.projection.version().get();
        if *expected != actual {
            mismatches.push(ProjectionMismatch {
                table: "obligation_events",
                row: format!("{id}@{seq}"),
                column: "obligation_version",
                stored: expected.to_string(),
                replayed: actual.to_string(),
            });
        }
    }
    Ok(())
}

/// Checks each turn's lifecycle against the terminal event its obligation holds.
///
/// `turns.lifecycle_state` is a projection of accepted worker events, never a
/// copy of runtime status text, so replay derives it the same way: a published
/// result completes the turn, a verified failure fails it, anything else leaves
/// it running.
fn compare_turns(
    tx: &Tx<'_>,
    by_obligation: &BTreeMap<ObligationId, Vec<LedgerEvent>>,
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    for (id, slice) in by_obligation {
        let identity = load::obligation_identity(tx, *id)?;
        let Some(turn) = identity.turn else {
            continue;
        };
        let replayed = slice.iter().rev().find_map(|event| match event.kind {
            EventKind::ResultPublished => Some(TurnLifecycle::Completed),
            EventKind::WorkerFailed => Some(TurnLifecycle::Failed),
            _ => None,
        });
        let replayed = replayed.unwrap_or(TurnLifecycle::Running);
        let stored: String = tx.conn().query_row(
            "SELECT lifecycle_state FROM turns WHERE turn_id = ?1",
            params![id_text(turn)],
            |row| row.get(0),
        )?;
        let stored = decode_turn_lifecycle(&stored, "turns")?;
        if stored != replayed {
            mismatches.push(ProjectionMismatch {
                table: "turns",
                row: turn.to_string(),
                column: "lifecycle_state",
                stored: encode_turn_lifecycle(stored).to_owned(),
                replayed: encode_turn_lifecycle(replayed).to_owned(),
            });
        }
    }
    Ok(())
}

/// Checks artifact retention against the obligations that actually pin it.
///
/// Retention is derived, never set: `ResultArtifact::retention` answers from the
/// obligations, so replay asks the same question of the folded ones. This is the
/// durable half of invariant 2 — an artifact an open obligation still needs
/// cannot be marked releasable.
fn compare_retention(
    tx: &Tx<'_>,
    by_obligation: &BTreeMap<ObligationId, Vec<LedgerEvent>>,
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    let mut folded = Vec::new();
    for (id, slice) in by_obligation {
        let identity = load::obligation_identity(tx, *id)?;
        folded.push(load::fold_obligation(*id, identity, slice)?.projection);
    }

    let mut statement = tx.conn().prepare(
        "SELECT result_artifact_id, storage_ref, sha256_hex, byte_len, created_at_ms,
                retention_state
           FROM result_artifacts ORDER BY result_artifact_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut listed = Vec::new();
    for row in rows {
        listed.push(row?);
    }

    for (id, storage_ref, digest, byte_len, created, stored) in listed {
        const TABLE: &str = "result_artifacts";
        let artifact = ResultArtifact::new(
            parse_id(&id, TABLE, "result_artifact_id")?,
            parse_token(&storage_ref, TABLE, "storage_ref")?,
            ArtifactDigest::from_bytes(parse_hex32(&digest, TABLE, "sha256_hex")?),
            parse_u64(byte_len, TABLE, "byte_len")?,
            Timestamp::from_unix_millis(created),
        );
        let replayed = match artifact.retention(&folded) {
            RetentionState::Pinned => RetentionLabel::Pinned,
            RetentionState::Eligible => RetentionLabel::Eligible,
        };
        let stored = decode_retention(&stored, TABLE)?;
        if stored != replayed {
            mismatches.push(ProjectionMismatch {
                table: TABLE,
                row: id,
                column: "retention_state",
                stored: encode_retention(stored).to_owned(),
                replayed: encode_retention(replayed).to_owned(),
            });
        }
    }
    Ok(())
}

/// Checks the health ledger against the conditions actually recorded.
///
/// Attention is ledger-derived like everything else here: every open and
/// resolved condition is reachable only through an appended event, so folding
/// those events must reproduce the `health_conditions` rows exactly.
///
/// What is compared is `(kind, scope, state)` and how many rows hold it — not
/// the condition identities. An identity is a minted opaque value nothing
/// branches on, and the fold cannot reproduce one it never saw; the semantic
/// state is what a mismatch would have to corrupt to matter.
fn compare_health(
    tx: &Tx<'_>,
    events: &[LedgerEvent],
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    const TABLE: &str = "health_conditions";
    let mut replayed: BTreeMap<String, usize> = BTreeMap::new();
    for condition in load::fold_health(events)?.conditions() {
        *replayed
            .entry(condition_key(
                condition.kind(),
                condition.scope(),
                condition.state(),
            ))
            .or_default() += 1;
    }

    let mut statement = tx.conn().prepare(
        "SELECT kind, state, task_id, turn_id, obligation_id, external_attempt_id
           FROM health_conditions",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut stored: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        let (kind, state, task, turn, obligation, attempt) = row?;
        let scope = HealthScope {
            task: task
                .map(|text| parse_id(&text, TABLE, "task_id"))
                .transpose()?,
            turn: turn
                .map(|text| parse_id(&text, TABLE, "turn_id"))
                .transpose()?,
            obligation: obligation
                .map(|text| parse_id(&text, TABLE, "obligation_id"))
                .transpose()?,
            external_attempt: attempt
                .map(|text| parse_id(&text, TABLE, "external_attempt_id"))
                .transpose()?,
        };
        // Decoded rather than string-compared, so an unknown stored label is a
        // corrupt row rather than a silent mismatch.
        let key = condition_key(
            decode_health_kind(&kind, TABLE)?,
            scope,
            decode_health_state(&state, TABLE)?,
        );
        *stored.entry(key).or_default() += 1;
    }

    let keys: BTreeSet<&String> = stored.keys().chain(replayed.keys()).collect();
    for key in keys {
        let left = stored.get(key).copied().unwrap_or_default();
        let right = replayed.get(key).copied().unwrap_or_default();
        if left != right {
            mismatches.push(ProjectionMismatch {
                table: TABLE,
                row: key.clone(),
                column: "state",
                stored: left.to_string(),
                replayed: right.to_string(),
            });
        }
    }
    Ok(())
}

/// The comparable identity of one condition: kind, scope, and state.
fn condition_key(
    kind: HealthConditionKind,
    scope: HealthScope,
    state: HealthConditionState,
) -> String {
    let part = |id: Option<String>| id.unwrap_or_else(|| "-".to_owned());
    format!(
        "{}|{}|{}|{}|{}|{}",
        encode_health_kind(kind),
        part(scope.task.map(id_text)),
        part(scope.turn.map(id_text)),
        part(scope.obligation.map(id_text)),
        part(scope.external_attempt.map(id_text)),
        encode_health_state(state),
    )
}

fn compare_deliveries(tx: &Tx<'_>, mismatches: &mut Vec<ProjectionMismatch>) -> StoreResult<usize> {
    let mut statement = tx
        .conn()
        .prepare("SELECT delivery_id, state FROM browser_deliveries ORDER BY delivery_id")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut listed = Vec::new();
    for row in rows {
        listed.push(row?);
    }

    let mut count = 0;
    for (delivery_hex, stored_state) in listed {
        let delivery_id = parse_delivery_id(&delivery_hex, "browser_deliveries", "delivery_id")?;
        let loaded = load::wake_by_delivery_id(tx, &delivery_id)?;
        count += 1;
        // The row is named by its *key*, never by its correlation ID: a
        // mismatch becomes a `RepairNeeded` message that the daemon prints to
        // stderr and writes to its log. `DeliveryKey` is the deterministic,
        // non-secret idempotency key (`governor_core::delivery`) and naming a
        // row by it grants nothing; `DeliveryId` is a possession fence that
        // `foreman_resume` accepts, and it has no `Display` for this reason.
        let row = loaded.wake.delivery_key().to_hex();
        let replayed = encode_delivery_state(loaded.wake.state());
        // Decoded, not string-compared, so an unknown stored label is a
        // corrupt row rather than a silent mismatch.
        let stored =
            encode_delivery_state(decode_delivery_state(&stored_state, "browser_deliveries")?);
        if stored != replayed {
            mismatches.push(ProjectionMismatch {
                table: "browser_deliveries",
                row: row.clone(),
                column: "state",
                stored: stored.to_owned(),
                replayed: replayed.to_owned(),
            });
        }
        compare_attempts(tx, &delivery_hex, &row, &loaded, mismatches)?;
    }
    Ok(count)
}

/// Compares one revision's attempt rows with the attempts the fold produces.
///
/// `delivery_hex` addresses the rows and never leaves this function;
/// `row` is the non-secret delivery key a reported mismatch is named by.
fn compare_attempts(
    tx: &Tx<'_>,
    delivery_hex: &str,
    row: &str,
    loaded: &load::LoadedWake,
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    let mut statement = tx.conn().prepare(
        "SELECT attempt_no, state FROM delivery_attempts
          WHERE delivery_id = ?1 ORDER BY attempt_no",
    )?;
    let rows = statement.query_map(params![delivery_hex], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut stored = BTreeMap::new();
    for row in rows {
        let (number, state) = row?;
        stored.insert(parse_u32(number, "delivery_attempts", "attempt_no")?, state);
    }

    for attempt in loaded.wake.delivery().attempts() {
        let expected = encode_attempt_state(attempt.state());
        // Decoded rather than string-compared, so an unknown stored label is a
        // corrupt row rather than a silent mismatch.
        let found = match stored.get(&attempt.number().get()) {
            Some(label) => encode_attempt_state(decode_attempt_state(label, "delivery_attempts")?),
            None => "<missing>",
        };
        if found != expected {
            mismatches.push(ProjectionMismatch {
                table: "delivery_attempts",
                row: format!("{row}#{}", attempt.number()),
                column: "state",
                stored: found.to_owned(),
                replayed: expected.to_owned(),
            });
        }
    }
    Ok(())
}
