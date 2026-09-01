//! Durable logical session lineage and immutable worker launch loadouts.
//!
//! Runtime panes, PIDs, provider session strings, and role definition files are
//! observations/configuration inputs. None of them is a sufficient identity for
//! a logical delegated session. This module captures the provider-independent
//! contract Command Governor needs before a worker process may be spawned or
//! resumed:
//!
//! - a child has durable parent/turn lineage independent of the runtime tree;
//! - capabilities are whitelist-only: an empty profile grants nothing;
//! - recursive delegation is whitelist-only: an empty policy permits no child
//!   roles;
//! - the fully resolved launch loadout has a deterministic integrity digest;
//! - resume requires the exact loadout identity **and** digest that were bound at
//!   spawn time, so editing today's role/config cannot silently broaden an old
//!   session.
//!
//! The durable/store half lives in `governor-store-sqlite`: the store must commit
//! the logical session, lineage, loadout and external-spawn intent before an
//! adapter receives permission to create the process. This pure module only
//! defines the values and admission proofs.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::fence::SafeToken;
use crate::id::{
    CapabilityProfileId, DelegationPolicyId, ManagedConfigArtifactId, ModelPolicyId, SessionId,
    TurnId, WorkerLoadoutId,
};

/// Domain separator for capability-profile digests.
pub const CAPABILITY_PROFILE_DOMAIN: &str = "command-governor/capability-profile/v1";
/// Domain separator for delegation-policy digests.
pub const DELEGATION_POLICY_DOMAIN: &str = "command-governor/delegation-policy/v1";
/// Domain separator for resolved worker-loadout digests.
pub const WORKER_LOADOUT_DOMAIN: &str = "command-governor/worker-loadout/v1";

/// A named capability granted to a managed worker.
///
/// The vocabulary remains adapter/product-defined, so this is a [`SafeToken`]
/// rather than a closed enum. The security property is supplied by
/// [`CapabilityProfile`]: only names explicitly present in its set are granted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityName(SafeToken);

impl CapabilityName {
    /// Wraps a validated capability name.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the bounded token for persistence/diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// Integrity digest of one resolved capability-profile contents snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityProfileDigest([u8; 32]);

impl CapabilityProfileDigest {
    /// Rehydrates a persisted digest after the store has read its 32 bytes.
    #[must_use]
    pub const fn from_persisted(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes for persistence and exact comparison.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// An immutable, whitelist-only capability profile.
///
/// There is deliberately no implicit/default capability set. `new(id, [])`
/// grants nothing, which prevents an omitted profile from becoming "full tools."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityProfile {
    id: CapabilityProfileId,
    capabilities: BTreeSet<CapabilityName>,
    digest: CapabilityProfileDigest,
}

impl CapabilityProfile {
    /// Builds a resolved profile and computes its canonical contents digest.
    #[must_use]
    pub fn new(
        id: CapabilityProfileId,
        capabilities: impl IntoIterator<Item = CapabilityName>,
    ) -> Self {
        let capabilities = capabilities.into_iter().collect::<BTreeSet<_>>();
        let digest = CapabilityProfileDigest(hash_tokens(
            CAPABILITY_PROFILE_DOMAIN,
            capabilities.iter().map(CapabilityName::as_token),
        ));
        Self {
            id,
            capabilities,
            digest,
        }
    }

    /// Stable identity of this profile snapshot.
    #[must_use]
    pub const fn id(&self) -> CapabilityProfileId {
        self.id
    }

    /// Canonical integrity digest of the explicitly granted capabilities.
    #[must_use]
    pub const fn digest(&self) -> CapabilityProfileDigest {
        self.digest
    }

    /// Whether the profile explicitly grants `capability`.
    #[must_use]
    pub fn allows(&self, capability: &CapabilityName) -> bool {
        self.capabilities.contains(capability)
    }

    /// Number of explicitly granted capabilities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    /// Whether no capability is granted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// Immutable reference embedded in a worker loadout.
    #[must_use]
    pub const fn reference(&self) -> CapabilityProfileRef {
        CapabilityProfileRef::new(self.id, self.digest)
    }
}

/// Immutable capability-profile identity plus the exact resolved contents digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityProfileRef {
    id: CapabilityProfileId,
    digest: CapabilityProfileDigest,
}

impl CapabilityProfileRef {
    /// Builds a reference to an already-resolved capability profile.
    #[must_use]
    pub const fn new(id: CapabilityProfileId, digest: CapabilityProfileDigest) -> Self {
        Self { id, digest }
    }

    /// Profile identity.
    #[must_use]
    pub const fn id(self) -> CapabilityProfileId {
        self.id
    }

    /// Exact contents digest.
    #[must_use]
    pub const fn digest(self) -> CapabilityProfileDigest {
        self.digest
    }
}

/// Semantic role of a worker/subagent.
///
/// Roles are product-extensible (`scout`, `researcher`, `reviewer`, custom
/// project roles), so the role name is an opaque safe token. Authorization comes
/// from explicit capability/delegation profiles, never from role-name parsing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerRole(SafeToken);

impl WorkerRole {
    /// Wraps a validated role label.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the role token for persistence/diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// Integrity digest of one recursive-delegation policy snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DelegationPolicyDigest([u8; 32]);

impl DelegationPolicyDigest {
    /// Rehydrates a persisted digest.
    #[must_use]
    pub const fn from_persisted(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes for persistence/comparison.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Explicit whitelist of child roles one worker may delegate to.
///
/// An empty policy permits no child role. There is intentionally no fallback to
/// a default/full role when a requested child is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationPolicy {
    id: DelegationPolicyId,
    allowed_roles: BTreeSet<WorkerRole>,
    digest: DelegationPolicyDigest,
}

impl DelegationPolicy {
    /// Builds an immutable delegation whitelist.
    #[must_use]
    pub fn new(id: DelegationPolicyId, roles: impl IntoIterator<Item = WorkerRole>) -> Self {
        let allowed_roles = roles.into_iter().collect::<BTreeSet<_>>();
        let digest = DelegationPolicyDigest(hash_tokens(
            DELEGATION_POLICY_DOMAIN,
            allowed_roles.iter().map(WorkerRole::as_token),
        ));
        Self {
            id,
            allowed_roles,
            digest,
        }
    }

    /// Policy identity.
    #[must_use]
    pub const fn id(&self) -> DelegationPolicyId {
        self.id
    }

    /// Exact whitelist digest.
    #[must_use]
    pub const fn digest(&self) -> DelegationPolicyDigest {
        self.digest
    }

    /// Whether this policy explicitly allows spawning `role`.
    #[must_use]
    pub fn allows(&self, role: &WorkerRole) -> bool {
        self.allowed_roles.contains(role)
    }

    /// Number of explicitly spawnable roles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.allowed_roles.len()
    }

    /// Whether recursive delegation is completely disabled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allowed_roles.is_empty()
    }

    /// Immutable reference embedded in a worker loadout.
    #[must_use]
    pub const fn reference(&self) -> DelegationPolicyRef {
        DelegationPolicyRef::new(self.id, self.digest)
    }
}

/// Immutable delegation-policy identity plus exact whitelist digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DelegationPolicyRef {
    id: DelegationPolicyId,
    digest: DelegationPolicyDigest,
}

impl DelegationPolicyRef {
    /// Builds a reference to an already-resolved delegation policy.
    #[must_use]
    pub const fn new(id: DelegationPolicyId, digest: DelegationPolicyDigest) -> Self {
        Self { id, digest }
    }

    /// Policy identity.
    #[must_use]
    pub const fn id(self) -> DelegationPolicyId {
        self.id
    }

    /// Exact whitelist digest.
    #[must_use]
    pub const fn digest(self) -> DelegationPolicyDigest {
        self.digest
    }
}

/// Integrity digest of a separately managed model-policy snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelPolicyDigest([u8; 32]);

impl ModelPolicyDigest {
    /// Rehydrates a persisted/model-resolver-supplied digest.
    #[must_use]
    pub const fn from_persisted(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Immutable model-policy identity and contents digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelPolicyRef {
    id: ModelPolicyId,
    digest: ModelPolicyDigest,
}

impl ModelPolicyRef {
    /// Builds a reference to a resolved model policy.
    #[must_use]
    pub const fn new(id: ModelPolicyId, digest: ModelPolicyDigest) -> Self {
        Self { id, digest }
    }

    /// Model-policy identity.
    #[must_use]
    pub const fn id(self) -> ModelPolicyId {
        self.id
    }

    /// Exact model-policy digest.
    #[must_use]
    pub const fn digest(self) -> ModelPolicyDigest {
        self.digest
    }
}

/// Integrity digest of the private immutable managed configuration artifact used
/// to launch/resume a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagedConfigDigest([u8; 32]);

impl ManagedConfigDigest {
    /// Rehydrates a digest after the artifact layer validated its bytes.
    #[must_use]
    pub const fn from_persisted(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Identity and digest of the private immutable managed configuration artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagedConfigRef {
    id: ManagedConfigArtifactId,
    digest: ManagedConfigDigest,
}

impl ManagedConfigRef {
    /// Builds a reference after the artifact boundary has validated the config.
    #[must_use]
    pub const fn new(id: ManagedConfigArtifactId, digest: ManagedConfigDigest) -> Self {
        Self { id, digest }
    }

    /// Configuration-artifact identity.
    #[must_use]
    pub const fn id(self) -> ManagedConfigArtifactId {
        self.id
    }

    /// Exact configuration digest.
    #[must_use]
    pub const fn digest(self) -> ManagedConfigDigest {
        self.digest
    }
}

/// Version of the managed hook/configuration contract expected by a loadout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookContractEpoch(u64);

impl HookContractEpoch {
    /// Wraps a persisted contract epoch.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the epoch value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Resume policy for one resolved loadout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResumePolicy {
    /// Resume only when the exact loadout identity and digest match the launch
    /// snapshot. This is the only V1-safe policy.
    ExactLoadout,
}

impl ResumePolicy {
    /// Stable storage/digest code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ExactLoadout => "exact_loadout",
        }
    }
}

/// Fully resolved safe metadata needed to reconstruct one worker sandbox.
///
/// Raw prompts, cwd, tool arguments/results, credentials, and arbitrary config
/// text do not belong here. Exact launch configuration is represented by an
/// immutable private artifact reference + digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLoadoutSpec {
    /// Stable identity of this resolved loadout snapshot.
    pub id: WorkerLoadoutId,
    /// Worker adapter class, e.g. `claude`.
    pub worker_kind: SafeToken,
    /// Runtime adapter class, e.g. `herdr`.
    pub runtime_kind: SafeToken,
    /// Semantic role used for policy/analytics.
    pub role: WorkerRole,
    /// Immutable model-policy snapshot.
    pub model_policy: ModelPolicyRef,
    /// Exact capability whitelist snapshot.
    pub capability_profile: CapabilityProfileRef,
    /// Exact recursive delegation whitelist snapshot.
    pub delegation_policy: DelegationPolicyRef,
    /// Private immutable managed launch configuration.
    pub managed_config: ManagedConfigRef,
    /// Hook/configuration contract expected by the adapter.
    pub hook_contract_epoch: HookContractEpoch,
    /// Resume policy applied to this logical session.
    pub resume_policy: ResumePolicy,
}

/// Integrity digest binding every safe component of a resolved loadout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerLoadoutDigest([u8; 32]);

impl WorkerLoadoutDigest {
    /// Rehydrates a persisted digest for integrity verification.
    #[must_use]
    pub const fn from_persisted(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes for persistence/comparison.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Immutable resolved worker loadout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLoadout {
    spec: WorkerLoadoutSpec,
    digest: WorkerLoadoutDigest,
}

impl WorkerLoadout {
    /// Resolves a loadout and computes the canonical digest that will fence
    /// future resume attempts.
    #[must_use]
    pub fn new(spec: WorkerLoadoutSpec) -> Self {
        let digest = WorkerLoadoutDigest(hash_loadout(&spec));
        Self { spec, digest }
    }

    /// Rehydrates a persisted loadout and fails closed if its stored digest does
    /// not match the canonical contents.
    ///
    /// # Errors
    ///
    /// Returns [`LoadoutIntegrityError`] when durable fields were corrupted or
    /// changed without creating a new loadout revision.
    pub fn from_persisted(
        spec: WorkerLoadoutSpec,
        persisted_digest: WorkerLoadoutDigest,
    ) -> Result<Self, LoadoutIntegrityError> {
        let loadout = Self::new(spec);
        if loadout.digest == persisted_digest {
            Ok(loadout)
        } else {
            Err(LoadoutIntegrityError::DigestMismatch)
        }
    }

    /// Safe resolved fields of this immutable snapshot.
    #[must_use]
    pub const fn spec(&self) -> &WorkerLoadoutSpec {
        &self.spec
    }

    /// Canonical integrity digest.
    #[must_use]
    pub const fn digest(&self) -> WorkerLoadoutDigest {
        self.digest
    }

    /// Exact identity+digest a resume caller must present.
    #[must_use]
    pub const fn fence(&self) -> WorkerLoadoutFence {
        WorkerLoadoutFence::new(self.spec.id, self.digest)
    }

    /// Checks that a continuation targets this exact resolved loadout and returns
    /// a proof value only on an exact match.
    ///
    /// # Errors
    ///
    /// A different loadout identity or digest is a typed refusal; the caller
    /// must create reconciliation/new-session work rather than launching with a
    /// broader current configuration.
    pub fn admit_resume(
        &self,
        presented: WorkerLoadoutFence,
    ) -> Result<ResumePermit, LoadoutMismatch> {
        if presented.id != self.spec.id {
            return Err(LoadoutMismatch::DifferentIdentity);
        }
        if presented.digest != self.digest {
            return Err(LoadoutMismatch::DigestMismatch);
        }
        Ok(ResumePermit { fence: presented })
    }
}

/// Exact loadout identity plus digest used as a resume fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerLoadoutFence {
    id: WorkerLoadoutId,
    digest: WorkerLoadoutDigest,
}

impl WorkerLoadoutFence {
    /// Rehydrates/builds a fence from already validated persisted fields.
    #[must_use]
    pub const fn new(id: WorkerLoadoutId, digest: WorkerLoadoutDigest) -> Self {
        Self { id, digest }
    }

    /// Loadout identity.
    #[must_use]
    pub const fn id(self) -> WorkerLoadoutId {
        self.id
    }

    /// Exact resolved-loadout digest.
    #[must_use]
    pub const fn digest(self) -> WorkerLoadoutDigest {
        self.digest
    }
}

/// Proof that an exact immutable loadout fence admitted a resume.
///
/// Fields are private so adapters cannot construct this proof without passing
/// [`WorkerLoadout::admit_resume`]. The later store/adapter layer may require
/// this value before constructing an external continuation permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumePermit {
    fence: WorkerLoadoutFence,
}

impl ResumePermit {
    /// Exact loadout fence that was verified.
    #[must_use]
    pub const fn fence(self) -> WorkerLoadoutFence {
        self.fence
    }
}

/// A persisted loadout failed its own integrity check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LoadoutIntegrityError {
    /// Stored digest does not match the canonical safe fields.
    #[error("worker loadout digest mismatch")]
    DigestMismatch,
}

/// Why an existing logical session cannot be resumed under a presented loadout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LoadoutMismatch {
    /// Caller presented a different logical loadout identity.
    #[error("worker loadout identity does not match the session launch snapshot")]
    DifferentIdentity,
    /// Identity matched but resolved contents/configuration digest did not.
    #[error("worker loadout digest does not match the session launch snapshot")]
    DigestMismatch,
}

/// Semantic relationship between a parent session and a child logical session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionRelation {
    /// General delegated implementation worker.
    DelegatedWorker,
    /// Read-oriented codebase scout.
    Scout,
    /// Research specialist.
    Researcher,
    /// Independent review specialist.
    Reviewer,
    /// Background observational-memory mapper.
    Observer,
    /// Background memory consolidator.
    Consolidator,
    /// Provider-native fork/clone represented as provenance, not copied control
    /// transcript content.
    ProviderFork,
}

impl SessionRelation {
    /// Stable storage/diagnostic code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::DelegatedWorker => "delegated_worker",
            Self::Scout => "scout",
            Self::Researcher => "researcher",
            Self::Reviewer => "reviewer",
            Self::Observer => "observer",
            Self::Consolidator => "consolidator",
            Self::ProviderFork => "provider_fork",
        }
    }
}

/// Durable logical parent/child lineage edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionEdge {
    parent_session: SessionId,
    child_session: SessionId,
    parent_turn: TurnId,
    relation: SessionRelation,
}

impl SessionEdge {
    /// Constructs a child relationship.
    ///
    /// # Errors
    ///
    /// A session cannot be its own parent. Store-level foreign-key/projection
    /// checks additionally prove that `parent_turn` belongs to the parent.
    pub fn new(
        parent_session: SessionId,
        child_session: SessionId,
        parent_turn: TurnId,
        relation: SessionRelation,
    ) -> Result<Self, SessionLineageError> {
        if parent_session == child_session {
            return Err(SessionLineageError::SelfParent);
        }
        Ok(Self {
            parent_session,
            child_session,
            parent_turn,
            relation,
        })
    }

    /// Logical parent session.
    #[must_use]
    pub const fn parent_session(self) -> SessionId {
        self.parent_session
    }

    /// Logical child session.
    #[must_use]
    pub const fn child_session(self) -> SessionId {
        self.child_session
    }

    /// Parent turn that created the delegation/fork.
    #[must_use]
    pub const fn parent_turn(self) -> TurnId {
        self.parent_turn
    }

    /// Semantic child relationship.
    #[must_use]
    pub const fn relation(self) -> SessionRelation {
        self.relation
    }
}

/// Invalid logical session-lineage relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionLineageError {
    /// A logical session cannot delegate/fork to itself.
    #[error("a session cannot be its own parent")]
    SelfParent,
}

fn hash_tokens<'a>(domain: &str, tokens: impl IntoIterator<Item = &'a SafeToken>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    absorb(&mut hasher, domain.as_bytes());
    for token in tokens {
        absorb(&mut hasher, token.as_str().as_bytes());
    }
    hasher.finalize().into()
}

fn hash_loadout(spec: &WorkerLoadoutSpec) -> [u8; 32] {
    let mut hasher = Sha256::new();
    absorb(&mut hasher, WORKER_LOADOUT_DOMAIN.as_bytes());
    absorb_uuid(&mut hasher, spec.id.as_uuid());
    absorb(&mut hasher, spec.worker_kind.as_str().as_bytes());
    absorb(&mut hasher, spec.runtime_kind.as_str().as_bytes());
    absorb(&mut hasher, spec.role.as_token().as_str().as_bytes());

    absorb_uuid(&mut hasher, spec.model_policy.id().as_uuid());
    absorb(&mut hasher, spec.model_policy.digest().as_bytes());

    absorb_uuid(&mut hasher, spec.capability_profile.id().as_uuid());
    absorb(&mut hasher, spec.capability_profile.digest().as_bytes());

    absorb_uuid(&mut hasher, spec.delegation_policy.id().as_uuid());
    absorb(&mut hasher, spec.delegation_policy.digest().as_bytes());

    absorb_uuid(&mut hasher, spec.managed_config.id().as_uuid());
    absorb(&mut hasher, spec.managed_config.digest().as_bytes());

    absorb(&mut hasher, &spec.hook_contract_epoch.get().to_be_bytes());
    absorb(&mut hasher, spec.resume_policy.code().as_bytes());
    hasher.finalize().into()
}

fn absorb_uuid(hasher: &mut Sha256, value: uuid::Uuid) {
    absorb(hasher, value.as_bytes());
}

fn absorb(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).expect("bounded loadout field length fits in u64");
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn token(value: &str) -> SafeToken {
        SafeToken::new(value).expect("test token is safe")
    }

    fn id<T>(value: u128) -> crate::id::Id<T>
    where
        T: crate::id::IdKind,
    {
        crate::id::Id::from_uuid(Uuid::from_u128(value))
    }

    fn loadout_with(
        capability: CapabilityProfileRef,
        delegation: DelegationPolicyRef,
    ) -> WorkerLoadout {
        WorkerLoadout::new(WorkerLoadoutSpec {
            id: id(1),
            worker_kind: token("claude"),
            runtime_kind: token("herdr"),
            role: WorkerRole::new(token("worker")),
            model_policy: ModelPolicyRef::new(id(2), ModelPolicyDigest::from_persisted([2; 32])),
            capability_profile: capability,
            delegation_policy: delegation,
            managed_config: ManagedConfigRef::new(
                id(5),
                ManagedConfigDigest::from_persisted([5; 32]),
            ),
            hook_contract_epoch: HookContractEpoch::new(7),
            resume_policy: ResumePolicy::ExactLoadout,
        })
    }

    #[test]
    fn empty_capability_profile_grants_nothing() {
        let profile = CapabilityProfile::new(id(10), []);
        assert!(profile.is_empty());
        assert!(!profile.allows(&CapabilityName::new(token("write"))));
    }

    #[test]
    fn capability_profile_is_explicit_whitelist() {
        let read = CapabilityName::new(token("read"));
        let write = CapabilityName::new(token("write"));
        let profile = CapabilityProfile::new(id(10), [read.clone()]);
        assert!(profile.allows(&read));
        assert!(!profile.allows(&write));
    }

    #[test]
    fn empty_delegation_policy_cannot_spawn_a_default_role() {
        let policy = DelegationPolicy::new(id(20), []);
        assert!(policy.is_empty());
        assert!(!policy.allows(&WorkerRole::new(token("worker"))));
    }

    #[test]
    fn delegation_policy_allows_only_named_roles() {
        let scout = WorkerRole::new(token("scout"));
        let worker = WorkerRole::new(token("worker"));
        let policy = DelegationPolicy::new(id(20), [scout.clone()]);
        assert!(policy.allows(&scout));
        assert!(!policy.allows(&worker));
    }

    #[test]
    fn exact_loadout_fence_is_required_for_resume() {
        let capabilities = CapabilityProfile::new(id(30), [CapabilityName::new(token("read"))]);
        let delegation = DelegationPolicy::new(id(40), [WorkerRole::new(token("scout"))]);
        let loadout = loadout_with(capabilities.reference(), delegation.reference());

        let permit = loadout
            .admit_resume(loadout.fence())
            .expect("exact launch snapshot resumes");
        assert_eq!(permit.fence(), loadout.fence());
    }

    #[test]
    fn same_loadout_id_with_changed_capabilities_is_refused() {
        let capability_id: CapabilityProfileId = id(30);
        let original = CapabilityProfile::new(capability_id, [CapabilityName::new(token("read"))]);
        let widened = CapabilityProfile::new(
            capability_id,
            [
                CapabilityName::new(token("read")),
                CapabilityName::new(token("write")),
            ],
        );
        assert_ne!(original.digest(), widened.digest());

        let delegation = DelegationPolicy::new(id(40), [WorkerRole::new(token("scout"))]);
        let original_loadout = loadout_with(original.reference(), delegation.reference());
        let widened_loadout = loadout_with(widened.reference(), delegation.reference());

        assert_eq!(original_loadout.spec().id, widened_loadout.spec().id);
        assert_ne!(original_loadout.digest(), widened_loadout.digest());
        assert_eq!(
            original_loadout.admit_resume(widened_loadout.fence()),
            Err(LoadoutMismatch::DigestMismatch)
        );
    }

    #[test]
    fn different_loadout_identity_is_refused_even_with_same_digest_shape() {
        let capabilities = CapabilityProfile::new(id(30), []);
        let delegation = DelegationPolicy::new(id(40), []);
        let loadout = loadout_with(capabilities.reference(), delegation.reference());
        let presented = WorkerLoadoutFence::new(id(999), loadout.digest());
        assert_eq!(
            loadout.admit_resume(presented),
            Err(LoadoutMismatch::DifferentIdentity)
        );
    }

    #[test]
    fn persisted_loadout_digest_is_integrity_checked() {
        let capabilities = CapabilityProfile::new(id(30), []);
        let delegation = DelegationPolicy::new(id(40), []);
        let loadout = loadout_with(capabilities.reference(), delegation.reference());
        let spec = loadout.spec().clone();

        assert_eq!(
            WorkerLoadout::from_persisted(spec, WorkerLoadoutDigest::from_persisted([0xA5; 32])),
            Err(LoadoutIntegrityError::DigestMismatch)
        );
    }

    #[test]
    fn session_lineage_rejects_self_parent() {
        let session: SessionId = id(50);
        let turn: TurnId = id(51);
        assert_eq!(
            SessionEdge::new(session, session, turn, SessionRelation::Scout),
            Err(SessionLineageError::SelfParent)
        );
    }

    #[test]
    fn session_lineage_preserves_parent_turn_and_semantic_relation() {
        let edge = SessionEdge::new(id(50), id(51), id(52), SessionRelation::Researcher)
            .expect("distinct parent and child are valid");
        assert_eq!(edge.relation().code(), "researcher");
        assert_ne!(edge.parent_session(), edge.child_session());
    }
}
