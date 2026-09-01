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
//! [`governor_core::obligation::Obligation`] and compared field by field with
//! the stored projection row. Turn lifecycle, artifact retention, the health
//! ledger, the foreman binding ladder and the foreman claims are each derived
//! from the events too, and compared with their rows. Disagreements are
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
//!
//! Three comparisons here are narrower than a full rebuild, and the residue is
//! named rather than left implicit:
//!
//! - **Browser deliveries.** Only the delivery's state and each attempt's
//!   state are ledger-derived. The fold is seeded from the row being verified:
//!   `load::wake_by_delivery_id` takes the revision, the attempt budget, the
//!   binding generation, the target version and the accepted message ref from
//!   `browser_deliveries` before folding the attempt events. Those scheduling
//!   fields are therefore *inputs* to the replay, not outputs compared against
//!   it. What protects them instead is the row's own re-derivation on read: the
//!   loader recomputes `delivery_key` from `(obligation, generation, revision)`
//!   and refuses a row whose stored key does not match.
//! - **Foreman bindings.** The generation ladder, each generation's capability
//!   epoch and write capability, and which generation is active all rebuild
//!   from `foreman_binding_bound` and its successors — which is what invariant
//!   9's fence rests on. The binding's *target identity* does not: the
//!   canonical conversation, the browser profile, the connector ABI and the
//!   `foreman_binding_id` are not carried in allowlisted safe metadata, so
//!   there is nothing in the ledger to compare them with. `load::bindings`
//!   re-folds those rows through [`governor_core::binding::BindingLedger`] on
//!   every read that needs them.
//! - **Foreman claims.** The lifecycle, the obligation, the binding generation
//!   and the version the mint was fenced on all rebuild. `wake_delivery_id`
//!   does not, deliberately: the correlation ID is a possession fence and is
//!   never written into safe metadata. Neither does `expires_at_ms`, which is a
//!   clock reading rather than a ledger fact.

use std::collections::{BTreeMap, BTreeSet};

use governor_core::artifact::{ArtifactDigest, ResultArtifact, RetentionState};
use governor_core::binding::WriteCapabilityState;
use governor_core::fence::EventSeq;
use governor_core::health::{HealthConditionKind, HealthConditionState, HealthScope};
use governor_core::id::{ClaimId, ObligationId, SessionId};
use governor_core::session::{CommittedLoadout, LoadoutIntegrityError};
use governor_core::time::Timestamp;
use rusqlite::params;

use crate::codec::{
    ClaimLifecycle, RetentionLabel, TurnLifecycle, decode_attempt_state, decode_claim_state,
    decode_delivery_state, decode_health_kind, decode_health_state, decode_obligation_state,
    decode_retention, decode_session_relation, decode_turn_lifecycle, decode_write_capability,
    encode_attempt_state, encode_claim_state, encode_delivery_state, encode_health_kind,
    encode_health_state, encode_obligation_state, encode_retention, encode_session_relation,
    encode_turn_lifecycle, encode_write_capability, hex32, id_text, parse_delivery_id, parse_hex32,
    parse_id, parse_token, parse_u32, parse_u64,
};
use crate::error::{CorruptReason, ProjectionMismatch, RepairNeeded, StoreResult};
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
    /// Immutable loadout snapshots re-proved against their own digests.
    pub loadouts: usize,
    /// Lineage edges rebuilt from the ledger and compared.
    pub lineage_edges: usize,
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
    compare_bindings(tx, &events, &mut mismatches)?;
    compare_claims(tx, &events, &mut mismatches)?;
    let deliveries = compare_deliveries(tx, &mut mismatches)?;
    let loadouts = compare_loadouts(tx, &mut mismatches)?;
    compare_config_retention(tx, &mut mismatches)?;
    let lineage_edges = compare_lineage(tx, &events, &mut mismatches)?;

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
        loadouts,
        lineage_edges,
        verified_through,
    })
}

/// Re-proves every immutable loadout snapshot against its own recorded digest.
///
/// **This is a self-consistency check, not a ledger fold**, and the difference
/// is worth naming. A loadout's safe fields and its digest are written in one
/// transaction and are two halves of one fact; what this catches is the two
/// halves disagreeing — a row edited in place, a restore that mixed generations,
/// a widened profile written under an old loadout's digest. It does not, and
/// cannot, prove the row is one this store wrote: authenticity is the schema's
/// job, through the composite foreign keys, and the resume path's, through the
/// `ManagedConfigVerified` witness that comes from bytes the row does not
/// control.
///
/// The blast radius is deliberately the whole open: a launch snapshot that
/// disagrees with itself means the durable authority disagrees with itself, and
/// `verify` runs before anything is scheduled.
fn compare_loadouts(tx: &Tx<'_>, mismatches: &mut Vec<ProjectionMismatch>) -> StoreResult<usize> {
    const TABLE: &str = "worker_loadouts";
    let mut count = 0;
    for persisted in load::worker_loadouts(tx)? {
        count += 1;
        let row = persisted.spec.id.to_string();
        let recorded = hex32(persisted.digest.as_bytes());
        // `rehydrate` is the *only* path from persisted parts to a value that
        // can admit a resume, and it re-derives the digest rather than trusting
        // it. Calling it here is therefore the same check the resume path makes.
        // `LoadoutIntegrityError` is `#[non_exhaustive]`: a future variant is a
        // reason the row did not prove itself, and reporting it as a mismatch
        // is the fail-closed answer for every one of them.
        match CommittedLoadout::rehydrate(persisted) {
            Ok(committed) => debug_assert_eq!(hex32(committed.digest().as_bytes()), recorded),
            Err(LoadoutIntegrityError::DigestMismatch) => mismatches.push(ProjectionMismatch {
                table: TABLE,
                row,
                column: "digest_hex",
                stored: recorded,
                replayed: "<does not match the safe fields beside it>".to_owned(),
            }),
            Err(_) => mismatches.push(ProjectionMismatch {
                table: TABLE,
                row,
                column: "digest_hex",
                stored: recorded,
                replayed: "<failed its own integrity check>".to_owned(),
            }),
        }
    }
    Ok(count)
}

/// Checks that every managed configuration is still pinned.
///
/// Epoch 2 defines no releaser for a managed configuration: it is pinned by any
/// loadout any session was ever launched under, and there is no operation that
/// decides that relation has ended. So the rule is "always pinned, never a
/// deletion instant", and asserting it here is what keeps it a checked property
/// rather than an undocumented habit of the writer. The table's own CHECK
/// constraints say the same thing; this is the half that would catch a future
/// migration relaxing them without a releaser to go with it.
fn compare_config_retention(
    tx: &Tx<'_>,
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    const TABLE: &str = "managed_config_artifacts";
    let mut statement = tx.conn().prepare(
        "SELECT managed_config_artifact_id, retention_state, eligible_for_delete_at_ms
           FROM managed_config_artifacts ORDER BY managed_config_artifact_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })?;
    let mut listed = Vec::new();
    for row in rows {
        listed.push(row?);
    }
    for (id, state, deletable_at) in listed {
        let stored = decode_retention(&state, TABLE)?;
        if stored != RetentionLabel::Pinned {
            mismatches.push(ProjectionMismatch {
                table: TABLE,
                row: id.clone(),
                column: "retention_state",
                stored: encode_retention(stored).to_owned(),
                replayed: encode_retention(RetentionLabel::Pinned).to_owned(),
            });
        }
        if deletable_at.is_some() {
            mismatches.push(ProjectionMismatch {
                table: TABLE,
                row: id,
                column: "eligible_for_delete_at_ms",
                stored: "set".to_owned(),
                replayed: "null".to_owned(),
            });
        }
    }
    Ok(())
}

/// Rebuilds every lineage edge from the ledger and compares it with the rows.
///
/// A genuine fold, unlike [`compare_loadouts`]: `session_lineage_recorded`
/// carries the parent session, the parent turn and the relation in allowlisted
/// metadata, and the child is the event's own `session` scope. Every field of
/// `session_edges` is therefore ledger-derivable, so the comparison checks the
/// projection rather than restating it. Remove any one of those three metadata
/// fields and this stops being able to rebuild the edge at all.
fn compare_lineage(
    tx: &Tx<'_>,
    events: &[LedgerEvent],
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<usize> {
    const TABLE: &str = "session_edges";
    let mut replayed: BTreeMap<SessionId, load::LineageEdge> = BTreeMap::new();
    for event in events {
        if event.kind != EventKind::SessionLineageRecorded {
            continue;
        }
        let child = event.scope.session.ok_or_else(|| {
            crate::error::CorruptValue::new(
                "events",
                "session_id",
                CorruptReason::UnprovableEvidence,
            )
        })?;
        replayed.insert(
            child,
            load::LineageEdge {
                parent_session: event.metadata.id("parent_session")?,
                child_session: child,
                parent_turn: event.metadata.id("parent_turn")?,
                relation: decode_session_relation(event.metadata.label("relation")?, "events")?,
            },
        );
    }

    let stored: BTreeMap<SessionId, load::LineageEdge> = load::session_edges(tx)?
        .into_iter()
        .map(|edge| (edge.child_session, edge))
        .collect();

    let mut count = 0;
    for child in stored
        .keys()
        .chain(replayed.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        count += 1;
        let row = child.to_string();
        match (stored.get(&child), replayed.get(&child)) {
            (Some(left), Some(right)) => {
                let mut record = |column, stored: String, expected: String| {
                    if stored != expected {
                        mismatches.push(ProjectionMismatch {
                            table: TABLE,
                            row: row.clone(),
                            column,
                            stored,
                            replayed: expected,
                        });
                    }
                };
                record(
                    "parent_session_id",
                    id_text(left.parent_session),
                    id_text(right.parent_session),
                );
                record(
                    "parent_turn_id",
                    id_text(left.parent_turn),
                    id_text(right.parent_turn),
                );
                record(
                    "relation_kind",
                    encode_session_relation(left.relation).to_owned(),
                    encode_session_relation(right.relation).to_owned(),
                );
            }
            (left, right) => mismatches.push(ProjectionMismatch {
                table: TABLE,
                row,
                column: "child_session_id",
                stored: presence(left.is_some()).to_owned(),
                replayed: presence(right.is_some()).to_owned(),
            }),
        }
    }
    Ok(count)
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
        "SELECT kind, state, task_id, session_id, turn_id, obligation_id,
                external_attempt_id
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
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut stored: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        let (kind, state, task, session, turn, obligation, attempt) = row?;
        let scope = HealthScope {
            task: task
                .map(|text| parse_id(&text, TABLE, "task_id"))
                .transpose()?,
            session: session
                .map(|text| parse_id(&text, TABLE, "session_id"))
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
        "{}|{}|{}|{}|{}|{}|{}",
        encode_health_kind(kind),
        part(scope.task.map(id_text)),
        part(scope.session.map(id_text)),
        part(scope.turn.map(id_text)),
        part(scope.obligation.map(id_text)),
        part(scope.external_attempt.map(id_text)),
        encode_health_state(state),
    )
}

/// One binding generation, as the ledger determines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReplayedBinding {
    capability_epoch: u64,
    write_capability: WriteCapabilityState,
    active: bool,
}

/// Checks the binding ladder against the generations the ledger assigns.
///
/// Invariant 9 fences every wake and every claim on `binding_generation`, so
/// the ladder those rows record is load-bearing and must be derivable from the
/// events rather than trusted. It is: `BindingEvent::Bound` always takes
/// `highest + 1`, so the *n*-th `foreman_binding_bound` event owns generation
/// *n*, the last one is active, and each event's own recorded generation must
/// agree with the position replay puts it in.
///
/// What the events cannot supply is the binding's *target identity* — the
/// canonical conversation, the browser profile, the connector ABI, and the
/// `foreman_binding_id` itself. None is carried in allowlisted safe metadata,
/// so none is rebuildable here; see the module's "What is not compared, and
/// why".
fn compare_bindings(
    tx: &Tx<'_>,
    events: &[LedgerEvent],
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    const TABLE: &str = "foreman_bindings";
    let mut replayed: BTreeMap<u64, ReplayedBinding> = BTreeMap::new();
    let mut newest: Option<u64> = None;

    for event in events {
        match event.kind {
            EventKind::ForemanBindingBound => {
                let generation = u64::try_from(replayed.len())
                    .map_err(|_| corrupt(TABLE, "binding_generation"))?
                    .saturating_add(1);
                let recorded = event.metadata.u64("generation")?;
                if recorded != generation {
                    // The ledger contradicts itself: the event says it bound a
                    // generation other than the one its position assigns.
                    mismatches.push(ProjectionMismatch {
                        table: "events",
                        row: format!("foreman_binding_bound@{}", event.seq),
                        column: "generation",
                        stored: recorded.to_string(),
                        replayed: generation.to_string(),
                    });
                }
                if let Some(previous) = newest.and_then(|latest| replayed.get_mut(&latest)) {
                    previous.active = false;
                }
                replayed.insert(
                    generation,
                    ReplayedBinding {
                        capability_epoch: event.metadata.u64("capability_epoch")?,
                        write_capability: decode_write_capability(
                            event.metadata.label("write_capability")?,
                            "events",
                        )?,
                        active: true,
                    },
                );
                newest = Some(generation);
            }
            EventKind::ForemanBindingCapabilityObserved => {
                // A later observation restates the capability of the
                // generation it names, and changes nothing else.
                let generation = event.metadata.u64("generation")?;
                if let Some(binding) = replayed.get_mut(&generation) {
                    binding.capability_epoch = event.metadata.u64("capability_epoch")?;
                    binding.write_capability = decode_write_capability(
                        event.metadata.label("write_capability")?,
                        "events",
                    )?;
                }
            }
            EventKind::ForemanBindingDisplaced => {
                let generation = event.metadata.u64("generation")?;
                if let Some(binding) = replayed.get_mut(&generation) {
                    binding.active = false;
                }
            }
            _ => {}
        }
    }

    let mut statement = tx.conn().prepare(
        "SELECT binding_generation, capability_epoch, write_capability_state, is_active
           FROM foreman_bindings ORDER BY binding_generation",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut stored: BTreeMap<u64, ReplayedBinding> = BTreeMap::new();
    for row in rows {
        let (generation, epoch, capability, active) = row?;
        stored.insert(
            parse_u64(generation, TABLE, "binding_generation")?,
            ReplayedBinding {
                capability_epoch: parse_u64(epoch, TABLE, "capability_epoch")?,
                write_capability: decode_write_capability(&capability, TABLE)?,
                active: active != 0,
            },
        );
    }

    for generation in stored
        .keys()
        .chain(replayed.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let row = format!("generation {generation}");
        match (stored.get(&generation), replayed.get(&generation)) {
            (Some(left), Some(right)) => {
                let mut record = |column, stored: String, expected: String| {
                    if stored != expected {
                        mismatches.push(ProjectionMismatch {
                            table: TABLE,
                            row: row.clone(),
                            column,
                            stored,
                            replayed: expected,
                        });
                    }
                };
                record(
                    "capability_epoch",
                    left.capability_epoch.to_string(),
                    right.capability_epoch.to_string(),
                );
                record(
                    "write_capability_state",
                    encode_write_capability(left.write_capability, TABLE)?.to_owned(),
                    encode_write_capability(right.write_capability, TABLE)?.to_owned(),
                );
                record(
                    "is_active",
                    left.active.to_string(),
                    right.active.to_string(),
                );
            }
            (left, right) => mismatches.push(ProjectionMismatch {
                table: TABLE,
                row,
                column: "binding_generation",
                stored: presence(left.is_some()).to_owned(),
                replayed: presence(right.is_some()).to_owned(),
            }),
        }
    }
    Ok(())
}

/// One foreman claim, as far as the ledger determines it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayedClaim {
    obligation: Option<ObligationId>,
    version_at_claim: u64,
    binding_generation: u64,
    state: ClaimLifecycle,
}

/// Checks the claim rows against the claim events.
///
/// A claim is minted `live`, an expiry moves it to `expired`, and the ACK that
/// closes its obligation moves it to `closed`; a handoff changes no lifecycle.
/// Every one of those events names its `claim_id` in allowlisted metadata and
/// its obligation in the event's scope, so the lifecycle, the obligation, the
/// binding generation and the version the mint was fenced on all rebuild.
///
/// Two row fields cannot: `wake_delivery_id`, because the correlation ID is a
/// possession fence that is deliberately never written into safe metadata, and
/// `expires_at_ms`, because it is a clock reading the ledger does not record.
/// See the module's "What is not compared, and why".
fn compare_claims(
    tx: &Tx<'_>,
    events: &[LedgerEvent],
    mismatches: &mut Vec<ProjectionMismatch>,
) -> StoreResult<()> {
    const TABLE: &str = "foreman_claims";
    let mut replayed: BTreeMap<ClaimId, ReplayedClaim> = BTreeMap::new();

    for event in events {
        match event.kind {
            EventKind::ForemanClaimMinted => {
                replayed.insert(
                    event.metadata.id("claim_id")?,
                    ReplayedClaim {
                        obligation: event.scope.obligation,
                        version_at_claim: event.metadata.u64("expected_version")?,
                        binding_generation: event.metadata.u64("binding_generation")?,
                        state: ClaimLifecycle::Live,
                    },
                );
            }
            EventKind::ForemanClaimExpired => {
                if let Some(claim) = replayed.get_mut(&event.metadata.id("claim_id")?) {
                    claim.state = ClaimLifecycle::Expired;
                }
            }
            EventKind::ForemanAcked => {
                if let Some(claim) = replayed.get_mut(&event.metadata.id("claim_id")?) {
                    claim.state = ClaimLifecycle::Closed;
                }
            }
            _ => {}
        }
    }

    let mut statement = tx.conn().prepare(
        "SELECT claim_id, obligation_id, obligation_version_at_claim, binding_generation, state
           FROM foreman_claims ORDER BY claim_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut stored: BTreeMap<ClaimId, ReplayedClaim> = BTreeMap::new();
    for row in rows {
        let (claim, obligation, version, generation, state) = row?;
        stored.insert(
            parse_id(&claim, TABLE, "claim_id")?,
            ReplayedClaim {
                obligation: Some(parse_id(&obligation, TABLE, "obligation_id")?),
                version_at_claim: parse_u64(version, TABLE, "obligation_version_at_claim")?,
                binding_generation: parse_u64(generation, TABLE, "binding_generation")?,
                state: decode_claim_state(&state, TABLE)?,
            },
        );
    }

    for claim in stored
        .keys()
        .chain(replayed.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let row = claim.to_string();
        match (stored.get(&claim), replayed.get(&claim)) {
            (Some(left), Some(right)) => {
                let mut record = |column, stored: String, expected: String| {
                    if stored != expected {
                        mismatches.push(ProjectionMismatch {
                            table: TABLE,
                            row: row.clone(),
                            column,
                            stored,
                            replayed: expected,
                        });
                    }
                };
                record(
                    "obligation_id",
                    left.obligation.map(id_text).unwrap_or_default(),
                    right.obligation.map(id_text).unwrap_or_default(),
                );
                record(
                    "obligation_version_at_claim",
                    left.version_at_claim.to_string(),
                    right.version_at_claim.to_string(),
                );
                record(
                    "binding_generation",
                    left.binding_generation.to_string(),
                    right.binding_generation.to_string(),
                );
                record(
                    "state",
                    encode_claim_state(left.state).to_owned(),
                    encode_claim_state(right.state).to_owned(),
                );
            }
            (left, right) => mismatches.push(ProjectionMismatch {
                table: TABLE,
                row,
                column: "claim_id",
                stored: presence(left.is_some()).to_owned(),
                replayed: presence(right.is_some()).to_owned(),
            }),
        }
    }
    Ok(())
}

/// How a presence disagreement is rendered on both sides of a comparison.
const fn presence(found: bool) -> &'static str {
    if found { "present" } else { "absent" }
}

/// A corrupt-row error for a value this module could not narrow.
fn corrupt(table: &'static str, column: &'static str) -> crate::error::StoreError {
    crate::error::CorruptValue::new(
        table,
        column,
        crate::error::CorruptReason::IntegerOutOfRange,
    )
    .into()
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
