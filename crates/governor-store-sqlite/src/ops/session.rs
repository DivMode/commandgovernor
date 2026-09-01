//! Immutable worker loadouts, their session bindings, and durable lineage.
//!
//! This is the durable half of [`governor_core::session`], which says so in its
//! own module docs: *the store must commit the logical session, lineage,
//! loadout and external-spawn intent before an adapter receives permission to
//! create the process*. Everything below exists to make that ordering
//! structural rather than remembered.
//!
//! # Immutability is the primary key, not a rule
//!
//! `capability_profiles`, `delegation_policies`, `model_policies` and
//! `worker_loadouts` are all keyed on `(identity, digest_hex)`, and this module
//! contains no `UPDATE` statement for any of them. Editing a role file
//! therefore *inserts a second snapshot*, and the composite foreign keys mean
//! every loadout that embedded the first one still resolves to the first one.
//! A resume presenting the widened fence is refused by
//! [`governor_core::session::CommittedLoadout::admit_resume`], and a resume
//! presenting the original fence reads back the original capability set.
//!
//! # Worker spawn is a non-idempotent write with no idempotency contract
//!
//! [`AuthorizeWorkerSpawn`] records its intent as
//! [`ExternalEffectClass::NonIdempotentWrite`] and supplies no idempotency
//! contract, deliberately. Starting a process is not deduplicated by any key
//! the destination honours, so
//! [`governor_core::effect::ExternalAttempt::admit_retry`] refuses an automatic
//! retry with
//! [`Conflict::RetryRequiresIdempotencyContract`](governor_core::error::Conflict::RetryRequiresIdempotencyContract).
//!
//! What that means operationally, and it is the whole point of classifying it
//! this way: a spawn intent that startup quarantines means **a worker process
//! may exist**. The answer is reconciliation — find the process, or start a new
//! logical incarnation with its own loadout binding — and never a silent
//! respawn under the old intent. There is no operation here that resolves a
//! quarantined spawn into a second permit, and there is no code path that could
//! produce one.
//!
//! # Two guards run inside the edge-insert transaction, in this order
//!
//! 1. [`require_parent_turn_ownership`] — `turns` records the *incarnation*,
//!    not the session, so a turn's session is derived through a two-hop join
//!    the store performs. The caller never supplies it.
//! 2. [`require_no_lineage_cycle`] — [`SessionEdge::new`] refuses the one-hop
//!    self-parent case; the multi-hop case is a property of the whole durable
//!    graph and can only be seen under the write lock the insert already holds.
//!
//! Only then is [`SessionEdge::new`] called, which re-proves the self-parent
//! case purely, and only then is anything appended or inserted.

use std::collections::BTreeSet;

use governor_core::effect::{DestinationRef, ExternalAttempt, ExternalEffectClass, RecordedIntent};
use governor_core::error::Conflict;
use governor_core::fence::{DaemonEpoch, SafeToken, SourceRef};
use governor_core::id::{
    CapabilityProfileId, DelegationPolicyId, EventId, ExternalAttemptId, ManagedConfigArtifactId,
    SessionId, SessionIncarnationId, TurnId, WorkerLoadoutId,
};
use governor_core::session::{
    CapabilityName, CapabilityProfile, DelegationPolicy, HookContractEpoch, ManagedConfigDigest,
    ManagedConfigRef, ModelPolicyRef, ResumePolicy, RuntimeKind, SessionEdge, SessionRelation,
    WorkerKind, WorkerLoadout, WorkerLoadoutDigest, WorkerLoadoutFence, WorkerLoadoutSpec,
    WorkerRole,
};
use governor_core::time::Timestamp;
use rusqlite::{OptionalExtension as _, params};

use crate::codec::{
    encode_resume_policy, encode_session_relation, hex32, id_text, parse_id, store_time, store_u64,
};
use crate::error::{CorruptReason, CorruptValue, StoreResult};
use crate::event::{self, EventKind, EventScope, NewEvent};
use crate::ops::effect::{GrantedPermit, grant, insert_intent};
use crate::ops::worker::DurableArtifact;
use crate::ops::{AttemptEvidence, internal_source, internal_source_text};
use crate::ports::StorePorts;
use crate::safe_metadata::SafeMetadata;
use crate::tx::{Failpoint, Tx, WriteOp};

/// Longest ancestor chain a lineage walk will follow before refusing.
///
/// `PRIMARY KEY(child_session_id)` makes the parent relation a chain, so a
/// healthy graph terminates long before this. The bound exists for the
/// *unhealthy* one: SQLite's recursive CTE has no cycle detection of its own,
/// and a database restored from backup is not a proof that no cycle is already
/// in the table. Without the bound such a row would make the walk loop forever;
/// with it, the walk becomes a typed refusal.
pub(crate) const MAX_LINEAGE_DEPTH: u32 = 64;

// --- Managed configuration artifacts -----------------------------------------

/// A private immutable managed configuration whose bytes are already durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordManagedConfigRequest {
    /// The configuration the artifact layer already made durable.
    ///
    /// The same [`DurableArtifact`] seam a result publication uses: its only
    /// constructor is the assertion that temp → write → `fsync` → link →
    /// directory `fsync` → verify completed for these exact bytes, so a
    /// configuration row pointing at bytes that were never made durable is not
    /// reachable through this API.
    pub artifact: DurableArtifact,
    /// Hook/configuration contract epoch these bytes implement.
    pub hook_contract_epoch: HookContractEpoch,
}

/// The configuration reference a loadout may now embed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedManagedConfig {
    /// Identity, digest and exact length of the recorded configuration.
    pub reference: ManagedConfigRef,
    /// Whether the source identity was already in the ledger, so this call
    /// changed nothing.
    pub duplicate: bool,
}

/// Commits one managed-configuration metadata row.
pub(crate) struct RecordManagedConfig {
    request: RecordManagedConfigRequest,
    artifact: ManagedConfigArtifactId,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RecordManagedConfig {
    type Request = RecordManagedConfigRequest;
    type Committed = RecordedManagedConfig;
    type Output = RecordedManagedConfig;

    const NAME: &'static str = "record_managed_config";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            artifact: ports.next_id(),
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let digest =
            ManagedConfigDigest::from_persisted(*self.request.artifact.digest().as_bytes());
        let reference =
            ManagedConfigRef::new(self.artifact, digest, self.request.artifact.byte_len());

        // Deterministic in the storage key, which the artifact layer allocated
        // exactly once: recording the same published bytes twice converges on
        // the first row instead of minting a second configuration identity for
        // one file.
        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::ManagedConfigRecorded,
                source: internal_source_text(
                    self.request.artifact.storage_ref().as_str(),
                    "managed_config_recorded",
                )?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope::default(),
                metadata: SafeMetadata::new()
                    .id("managed_config", self.artifact)
                    .token("digest", &digest_token(digest.as_bytes())?)
                    .int(
                        "hook_contract_epoch",
                        store_u64(
                            self.request.hook_contract_epoch.get(),
                            "events",
                            "hook_contract_epoch",
                        )?,
                    ),
            },
        )?;
        if appended.is_duplicate() {
            let existing = existing_config_by_storage_ref(tx, self.request.artifact.storage_ref())?;
            return Ok(RecordedManagedConfig {
                reference: existing,
                duplicate: true,
            });
        }

        tx.conn().execute(
            "INSERT INTO managed_config_artifacts (managed_config_artifact_id, storage_ref,
                    sha256_hex, byte_len, media_type, hook_contract_epoch, created_at_ms,
                    created_event_seq, retention_state, eligible_for_delete_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pinned', NULL)",
            params![
                id_text(self.artifact),
                self.request.artifact.storage_ref().as_str(),
                hex32(self.request.artifact.digest().as_bytes()),
                store_u64(
                    self.request.artifact.byte_len(),
                    "managed_config_artifacts",
                    "byte_len"
                )?,
                self.request.artifact.media_type().as_str(),
                store_u64(
                    self.request.hook_contract_epoch.get(),
                    "managed_config_artifacts",
                    "hook_contract_epoch"
                )?,
                store_time(self.now),
                event::store_seq(appended.seq())?,
            ],
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(RecordedManagedConfig {
            reference,
            duplicate: false,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

/// Reads back the configuration a duplicate publication already recorded.
fn existing_config_by_storage_ref(
    tx: &Tx<'_>,
    storage_ref: &SafeToken,
) -> StoreResult<ManagedConfigRef> {
    const TABLE: &str = "managed_config_artifacts";
    let row: Option<(String, String, i64)> = tx
        .conn()
        .query_row(
            "SELECT managed_config_artifact_id, sha256_hex, byte_len
               FROM managed_config_artifacts WHERE storage_ref = ?1",
            params![storage_ref.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (id, digest, byte_len) = row
        .ok_or_else(|| CorruptValue::new(TABLE, "storage_ref", CorruptReason::DanglingReference))?;
    Ok(ManagedConfigRef::new(
        parse_id(&id, TABLE, "managed_config_artifact_id")?,
        ManagedConfigDigest::from_persisted(crate::codec::parse_hex32(
            &digest,
            TABLE,
            "sha256_hex",
        )?),
        crate::codec::parse_u64(byte_len, TABLE, "byte_len")?,
    ))
}

// --- Resolved worker loadouts -------------------------------------------------

/// One fully resolved launch loadout, ready to be written down.
///
/// The identities are the caller's: a capability profile, a delegation policy
/// and a loadout all have stable configuration identities that outlive any one
/// snapshot of their contents. What the *store* refuses to let the caller
/// control is the digest, which is derived here from the parts.
///
/// The whitelists arrive as their *contents* rather than as an already-resolved
/// [`CapabilityProfile`] on purpose. The store has to write one row per granted
/// name, and `governor-core` publishes no iterator over a resolved profile — so
/// taking a resolved value would have meant taking the names alongside it, and
/// two copies of one set are two things that can disagree. Building the profile
/// here from the names keeps the digest a function of the rows that are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveWorkerLoadoutRequest {
    /// Stable identity of the logical loadout.
    pub loadout: WorkerLoadoutId,
    /// Worker adapter class, e.g. `claude`.
    pub worker_kind: WorkerKind,
    /// Runtime adapter class, e.g. `herdr`.
    pub runtime_kind: RuntimeKind,
    /// Semantic role used for policy and analytics.
    pub role: WorkerRole,
    /// Immutable model-policy snapshot, resolved outside this crate.
    pub model_policy: ModelPolicyRef,
    /// Stable identity of the capability whitelist.
    pub capability_profile: CapabilityProfileId,
    /// Every explicitly granted capability. An empty list grants nothing.
    pub capabilities: Vec<CapabilityName>,
    /// Stable identity of the recursive-delegation whitelist.
    pub delegation_policy: DelegationPolicyId,
    /// Every role this worker may delegate to. An empty list permits none.
    pub delegated_roles: Vec<WorkerRole>,
    /// The private immutable managed launch configuration.
    pub managed_config: ManagedConfigRef,
    /// Hook/configuration contract the adapter expects.
    pub hook_contract_epoch: HookContractEpoch,
    /// Resume policy applied to sessions launched under this loadout.
    pub resume_policy: ResumePolicy,
}

/// The immutable snapshot a resolution committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLoadout {
    /// Stable loadout identity.
    pub loadout: WorkerLoadoutId,
    /// Canonical digest of the resolved contents.
    pub digest: WorkerLoadoutDigest,
    /// Whether this exact snapshot was already recorded.
    pub duplicate: bool,
}

impl ResolvedLoadout {
    /// The exact identity+digest pair a resume must present.
    #[must_use]
    pub const fn fence(&self) -> WorkerLoadoutFence {
        WorkerLoadoutFence::new(self.loadout, self.digest)
    }
}

/// Commits one capability profile, delegation policy, model policy and loadout.
pub(crate) struct ResolveWorkerLoadout {
    request: ResolveWorkerLoadoutRequest,
    /// The deduplicated whitelists, which are both what is hashed and what is
    /// written. `BTreeSet` rather than the request's `Vec`: a repeated name is
    /// one grant, and the entry table's primary key says so too.
    capabilities: BTreeSet<CapabilityName>,
    delegated_roles: BTreeSet<WorkerRole>,
    profile: CapabilityProfile,
    policy: DelegationPolicy,
    resolved: WorkerLoadout,
    events: [EventId; 4],
    now: Timestamp,
}

impl WriteOp for ResolveWorkerLoadout {
    type Request = ResolveWorkerLoadoutRequest;
    type Committed = ResolvedLoadout;
    type Output = ResolvedLoadout;

    const NAME: &'static str = "resolve_worker_loadout";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        // Pure, and deliberately here rather than in `commit`: the digests are
        // a function of the parts alone, and deriving them outside the
        // transaction keeps the body a compare-then-mutate with nothing left to
        // compute.
        let capabilities: BTreeSet<CapabilityName> = request.capabilities.iter().cloned().collect();
        let delegated_roles: BTreeSet<WorkerRole> =
            request.delegated_roles.iter().cloned().collect();
        let profile = CapabilityProfile::new(request.capability_profile, capabilities.clone());
        let policy = DelegationPolicy::new(request.delegation_policy, delegated_roles.clone());
        let resolved = WorkerLoadout::resolve(WorkerLoadoutSpec {
            id: request.loadout,
            worker_kind: request.worker_kind.clone(),
            runtime_kind: request.runtime_kind.clone(),
            role: request.role.clone(),
            model_policy: request.model_policy,
            capability_profile: profile.reference(),
            delegation_policy: policy.reference(),
            managed_config: request.managed_config,
            hook_contract_epoch: request.hook_contract_epoch,
            resume_policy: request.resume_policy,
        });
        Ok(Self {
            request,
            capabilities,
            delegated_roles,
            profile,
            policy,
            resolved,
            events: [
                ports.next_id(),
                ports.next_id(),
                ports.next_id(),
                ports.next_id(),
            ],
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        self.commit_capability_profile(tx)?;
        self.commit_delegation_policy(tx)?;
        self.commit_model_policy(tx)?;
        let duplicate = self.commit_loadout(tx)?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(ResolvedLoadout {
            loadout: self.request.loadout,
            digest: self.resolved.digest(),
            duplicate,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

impl ResolveWorkerLoadout {
    fn commit_capability_profile(&self, tx: &Tx<'_>) -> StoreResult<()> {
        let profile = &self.profile;
        let digest = hex32(profile.digest().as_bytes());
        let appended = self.append(
            tx,
            0,
            EventKind::CapabilityProfileRecorded,
            &snapshot_source(
                &id_text(profile.id()),
                &digest,
                "capability_profile_recorded",
            )?,
            SafeMetadata::new()
                .id("capability_profile", profile.id())
                .token("digest", &digest_token(profile.digest().as_bytes())?)
                .int("entry_count", count_of(profile.len(), "entry_count")?),
        )?;
        if appended.is_duplicate() {
            return Ok(());
        }
        tx.conn().execute(
            "INSERT INTO capability_profiles (capability_profile_id, digest_hex,
                    capability_count, created_event_seq)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                id_text(profile.id()),
                digest,
                count_of(profile.len(), "capability_count")?,
                event::store_seq(appended.seq())?,
            ],
        )?;
        for name in &self.capabilities {
            tx.conn().execute(
                "INSERT INTO capability_profile_entries (capability_profile_id, digest_hex,
                        capability_name)
                 VALUES (?1, ?2, ?3)",
                params![id_text(profile.id()), digest, name.as_token().as_str()],
            )?;
        }
        Ok(())
    }

    fn commit_delegation_policy(&self, tx: &Tx<'_>) -> StoreResult<()> {
        let policy = &self.policy;
        let digest = hex32(policy.digest().as_bytes());
        let appended = self.append(
            tx,
            1,
            EventKind::DelegationPolicyRecorded,
            &snapshot_source(&id_text(policy.id()), &digest, "delegation_policy_recorded")?,
            SafeMetadata::new()
                .id("delegation_policy", policy.id())
                .token("digest", &digest_token(policy.digest().as_bytes())?)
                .int("entry_count", count_of(policy.len(), "entry_count")?),
        )?;
        if appended.is_duplicate() {
            return Ok(());
        }
        tx.conn().execute(
            "INSERT INTO delegation_policies (delegation_policy_id, digest_hex,
                    allowed_role_count, created_event_seq)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                id_text(policy.id()),
                digest,
                count_of(policy.len(), "allowed_role_count")?,
                event::store_seq(appended.seq())?,
            ],
        )?;
        for role in &self.delegated_roles {
            tx.conn().execute(
                "INSERT INTO delegation_policy_entries (delegation_policy_id, digest_hex,
                        allowed_role)
                 VALUES (?1, ?2, ?3)",
                params![id_text(policy.id()), digest, role.as_token().as_str()],
            )?;
        }
        Ok(())
    }

    fn commit_model_policy(&self, tx: &Tx<'_>) -> StoreResult<()> {
        let policy = self.request.model_policy;
        let digest = hex32(policy.digest().as_bytes());
        let appended = self.append(
            tx,
            2,
            EventKind::ModelPolicyRecorded,
            &snapshot_source(&id_text(policy.id()), &digest, "model_policy_recorded")?,
            SafeMetadata::new()
                .id("model_policy", policy.id())
                .token("digest", &digest_token(policy.digest().as_bytes())?),
        )?;
        if appended.is_duplicate() {
            return Ok(());
        }
        tx.conn().execute(
            "INSERT INTO model_policies (model_policy_id, digest_hex, created_event_seq)
             VALUES (?1, ?2, ?3)",
            params![
                id_text(policy.id()),
                digest,
                event::store_seq(appended.seq())?
            ],
        )?;
        Ok(())
    }

    fn commit_loadout(&self, tx: &Tx<'_>) -> StoreResult<bool> {
        let spec = self.resolved.spec();
        let digest = hex32(self.resolved.digest().as_bytes());
        let appended = self.append(
            tx,
            3,
            EventKind::WorkerLoadoutResolved,
            &snapshot_source(&id_text(spec.id), &digest, "worker_loadout_resolved")?,
            // Identities and the loadout digest only. `role`, `worker_kind` and
            // `runtime_kind` stay out on purpose: each is an opaque token that
            // belongs in exactly one column, and a second copy in metadata
            // would be a second place for it to leak from.
            SafeMetadata::new()
                .id("loadout_id", spec.id)
                .token("digest", &digest_token(self.resolved.digest().as_bytes())?)
                .id("capability_profile", spec.capability_profile.id())
                .id("delegation_policy", spec.delegation_policy.id())
                .id("model_policy", spec.model_policy.id())
                .id("managed_config", spec.managed_config.id()),
        )?;
        if appended.is_duplicate() {
            return Ok(true);
        }
        tx.conn().execute(
            "INSERT INTO worker_loadouts (worker_loadout_id, digest_hex, worker_kind,
                    runtime_kind, role, model_policy_id, model_policy_digest_hex,
                    capability_profile_id, capability_profile_digest_hex,
                    delegation_policy_id, delegation_policy_digest_hex,
                    managed_config_artifact_id, managed_config_digest_hex,
                    managed_config_byte_len, hook_contract_epoch, resume_policy,
                    created_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                id_text(spec.id),
                digest,
                spec.worker_kind.as_token().as_str(),
                spec.runtime_kind.as_token().as_str(),
                spec.role.as_token().as_str(),
                id_text(spec.model_policy.id()),
                hex32(spec.model_policy.digest().as_bytes()),
                id_text(spec.capability_profile.id()),
                hex32(spec.capability_profile.digest().as_bytes()),
                id_text(spec.delegation_policy.id()),
                hex32(spec.delegation_policy.digest().as_bytes()),
                id_text(spec.managed_config.id()),
                hex32(spec.managed_config.digest().as_bytes()),
                store_u64(
                    spec.managed_config.byte_len(),
                    "worker_loadouts",
                    "managed_config_byte_len"
                )?,
                store_u64(
                    spec.hook_contract_epoch.get(),
                    "worker_loadouts",
                    "hook_contract_epoch"
                )?,
                encode_resume_policy(spec.resume_policy),
                event::store_seq(appended.seq())?,
            ],
        )?;
        Ok(false)
    }

    fn append(
        &self,
        tx: &Tx<'_>,
        index: usize,
        kind: EventKind,
        source: &SourceRef,
        metadata: SafeMetadata,
    ) -> StoreResult<crate::event::Appended> {
        event::append(
            tx,
            &NewEvent {
                event_id: self.events[index],
                kind,
                source: source.clone(),
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope::default(),
                metadata,
            },
        )
    }
}

// --- Binding a loadout to one session incarnation -----------------------------

/// The launch snapshot one session incarnation runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindSessionLoadoutRequest {
    /// Logical session being launched.
    pub session: SessionId,
    /// The incarnation this binding is for. One loadout per incarnation.
    pub incarnation: SessionIncarnationId,
    /// Exact identity+digest of the resolved loadout.
    pub loadout: WorkerLoadoutFence,
}

/// What a binding committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundSessionLoadout {
    /// The bound session.
    pub session: SessionId,
    /// The bound incarnation.
    pub incarnation: SessionIncarnationId,
    /// The launch snapshot every later resume is fenced against.
    pub loadout: WorkerLoadoutFence,
    /// Whether the identical binding was already recorded.
    pub duplicate: bool,
}

/// Binds one incarnation to one immutable loadout snapshot, forever.
pub(crate) struct BindSessionLoadout {
    request: BindSessionLoadoutRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for BindSessionLoadout {
    type Request = BindSessionLoadoutRequest;
    type Committed = BoundSessionLoadout;
    type Output = BoundSessionLoadout;

    const NAME: &'static str = "bind_session_loadout";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        let bound = |duplicate| BoundSessionLoadout {
            session: self.request.session,
            incarnation: self.request.incarnation,
            loadout: self.request.loadout,
            duplicate,
        };
        require_incarnation_ownership(tx, self.request.session, self.request.incarnation)?;
        require_loadout_snapshot(tx, self.request.loadout)?;

        // One loadout per incarnation. A rebinding to the *same* snapshot is
        // convergence; a rebinding to a different one is refused, because
        // widening a live session's sandbox is exactly what the fence exists to
        // prevent — a new revision needs a new incarnation.
        if let Some(existing) = read_binding(tx, self.request.incarnation)? {
            if existing == self.request.loadout {
                return Ok(bound(true));
            }
            return Err(Conflict::SessionIncarnationAlreadyBound {
                incarnation: self.request.incarnation,
            }
            .into());
        }

        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::SessionLoadoutBound,
                source: internal_source(self.request.incarnation, "session_loadout_bound")?,
                observed_at: self.now,
                occurred_at: None,
                scope: EventScope {
                    session: Some(self.request.session),
                    incarnation: Some(self.request.incarnation),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .id("loadout_id", self.request.loadout.id())
                    .token(
                        "digest",
                        &digest_token(self.request.loadout.digest().as_bytes())?,
                    ),
            },
        )?;
        if appended.is_duplicate() {
            return Ok(bound(true));
        }

        tx.conn().execute(
            "INSERT INTO session_loadouts (session_id, session_incarnation_id,
                    worker_loadout_id, digest_hex, bound_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id_text(self.request.session),
                id_text(self.request.incarnation),
                id_text(self.request.loadout.id()),
                hex32(self.request.loadout.digest().as_bytes()),
                event::store_seq(appended.seq())?,
            ],
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(bound(false))
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

// --- Durable session lineage --------------------------------------------------

/// One delegation or fork, as durable provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordSessionLineageRequest {
    /// Logical parent session.
    pub parent_session: SessionId,
    /// Logical child session.
    pub child_session: SessionId,
    /// The parent turn that created the delegation or fork.
    ///
    /// Never trusted: the store derives the turn's owning session itself.
    pub parent_turn: TurnId,
    /// Semantic child relationship.
    pub relation: SessionRelation,
}

/// What a lineage record committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedLineage {
    /// The durable edge.
    pub edge: SessionEdge,
    /// Whether the identical edge was already recorded.
    pub duplicate: bool,
}

/// Records one parent/child lineage edge, cycle-free and ownership-proved.
pub(crate) struct RecordSessionLineage {
    request: RecordSessionLineageRequest,
    event: EventId,
    now: Timestamp,
}

impl WriteOp for RecordSessionLineage {
    type Request = RecordSessionLineageRequest;
    type Committed = RecordedLineage;
    type Output = RecordedLineage;

    const NAME: &'static str = "record_session_lineage";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        Ok(Self {
            request,
            event: ports.next_id(),
            now: ports.now(),
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        // Ownership first, then the cycle walk, then the pure constructor.
        // Ownership first because a caller that presented somebody else's turn
        // has already failed, and walking the graph for it would be answering a
        // question about a relationship that does not exist.
        require_parent_turn_ownership(tx, self.request.parent_session, self.request.parent_turn)?;
        require_no_lineage_cycle(tx, self.request.parent_session, self.request.child_session)?;
        let edge = SessionEdge::new(
            self.request.parent_session,
            self.request.child_session,
            self.request.parent_turn,
            self.request.relation,
        )
        .map_err(|_| Conflict::SessionLineageCycle {
            parent: self.request.parent_session,
            child: self.request.child_session,
        })?;

        let appended = event::append(
            tx,
            &NewEvent {
                event_id: self.event,
                kind: EventKind::SessionLineageRecorded,
                source: internal_source(self.request.child_session, "session_lineage_recorded")?,
                observed_at: self.now,
                occurred_at: None,
                // The event's own session scope is the *child*: an edge is the
                // fact that establishes the child's provenance, and scoping it
                // to the parent would make one parent's slice grow without
                // bound while the child's said nothing about where it came
                // from. The parent identity travels in allowlisted metadata,
                // which is what makes `replay::compare_lineage` a genuine fold
                // rather than a re-read of the row it is checking.
                scope: EventScope {
                    session: Some(self.request.child_session),
                    ..EventScope::default()
                },
                metadata: SafeMetadata::new()
                    .id("parent_session", self.request.parent_session)
                    .id("parent_turn", self.request.parent_turn)
                    .label("relation", encode_session_relation(self.request.relation)),
            },
        )?;
        if appended.is_duplicate() {
            return Ok(RecordedLineage {
                edge,
                duplicate: true,
            });
        }

        tx.conn().execute(
            "INSERT INTO session_edges (parent_session_id, child_session_id, parent_turn_id,
                    relation_kind, created_event_seq)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id_text(edge.parent_session()),
                id_text(edge.child_session()),
                id_text(edge.parent_turn()),
                encode_session_relation(edge.relation()),
                event::store_seq(appended.seq())?,
            ],
        )?;

        tx.reach(Failpoint::AfterProjectionUpdate)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(RecordedLineage {
            edge,
            duplicate: false,
        })
    }

    fn finish(self, committed: Self::Committed) -> Self::Output {
        committed
    }
}

// --- Spawn authorization ------------------------------------------------------

/// A worker process the daemon is about to create or resume.
///
/// The two `verified_*` fields carry what the composition root proved *outside*
/// any transaction: it read the persisted loadout row, re-derived its digest
/// through [`governor_core::session::CommittedLoadout::rehydrate`], read the
/// managed configuration's bytes back from the artifact root and re-hashed
/// them. Steps 5 to 7 of that sequence happen inside this operation's
/// transaction, under the write lock, and refuse on any difference — which is
/// what makes the outside-the-transaction verification sound rather than
/// merely optimistic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeWorkerSpawnRequest {
    /// Logical session being launched or resumed.
    pub session: SessionId,
    /// Incarnation whose bound loadout was verified.
    pub incarnation: SessionIncarnationId,
    /// The loadout fence the caller re-proved from the persisted row.
    pub verified_loadout: WorkerLoadoutFence,
    /// The configuration reference the caller re-proved from the bytes.
    pub verified_config: ManagedConfigRef,
    /// Opaque destination the spawn targets.
    pub destination: DestinationRef,
    /// The source fact that justifies the spawn.
    pub source: SourceRef,
    /// Daemon epoch the intent is recorded under.
    pub daemon_epoch: DaemonEpoch,
}

/// Commits one spawn intent, then surrenders one permit.
pub(crate) struct AuthorizeWorkerSpawn {
    request: AuthorizeWorkerSpawnRequest,
    attempt: ExternalAttemptId,
    recorded: RecordedIntent<AttemptEvidence>,
}

impl WriteOp for AuthorizeWorkerSpawn {
    type Request = AuthorizeWorkerSpawnRequest;
    type Committed = ();
    type Output = GrantedPermit;

    const NAME: &'static str = "authorize_worker_spawn";

    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self> {
        let attempt: ExternalAttemptId = ports.next_id();
        let recorded = ExternalAttempt::<AttemptEvidence>::record_intent(
            attempt,
            // No idempotency contract, deliberately: see the module docs.
            ExternalEffectClass::NonIdempotentWrite,
            request.destination.clone(),
            request.source.clone(),
            request.daemon_epoch,
            ports.now(),
        );
        Ok(Self {
            request,
            attempt,
            recorded,
        })
    }

    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed> {
        // Step 5: the binding, re-read under the write lock. Byte-identical or
        // nothing — this is the whole reason the outside-the-transaction
        // verification is allowed to happen at all.
        let bound =
            read_binding(tx, self.request.incarnation)?.ok_or(Conflict::NoSessionLoadout {
                incarnation: self.request.incarnation,
            })?;
        require_incarnation_ownership(tx, self.request.session, self.request.incarnation)?;
        if bound.id() != self.request.verified_loadout.id() {
            return Err(Conflict::LoadoutIdentityMismatch {
                presented: self.request.verified_loadout.id(),
                expected: bound.id(),
            }
            .into());
        }
        if bound.digest() != self.request.verified_loadout.digest() {
            return Err(Conflict::LoadoutDigestMismatch {
                loadout: bound.id(),
            }
            .into());
        }

        // Step 6: the configuration metadata the loadout embeds, and then the
        // configuration row itself. No file is touched here; the bytes were
        // proved outside, and what is checked is that the durable authority
        // still describes the same bytes.
        let embedded = read_loadout_config(tx, bound)?;
        require_same_config(bound.id(), embedded, self.request.verified_config)?;
        let recorded = read_managed_config(tx, embedded.id())?;
        require_same_config(bound.id(), recorded, self.request.verified_config)?;

        // Step 8: the durable intent, alone and first.
        insert_intent(
            tx,
            self.attempt,
            &ExternalEffectClass::NonIdempotentWrite,
            &self.request.destination,
            &self.request.source,
            self.request.daemon_epoch,
            self.recorded.attempt().recorded_at(),
        )?;
        tx.reach(Failpoint::AfterIntentInsert)?;
        tx.reach(Failpoint::BeforeCommit)?;
        Ok(())
    }

    fn finish(self, (): Self::Committed) -> Self::Output {
        // Strictly after `COMMIT`. The permit does not exist before this line.
        grant(self.attempt, self.recorded)
    }
}

// --- In-transaction guards ----------------------------------------------------

/// Proves the presented parent turn actually belongs to the presented session.
///
/// The join is `turns -> session_incarnations -> sessions`, because `turns`
/// records the incarnation rather than the session: a turn's session is
/// derived, never presented. The foreign key on `session_edges.parent_turn_id`
/// proves only that the turn exists, which any turn of any session satisfies.
fn require_parent_turn_ownership(
    tx: &Tx<'_>,
    parent_session: SessionId,
    parent_turn: TurnId,
) -> StoreResult<()> {
    let refused = || -> crate::error::StoreError {
        // An unknown turn identity is reported as "not this session's turn":
        // deliberately undifferentiated, following the `UnknownDeliveryId`
        // precedent, so probing turn identities reveals nothing.
        Conflict::ParentTurnNotOwnedByParentSession {
            session: parent_session,
            turn: parent_turn,
        }
        .into()
    };
    let owner: Option<String> = tx
        .conn()
        .query_row(
            "SELECT i.session_id
               FROM turns t
               JOIN session_incarnations i
                 ON i.session_incarnation_id = t.session_incarnation_id
              WHERE t.turn_id = ?1",
            params![id_text(parent_turn)],
            |row| row.get(0),
        )
        .optional()?;
    let Some(owner) = owner else {
        return Err(refused());
    };
    let owner: SessionId = parse_id(&owner, "session_incarnations", "session_id")?;
    if owner == parent_session {
        Ok(())
    } else {
        Err(refused())
    }
}

/// Refuses an edge that would close a lineage cycle, at any hop count.
///
/// [`SessionEdge::new`] already refuses `parent == child`. That is the one-hop
/// case only: A → B → C → A is three legal constructor calls and would
/// otherwise produce a graph with no root, which every lineage walk would
/// follow forever. So the proposed parent's whole ancestor chain is walked
/// here, inside the same transaction as the insert, under the write lock
/// `BEGIN IMMEDIATE` already took.
///
/// `PRIMARY KEY(child_session_id)` means each session has at most one parent,
/// so the walk is a chain and the recursion is bounded by its length. The
/// depth bound is not an optimisation and must not be removed: SQLite's
/// recursive CTE has no cycle detection of its own, so it is the only thing
/// standing between a cycle a corrupt row already created and an infinite loop.
fn require_no_lineage_cycle(tx: &Tx<'_>, parent: SessionId, child: SessionId) -> StoreResult<()> {
    // Seeded with the proposed *parent* at depth 0, so a chain that reaches the
    // proposed child anywhere — including at depth 0, which is the self-parent
    // case — closes a cycle.
    let (closes_cycle, depth): (bool, u32) = {
        let mut statement = tx.conn().prepare(
            "WITH RECURSIVE ancestry(session_id, depth) AS (
                 SELECT ?1, 0
                 UNION ALL
                 SELECT e.parent_session_id, a.depth + 1
                   FROM session_edges e
                   JOIN ancestry a ON e.child_session_id = a.session_id
                  WHERE a.depth < ?3
             )
             SELECT EXISTS(SELECT 1 FROM ancestry WHERE session_id = ?2),
                    COALESCE(MAX(depth), 0)
               FROM ancestry",
        )?;
        statement.query_row(
            params![
                id_text(parent),
                id_text(child),
                i64::from(MAX_LINEAGE_DEPTH)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? != 0,
                    crate::codec::parse_u32(row.get::<_, i64>(1)?, "session_edges", "depth")
                        .unwrap_or(MAX_LINEAGE_DEPTH),
                ))
            },
        )?
    };
    if closes_cycle {
        return Err(Conflict::SessionLineageCycle { parent, child }.into());
    }
    // Checked separately from the cycle answer: a walk that stopped at the
    // bound did not finish looking, so reporting "no cycle" from it would be
    // reporting a conclusion the query did not reach.
    if depth >= MAX_LINEAGE_DEPTH {
        return Err(Conflict::SessionLineageTooDeep {
            depth,
            bound: MAX_LINEAGE_DEPTH,
        }
        .into());
    }
    Ok(())
}

/// Proves the incarnation is one of the presented session's.
fn require_incarnation_ownership(
    tx: &Tx<'_>,
    session: SessionId,
    incarnation: SessionIncarnationId,
) -> StoreResult<()> {
    let owner: Option<String> = tx
        .conn()
        .query_row(
            "SELECT session_id FROM session_incarnations WHERE session_incarnation_id = ?1",
            params![id_text(incarnation)],
            |row| row.get(0),
        )
        .optional()?;
    let owner = owner.ok_or_else(|| {
        CorruptValue::new(
            "session_incarnations",
            "session_incarnation_id",
            CorruptReason::DanglingReference,
        )
    })?;
    if parse_id::<governor_core::id::kind::Session>(&owner, "session_incarnations", "session_id")?
        == session
    {
        Ok(())
    } else {
        Err(Conflict::NoSessionLoadout { incarnation }.into())
    }
}

/// Proves the exact `(identity, digest)` loadout snapshot is recorded.
fn require_loadout_snapshot(tx: &Tx<'_>, fence: WorkerLoadoutFence) -> StoreResult<()> {
    let found: Option<i64> = tx
        .conn()
        .query_row(
            "SELECT 1 FROM worker_loadouts WHERE worker_loadout_id = ?1 AND digest_hex = ?2",
            params![id_text(fence.id()), hex32(fence.digest().as_bytes())],
            |row| row.get(0),
        )
        .optional()?;
    if found.is_some() {
        Ok(())
    } else {
        Err(Conflict::LoadoutDigestMismatch {
            loadout: fence.id(),
        }
        .into())
    }
}

/// Requires two configuration references to be identical in every field.
fn require_same_config(
    loadout: WorkerLoadoutId,
    recorded: ManagedConfigRef,
    verified: ManagedConfigRef,
) -> StoreResult<()> {
    if recorded == verified {
        return Ok(());
    }
    Err(Conflict::ManagedConfigUnverifiable {
        loadout,
        expected: recorded.id(),
    }
    .into())
}

// --- Row reads ----------------------------------------------------------------

/// The loadout snapshot one incarnation is bound to, if any.
pub(crate) fn read_binding(
    tx: &Tx<'_>,
    incarnation: SessionIncarnationId,
) -> StoreResult<Option<WorkerLoadoutFence>> {
    const TABLE: &str = "session_loadouts";
    let row: Option<(String, String)> = tx
        .conn()
        .query_row(
            "SELECT worker_loadout_id, digest_hex FROM session_loadouts
              WHERE session_incarnation_id = ?1",
            params![id_text(incarnation)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(id, digest)| {
        Ok(WorkerLoadoutFence::new(
            parse_id(&id, TABLE, "worker_loadout_id")?,
            WorkerLoadoutDigest::from_persisted(crate::codec::parse_hex32(
                &digest,
                TABLE,
                "digest_hex",
            )?),
        ))
    })
    .transpose()
}

/// The configuration reference one loadout snapshot embeds.
fn read_loadout_config(tx: &Tx<'_>, fence: WorkerLoadoutFence) -> StoreResult<ManagedConfigRef> {
    const TABLE: &str = "worker_loadouts";
    let row: Option<(String, String, i64)> = tx
        .conn()
        .query_row(
            "SELECT managed_config_artifact_id, managed_config_digest_hex,
                    managed_config_byte_len
               FROM worker_loadouts WHERE worker_loadout_id = ?1 AND digest_hex = ?2",
            params![id_text(fence.id()), hex32(fence.digest().as_bytes())],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (id, digest, byte_len) = row.ok_or_else(|| {
        CorruptValue::new(TABLE, "worker_loadout_id", CorruptReason::DanglingReference)
    })?;
    Ok(ManagedConfigRef::new(
        parse_id(&id, TABLE, "managed_config_artifact_id")?,
        ManagedConfigDigest::from_persisted(crate::codec::parse_hex32(
            &digest,
            TABLE,
            "managed_config_digest_hex",
        )?),
        crate::codec::parse_u64(byte_len, TABLE, "managed_config_byte_len")?,
    ))
}

/// The configuration reference the artifact row itself records.
fn read_managed_config(tx: &Tx<'_>, id: ManagedConfigArtifactId) -> StoreResult<ManagedConfigRef> {
    const TABLE: &str = "managed_config_artifacts";
    let row: Option<(String, i64)> = tx
        .conn()
        .query_row(
            "SELECT sha256_hex, byte_len FROM managed_config_artifacts
              WHERE managed_config_artifact_id = ?1",
            params![id_text(id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (digest, byte_len) = row.ok_or_else(|| {
        CorruptValue::new(
            TABLE,
            "managed_config_artifact_id",
            CorruptReason::DanglingReference,
        )
    })?;
    Ok(ManagedConfigRef::new(
        id,
        ManagedConfigDigest::from_persisted(crate::codec::parse_hex32(
            &digest,
            TABLE,
            "sha256_hex",
        )?),
        crate::codec::parse_u64(byte_len, TABLE, "byte_len")?,
    ))
}

// --- Shared helpers -----------------------------------------------------------

/// The source identity of one immutable `(identity, digest)` snapshot.
///
/// Deterministic in both halves, which is what makes recording the same
/// snapshot twice converge on the ledger row that is already there instead of
/// appending a second creation event for one immutable fact.
fn snapshot_source(id: &str, digest_hex: &str, label: &str) -> StoreResult<SourceRef> {
    internal_source_text(&format!("{id}:{digest_hex}"), label)
}

/// A digest rendered as a bounded token, for allowlisted event metadata.
fn digest_token(bytes: &[u8; 32]) -> StoreResult<SafeToken> {
    SafeToken::new(&hex32(bytes)).map_err(|_| {
        CorruptValue::new("events", "safe_metadata_json", CorruptReason::UnsafeToken).into()
    })
}

/// Narrows a set size for an `INTEGER` column.
fn count_of(len: usize, column: &'static str) -> StoreResult<i64> {
    i64::try_from(len).map_err(|_| {
        CorruptValue::new(
            "capability_profiles",
            column,
            CorruptReason::IntegerOutOfRange,
        )
        .into()
    })
}
