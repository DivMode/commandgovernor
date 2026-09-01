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
//!   session;
//! - resume additionally requires proof that the managed configuration artifact
//!   was re-read and still hashes to what the launch snapshot recorded, so a
//!   deleted or rewritten config fails closed instead of resuming under whatever
//!   is on disk now.
//!
//! # Two constructors, deliberately not interchangeable
//!
//! Resolving a loadout and loading one back are different operations and have
//! different types. [`WorkerLoadout::resolve`] is a *resolve-time* computation:
//! it takes freshly resolved parts and derives the digest that will be written
//! down. [`CommittedLoadout::rehydrate`] is the only path from a persisted row,
//! and it re-derives the digest and refuses a row whose safe fields no longer
//! agree with it. Only a [`CommittedLoadout`] can admit a resume, so a loadout
//! assembled at run time cannot stand in for the one a session was launched
//! under.
//!
//! The durable/store half lives in `governor-store-sqlite`: the store must commit
//! the logical session, lineage, loadout and external-spawn intent before an
//! adapter receives permission to create the process. This pure module only
//! defines the values and admission proofs.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::artifact::{ArtifactDigest, ArtifactIntegrityError};
use crate::digest::{absorb, absorb_u64, absorb_uuid};
use crate::error::Conflict;
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

/// Adapter class of the worker itself, e.g. `claude`.
///
/// A distinct type from [`RuntimeKind`] on purpose: the two are adjacent fields
/// of the same shape in a loadout, and transposing them would resolve a real
/// worker under the wrong adapter without any value looking wrong.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerKind(SafeToken);

impl WorkerKind {
    /// Wraps a validated worker-adapter label.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the label for persistence/diagnostics.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }
}

/// Adapter class of the runtime that hosts the worker, e.g. `herdr`.
///
/// See [`WorkerKind`] for why this is not a bare [`SafeToken`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeKind(SafeToken);

impl RuntimeKind {
    /// Wraps a validated runtime-adapter label.
    #[must_use]
    pub const fn new(token: SafeToken) -> Self {
        Self(token)
    }

    /// Returns the label for persistence/diagnostics.
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

/// Identity, digest and exact length of the private immutable managed
/// configuration artifact.
///
/// The length is carried alongside the digest because a truncated read has a
/// perfectly valid digest *of the truncation*; checking length first is what
/// keeps that failure distinguishable. The rule itself is
/// [`ArtifactIntegrityError::check`] and is not restated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ManagedConfigRef {
    id: ManagedConfigArtifactId,
    digest: ManagedConfigDigest,
    byte_len: u64,
}

impl ManagedConfigRef {
    /// Builds a reference after the artifact boundary has validated the config.
    #[must_use]
    pub const fn new(
        id: ManagedConfigArtifactId,
        digest: ManagedConfigDigest,
        byte_len: u64,
    ) -> Self {
        Self {
            id,
            digest,
            byte_len,
        }
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

    /// Exact configuration length in bytes.
    #[must_use]
    pub const fn byte_len(self) -> u64 {
        self.byte_len
    }
}

/// Proof that a managed configuration artifact was re-read and still matches the
/// reference a loadout was launched under.
///
/// # There is no public constructor
///
/// The only source is [`Self::verify`], which takes a *freshly computed* digest
/// and length and compares them against the recorded reference through
/// [`ArtifactIntegrityError::check`]. There is no way to mint this value from a
/// reference alone, so "the row still says the config is fine" cannot be
/// mistaken for "the bytes are still there and still hash to this."
///
/// The value is neither `Clone` nor `Copy`: one re-verification authorises at
/// most one [`ResumePermit`].
///
/// # What the artifacts layer must uphold
///
/// `governor-core` performs no I/O and cannot read the artifact itself. This
/// type is the *place where the artifacts/daemon layer asserts it*, and the
/// assertion has two parts:
///
/// 1. `observed_digest` and `observed_len` come from bytes read *now*, in full,
///    from the private artifact store — not from a cached row, a manifest, or a
///    previous verification.
/// 2. Nothing between that read and [`CommittedLoadout::admit_resume`] may
///    substitute different bytes for the ones that were hashed.
///
/// A caller that passes the reference's own digest straight back in has proved
/// nothing; every guarantee below rests on those two lines.
#[derive(Debug)]
pub struct ManagedConfigVerified {
    reference: ManagedConfigRef,
}

impl ManagedConfigVerified {
    /// Re-proves a managed configuration artifact against freshly read bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactIntegrityError`] when the re-read bytes are a different
    /// length or hash to something else. Callers must fail closed: an
    /// unverifiable launch configuration is reconciliation work, never a resume
    /// under whatever configuration exists now.
    pub fn verify(
        reference: ManagedConfigRef,
        observed_digest: ManagedConfigDigest,
        observed_len: u64,
    ) -> Result<Self, ArtifactIntegrityError> {
        ArtifactIntegrityError::check(
            ArtifactDigest::from_bytes(*reference.digest().as_bytes()),
            reference.byte_len(),
            ArtifactDigest::from_bytes(*observed_digest.as_bytes()),
            observed_len,
        )?;
        Ok(Self { reference })
    }

    /// The configuration reference these re-read bytes vouch for.
    #[must_use]
    pub const fn reference(&self) -> ManagedConfigRef {
        self.reference
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
///
/// # Not in the digest, on purpose
///
/// ADR 0007 lists `created_event_seq` as part of the durable loadout record, and
/// it is deliberately absent here. The sequence number is assigned by the store
/// when the row commits — after the digest exists and is itself part of what is
/// written — so a digest over it could never be computed by the resolver. Slice
/// 2 carries it as a schema column on the loadout row, next to the digest rather
/// than inside it.
///
/// # Field-wise, and what that costs
///
/// This is a parts record, not a capability: anyone can assemble one, exactly as
/// anyone can assemble a [`crate::claim::PersistedClaim`]. Assembling a spec
/// proves nothing on its own. Safety comes from what a spec can be turned *into*:
/// [`WorkerLoadout::resolve`] only ever produces a loadout consistent with the
/// parts it was handed, and [`CommittedLoadout::rehydrate`] — the sole path from
/// a persisted row — refuses parts that disagree with the digest recorded beside
/// them. A tampered spec therefore yields a different loadout identity/digest
/// pair, which is precisely what [`CommittedLoadout::admit_resume`] refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLoadoutSpec {
    /// Stable identity of this resolved loadout snapshot.
    pub id: WorkerLoadoutId,
    /// Worker adapter class, e.g. `claude`.
    pub worker_kind: WorkerKind,
    /// Runtime adapter class, e.g. `herdr`.
    pub runtime_kind: RuntimeKind,
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

/// A resolved worker loadout as it exists *before* it is written down.
///
/// This is the spawn-time value: the store persists its spec and digest, and the
/// pair becomes the launch snapshot every later resume is fenced against. It
/// cannot admit a resume — that authority belongs to [`CommittedLoadout`], which
/// is reachable only by loading a row back and re-proving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLoadout {
    spec: WorkerLoadoutSpec,
    digest: WorkerLoadoutDigest,
}

impl WorkerLoadout {
    /// Resolves a loadout and computes the canonical digest that will fence
    /// future resume attempts.
    ///
    /// A resolve-time operation, not a loader: it derives the digest rather than
    /// checking one, so it can never *disagree* with the parts it was given.
    /// Reaching it with parts read from durable storage would therefore launder
    /// a tampered row into a self-consistent loadout —
    /// [`CommittedLoadout::rehydrate`] exists so that path does not have to be
    /// taken.
    #[must_use]
    pub fn resolve(spec: WorkerLoadoutSpec) -> Self {
        let digest = WorkerLoadoutDigest(hash_loadout(&spec));
        Self { spec, digest }
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
}

/// One persisted worker-loadout row, as the store read it back.
///
/// The digest travels with the safe fields precisely so the two can be checked
/// against each other; nothing here is trusted until [`CommittedLoadout::rehydrate`]
/// has re-derived it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedWorkerLoadout {
    /// Safe resolved fields recorded at launch.
    pub spec: WorkerLoadoutSpec,
    /// Integrity digest recorded beside them.
    pub digest: WorkerLoadoutDigest,
}

/// The launch snapshot of a logical session, loaded back from durable state.
///
/// # Why this is a separate type
///
/// Resume is only meaningful against the loadout a session was *launched* under,
/// and that value lives in the store. Making [`Self::admit_resume`] reachable
/// only from [`Self::rehydrate`] means a loadout resolved from current
/// configuration — today's role file, today's capability profile — has no method
/// that can admit anything, so the broadening substitution that fence exists to
/// prevent is not expressible rather than merely discouraged.
///
/// # Residual surface
///
/// `governor-core` performs no I/O and so cannot distinguish a genuine row from
/// a fabricated one: a caller that invents both a spec and a matching digest gets
/// a `CommittedLoadout`, because the two agree. That is the same boundary
/// [`crate::claim::ForemanClaim::rehydrate`] sits on — the loader re-proves a row
/// against itself, and authenticity of the row is the store's problem, enforced
/// there by the durable schema and by nothing in this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedLoadout {
    loadout: WorkerLoadout,
}

impl CommittedLoadout {
    /// Rebuilds the launch snapshot the store previously persisted.
    ///
    /// A *validating* loader, not a field-wise constructor: it re-derives the
    /// canonical digest from `parts.spec` and requires it to equal
    /// `parts.digest`. This is the only path from a persisted row to a value that
    /// can admit a resume.
    ///
    /// # Errors
    ///
    /// Returns [`LoadoutIntegrityError`] when durable fields were corrupted or
    /// changed without creating a new loadout revision.
    pub fn rehydrate(parts: PersistedWorkerLoadout) -> Result<Self, LoadoutIntegrityError> {
        let PersistedWorkerLoadout { spec, digest } = parts;
        let loadout = WorkerLoadout::resolve(spec);
        if loadout.digest == digest {
            Ok(Self { loadout })
        } else {
            Err(LoadoutIntegrityError::DigestMismatch)
        }
    }

    /// The re-proved launch snapshot.
    #[must_use]
    pub const fn loadout(&self) -> &WorkerLoadout {
        &self.loadout
    }

    /// Safe resolved fields recorded at launch.
    #[must_use]
    pub const fn spec(&self) -> &WorkerLoadoutSpec {
        self.loadout.spec()
    }

    /// Canonical integrity digest recorded at launch.
    #[must_use]
    pub const fn digest(&self) -> WorkerLoadoutDigest {
        self.loadout.digest()
    }

    /// Exact identity+digest a resume caller must present.
    #[must_use]
    pub const fn fence(&self) -> WorkerLoadoutFence {
        self.loadout.fence()
    }

    /// Checks that a continuation targets this exact launch snapshot *and* that
    /// its managed configuration artifact still exists and still hashes to what
    /// was recorded, returning a proof value only when all three hold.
    ///
    /// Identity and digest equality alone would admit a session whose private
    /// launch configuration had since been deleted or rewritten, because the
    /// durable row would still agree with itself. `config` is what closes that:
    /// it can only have come from [`ManagedConfigVerified::verify`], which
    /// requires a freshly computed digest and length.
    ///
    /// # Errors
    ///
    /// - [`Conflict::LoadoutIdentityMismatch`] — a different logical loadout.
    /// - [`Conflict::LoadoutDigestMismatch`] — same identity, different resolved
    ///   contents; the caller must create a new loadout revision rather than
    ///   launch under a broader current configuration.
    /// - [`Conflict::ManagedConfigUnverifiable`] — the presented proof does not
    ///   vouch for this loadout's configuration artifact; the caller must raise
    ///   reconciliation/input attention, never resume.
    pub fn admit_resume(
        &self,
        presented: WorkerLoadoutFence,
        config: ManagedConfigVerified,
    ) -> Result<ResumePermit, Conflict> {
        let expected = self.spec();
        if presented.id != expected.id {
            return Err(Conflict::LoadoutIdentityMismatch {
                presented: presented.id,
                expected: expected.id,
            });
        }
        if presented.digest != self.digest() {
            return Err(Conflict::LoadoutDigestMismatch {
                loadout: expected.id,
            });
        }
        if config.reference() != expected.managed_config {
            return Err(Conflict::ManagedConfigUnverifiable {
                loadout: expected.id,
                expected: expected.managed_config.id(),
            });
        }
        Ok(ResumePermit {
            fence: presented,
            managed_config: config.reference(),
        })
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

/// Proof that an exact immutable loadout fence and a re-verified managed
/// configuration together admitted a resume.
///
/// # There is no public constructor
///
/// The only source is [`CommittedLoadout::admit_resume`], and the value is
/// neither `Clone` nor `Copy`, so one admission yields at most one permit. The
/// later store/adapter layer may require this value before constructing an
/// external continuation permit.
#[derive(Debug, PartialEq, Eq)]
pub struct ResumePermit {
    fence: WorkerLoadoutFence,
    managed_config: ManagedConfigRef,
}

impl ResumePermit {
    /// Exact loadout fence that was verified.
    #[must_use]
    pub const fn fence(&self) -> WorkerLoadoutFence {
        self.fence
    }

    /// Managed configuration artifact whose bytes were re-proved.
    #[must_use]
    pub const fn managed_config(&self) -> ManagedConfigRef {
        self.managed_config
    }
}

/// A persisted loadout failed its own integrity check.
///
/// Module-local rather than a [`Conflict`]: like
/// [`crate::claim::ClaimProvenanceMismatch`], this is a corrupt durable row, not
/// a caller presenting a stale fence, and the two must stay distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoadoutIntegrityError {
    /// Stored digest does not match the canonical safe fields.
    #[error("worker loadout digest mismatch")]
    DigestMismatch,
}

/// Semantic relationship between a parent session and a child logical session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
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
#[non_exhaustive]
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
    // Destructured exhaustively on purpose. Reading the fields through accessors
    // would let a field added later escape the digest silently; with the pattern
    // below, adding one is a compile error at the exact place that decides
    // whether it fences a resume.
    let WorkerLoadoutSpec {
        id,
        worker_kind,
        runtime_kind,
        role,
        model_policy,
        capability_profile,
        delegation_policy,
        managed_config,
        hook_contract_epoch,
        resume_policy,
    } = spec;

    let mut hasher = Sha256::new();
    absorb(&mut hasher, WORKER_LOADOUT_DOMAIN.as_bytes());
    absorb_uuid(&mut hasher, id.as_uuid());
    absorb(&mut hasher, worker_kind.as_token().as_str().as_bytes());
    absorb(&mut hasher, runtime_kind.as_token().as_str().as_bytes());
    absorb(&mut hasher, role.as_token().as_str().as_bytes());

    absorb_uuid(&mut hasher, model_policy.id().as_uuid());
    absorb(&mut hasher, model_policy.digest().as_bytes());

    absorb_uuid(&mut hasher, capability_profile.id().as_uuid());
    absorb(&mut hasher, capability_profile.digest().as_bytes());

    absorb_uuid(&mut hasher, delegation_policy.id().as_uuid());
    absorb(&mut hasher, delegation_policy.digest().as_bytes());

    absorb_uuid(&mut hasher, managed_config.id().as_uuid());
    absorb(&mut hasher, managed_config.digest().as_bytes());
    absorb_u64(&mut hasher, managed_config.byte_len());

    absorb_u64(&mut hasher, hook_contract_epoch.get());
    absorb(&mut hasher, resume_policy.code().as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::error::ConflictKind;

    const CONFIG_DIGEST: [u8; 32] = [5; 32];
    const CONFIG_LEN: u64 = 512;

    fn token(value: &str) -> SafeToken {
        SafeToken::new(value).expect("test token is safe")
    }

    fn id<T>(value: u128) -> crate::id::Id<T>
    where
        T: crate::id::IdKind,
    {
        crate::id::Id::from_uuid(Uuid::from_u128(value))
    }

    fn spec_with(
        capability: CapabilityProfileRef,
        delegation: DelegationPolicyRef,
    ) -> WorkerLoadoutSpec {
        WorkerLoadoutSpec {
            id: id(1),
            worker_kind: WorkerKind::new(token("claude")),
            runtime_kind: RuntimeKind::new(token("herdr")),
            role: WorkerRole::new(token("worker")),
            model_policy: ModelPolicyRef::new(id(2), ModelPolicyDigest::from_persisted([2; 32])),
            capability_profile: capability,
            delegation_policy: delegation,
            managed_config: ManagedConfigRef::new(
                id(5),
                ManagedConfigDigest::from_persisted(CONFIG_DIGEST),
                CONFIG_LEN,
            ),
            hook_contract_epoch: HookContractEpoch::new(7),
            resume_policy: ResumePolicy::ExactLoadout,
        }
    }

    fn loadout_with(
        capability: CapabilityProfileRef,
        delegation: DelegationPolicyRef,
    ) -> WorkerLoadout {
        WorkerLoadout::resolve(spec_with(capability, delegation))
    }

    fn committed(loadout: &WorkerLoadout) -> CommittedLoadout {
        CommittedLoadout::rehydrate(PersistedWorkerLoadout {
            spec: loadout.spec().clone(),
            digest: loadout.digest(),
        })
        .expect("a freshly resolved loadout agrees with its own digest")
    }

    /// The re-read the artifacts layer is expected to perform, standing in for
    /// bytes that are still exactly as they were at launch.
    fn verified_config(reference: ManagedConfigRef) -> ManagedConfigVerified {
        ManagedConfigVerified::verify(
            reference,
            ManagedConfigDigest::from_persisted(CONFIG_DIGEST),
            CONFIG_LEN,
        )
        .expect("unchanged bytes re-verify")
    }

    fn base_spec() -> WorkerLoadoutSpec {
        let capabilities = CapabilityProfile::new(id(30), [CapabilityName::new(token("read"))]);
        let delegation = DelegationPolicy::new(id(40), [WorkerRole::new(token("scout"))]);
        spec_with(capabilities.reference(), delegation.reference())
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
        let committed = committed(&loadout);

        // A loadout resolved from current configuration has no `admit_resume` at
        // all; only the re-proved launch snapshot does.
        let permit = committed
            .admit_resume(
                committed.fence(),
                verified_config(committed.spec().managed_config),
            )
            .expect("exact launch snapshot with a re-verified config resumes");
        assert_eq!(permit.fence(), loadout.fence());
        assert_eq!(permit.managed_config(), loadout.spec().managed_config);
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

        let committed = committed(&original_loadout);
        let refusal = committed
            .admit_resume(
                widened_loadout.fence(),
                verified_config(committed.spec().managed_config),
            )
            .expect_err("a widened profile is not the launch snapshot");
        assert_eq!(refusal.kind(), ConflictKind::LoadoutDigestMismatch);
    }

    #[test]
    fn different_loadout_identity_is_refused_even_with_same_digest_shape() {
        let capabilities = CapabilityProfile::new(id(30), []);
        let delegation = DelegationPolicy::new(id(40), []);
        let loadout = loadout_with(capabilities.reference(), delegation.reference());
        let committed = committed(&loadout);
        let presented = WorkerLoadoutFence::new(id(999), loadout.digest());

        let refusal = committed
            .admit_resume(presented, verified_config(committed.spec().managed_config))
            .expect_err("a different loadout identity is refused");
        assert_eq!(refusal.kind(), ConflictKind::LoadoutIdentityMismatch);
    }

    #[test]
    fn persisted_loadout_digest_is_integrity_checked() {
        let capabilities = CapabilityProfile::new(id(30), []);
        let delegation = DelegationPolicy::new(id(40), []);
        let loadout = loadout_with(capabilities.reference(), delegation.reference());

        assert_eq!(
            CommittedLoadout::rehydrate(PersistedWorkerLoadout {
                spec: loadout.spec().clone(),
                digest: WorkerLoadoutDigest::from_persisted([0xA5; 32]),
            }),
            Err(LoadoutIntegrityError::DigestMismatch)
        );
    }

    #[test]
    fn a_widened_profile_cannot_be_laundered_through_a_persisted_row() {
        // The reviewer's probe: take the launch row, widen the capability
        // profile, and try to load it back. `rehydrate` is the only path from a
        // row to something that can resume, and it re-derives the digest.
        let capability_id: CapabilityProfileId = id(30);
        let delegation = DelegationPolicy::new(id(40), [WorkerRole::new(token("scout"))]);
        let original = CapabilityProfile::new(capability_id, [CapabilityName::new(token("read"))]);
        let launched = loadout_with(original.reference(), delegation.reference());

        let widened = CapabilityProfile::new(
            capability_id,
            [
                CapabilityName::new(token("read")),
                CapabilityName::new(token("write")),
            ],
        );
        let mut tampered = launched.spec().clone();
        tampered.capability_profile = widened.reference();

        assert_eq!(
            CommittedLoadout::rehydrate(PersistedWorkerLoadout {
                spec: tampered,
                digest: launched.digest(),
            }),
            Err(LoadoutIntegrityError::DigestMismatch)
        );
    }

    #[test]
    fn every_spec_field_is_bound_by_the_loadout_digest() {
        // One case per field the digest must cover. `resume_policy` has a single
        // variant today, so no second value exists to differ from it; the
        // exhaustive destructure in `hash_loadout` is what will force a future
        // variant into the pre-image.
        type Mutate = fn(&mut WorkerLoadoutSpec);
        let cases: &[(&str, Mutate)] = &[
            ("id", |spec| spec.id = id(0xBEEF)),
            ("worker_kind", |spec| {
                spec.worker_kind = WorkerKind::new(token("codex"));
            }),
            ("runtime_kind", |spec| {
                spec.runtime_kind = RuntimeKind::new(token("tmux"));
            }),
            ("role", |spec| {
                spec.role = WorkerRole::new(token("reviewer"));
            }),
            ("model_policy.id", |spec| {
                spec.model_policy = ModelPolicyRef::new(id(0xA1), spec.model_policy.digest());
            }),
            ("model_policy.digest", |spec| {
                spec.model_policy = ModelPolicyRef::new(
                    spec.model_policy.id(),
                    ModelPolicyDigest::from_persisted([0xA2; 32]),
                );
            }),
            ("capability_profile.id", |spec| {
                spec.capability_profile =
                    CapabilityProfileRef::new(id(0xB1), spec.capability_profile.digest());
            }),
            ("capability_profile.digest", |spec| {
                spec.capability_profile = CapabilityProfileRef::new(
                    spec.capability_profile.id(),
                    CapabilityProfileDigest::from_persisted([0xB2; 32]),
                );
            }),
            ("delegation_policy.id", |spec| {
                spec.delegation_policy =
                    DelegationPolicyRef::new(id(0xC1), spec.delegation_policy.digest());
            }),
            ("delegation_policy.digest", |spec| {
                spec.delegation_policy = DelegationPolicyRef::new(
                    spec.delegation_policy.id(),
                    DelegationPolicyDigest::from_persisted([0xC2; 32]),
                );
            }),
            ("managed_config.id", |spec| {
                spec.managed_config = ManagedConfigRef::new(
                    id(0xD1),
                    spec.managed_config.digest(),
                    spec.managed_config.byte_len(),
                );
            }),
            ("managed_config.digest", |spec| {
                spec.managed_config = ManagedConfigRef::new(
                    spec.managed_config.id(),
                    ManagedConfigDigest::from_persisted([0xD2; 32]),
                    spec.managed_config.byte_len(),
                );
            }),
            ("managed_config.byte_len", |spec| {
                spec.managed_config = ManagedConfigRef::new(
                    spec.managed_config.id(),
                    spec.managed_config.digest(),
                    spec.managed_config.byte_len() + 1,
                );
            }),
            ("hook_contract_epoch", |spec| {
                spec.hook_contract_epoch =
                    HookContractEpoch::new(spec.hook_contract_epoch.get() + 1);
            }),
        ];

        let base = hash_loadout(&base_spec());
        for (field, mutate) in cases {
            let mut spec = base_spec();
            mutate(&mut spec);
            assert_ne!(spec, base_spec(), "case `{field}` did not change the spec");
            assert_ne!(
                hash_loadout(&spec),
                base,
                "field `{field}` escapes the loadout digest"
            );
        }
    }

    #[test]
    fn loadout_fields_cannot_be_reflowed_across_their_boundaries() {
        // `worker_kind` and `runtime_kind` are adjacent variable-length fields:
        // without the length prefix these two specs share a pre-image.
        let mut left = base_spec();
        left.worker_kind = WorkerKind::new(token("ab"));
        left.runtime_kind = RuntimeKind::new(token("c"));

        let mut right = base_spec();
        right.worker_kind = WorkerKind::new(token("a"));
        right.runtime_kind = RuntimeKind::new(token("bc"));

        assert_ne!(hash_loadout(&left), hash_loadout(&right));
    }

    #[test]
    fn token_set_digests_are_domain_separated() {
        let subject = token("read");
        let capability = hash_tokens(CAPABILITY_PROFILE_DOMAIN, [&subject]);
        let delegation = hash_tokens(DELEGATION_POLICY_DOMAIN, [&subject]);
        let loadout = hash_tokens(WORKER_LOADOUT_DOMAIN, [&subject]);
        assert_ne!(capability, delegation);
        assert_ne!(capability, loadout);
        assert_ne!(delegation, loadout);
    }

    #[test]
    fn a_capability_set_never_hashes_to_the_same_bytes_as_a_role_set() {
        // Identical token content, different meaning: granting `write` must not
        // produce the digest of a policy that permits delegating to `write`.
        let profile = CapabilityProfile::new(id(1), [CapabilityName::new(token("write"))]);
        let policy = DelegationPolicy::new(id(1), [WorkerRole::new(token("write"))]);
        assert_ne!(
            profile.digest().as_bytes(),
            policy.digest().as_bytes(),
            "capability and delegation digests share a domain"
        );
    }

    #[test]
    fn capability_set_contents_are_bound_by_the_profile_digest() {
        let profile_id: CapabilityProfileId = id(1);
        let empty = CapabilityProfile::new(profile_id, []);
        let read = CapabilityProfile::new(profile_id, [CapabilityName::new(token("read"))]);
        let write = CapabilityProfile::new(profile_id, [CapabilityName::new(token("write"))]);
        let both = CapabilityProfile::new(
            profile_id,
            [
                CapabilityName::new(token("read")),
                CapabilityName::new(token("write")),
            ],
        );
        assert_ne!(empty.digest(), read.digest());
        assert_ne!(read.digest(), write.digest());
        assert_ne!(read.digest(), both.digest());

        // Adjacent members must not be reflowable either.
        let split = CapabilityProfile::new(
            profile_id,
            [
                CapabilityName::new(token("ab")),
                CapabilityName::new(token("c")),
            ],
        );
        let shifted = CapabilityProfile::new(
            profile_id,
            [
                CapabilityName::new(token("a")),
                CapabilityName::new(token("bc")),
            ],
        );
        assert_ne!(split.digest(), shifted.digest());
    }

    #[test]
    fn delegation_set_contents_are_bound_by_the_policy_digest() {
        let policy_id: DelegationPolicyId = id(1);
        let empty = DelegationPolicy::new(policy_id, []);
        let scout = DelegationPolicy::new(policy_id, [WorkerRole::new(token("scout"))]);
        let reviewer = DelegationPolicy::new(policy_id, [WorkerRole::new(token("reviewer"))]);
        let both = DelegationPolicy::new(
            policy_id,
            [
                WorkerRole::new(token("scout")),
                WorkerRole::new(token("reviewer")),
            ],
        );
        assert_ne!(empty.digest(), scout.digest());
        assert_ne!(scout.digest(), reviewer.digest());
        assert_ne!(scout.digest(), both.digest());

        let split = DelegationPolicy::new(
            policy_id,
            [WorkerRole::new(token("ab")), WorkerRole::new(token("c"))],
        );
        let shifted = DelegationPolicy::new(
            policy_id,
            [WorkerRole::new(token("a")), WorkerRole::new(token("bc"))],
        );
        assert_ne!(split.digest(), shifted.digest());
    }

    #[test]
    fn a_rewritten_managed_config_cannot_be_verified() {
        let reference = base_spec().managed_config;
        assert_eq!(
            ManagedConfigVerified::verify(
                reference,
                ManagedConfigDigest::from_persisted([0xEE; 32]),
                CONFIG_LEN,
            )
            .err(),
            Some(ArtifactIntegrityError::DigestMismatch)
        );
    }

    #[test]
    fn a_truncated_managed_config_is_reported_as_a_length_mismatch() {
        // A truncated read has a valid digest *of the truncation*; the shared
        // rule checks length first so the two failures stay distinguishable.
        let reference = base_spec().managed_config;
        assert_eq!(
            ManagedConfigVerified::verify(
                reference,
                ManagedConfigDigest::from_persisted(CONFIG_DIGEST),
                CONFIG_LEN - 1,
            )
            .err(),
            Some(ArtifactIntegrityError::LengthMismatch {
                expected: CONFIG_LEN,
                observed: CONFIG_LEN - 1,
            })
        );
    }

    #[test]
    fn a_proof_for_another_artifact_cannot_admit_a_resume() {
        let loadout = WorkerLoadout::resolve(base_spec());
        let committed = committed(&loadout);

        let other =
            ManagedConfigRef::new(id(0xF00D), ManagedConfigDigest::from_persisted([9; 32]), 64);
        let elsewhere =
            ManagedConfigVerified::verify(other, ManagedConfigDigest::from_persisted([9; 32]), 64)
                .expect("the other artifact verifies against its own reference");

        let refusal = committed
            .admit_resume(committed.fence(), elsewhere)
            .expect_err("a proof for a different artifact vouches for nothing here");
        assert_eq!(refusal.kind(), ConflictKind::ManagedConfigUnverifiable);
    }

    #[test]
    fn a_stale_length_in_the_proof_cannot_admit_a_resume() {
        // Same artifact identity and same digest, different recorded length: the
        // witness must match the launch reference exactly, byte length included.
        let loadout = WorkerLoadout::resolve(base_spec());
        let committed = committed(&loadout);
        let recorded = committed.spec().managed_config;

        let shorter =
            ManagedConfigRef::new(recorded.id(), recorded.digest(), recorded.byte_len() - 1);
        let proof = ManagedConfigVerified::verify(
            shorter,
            ManagedConfigDigest::from_persisted(CONFIG_DIGEST),
            CONFIG_LEN - 1,
        )
        .expect("the proof is self-consistent for the shorter reference");

        let refusal = committed
            .admit_resume(committed.fence(), proof)
            .expect_err("a proof over a different byte length is not this configuration");
        assert_eq!(refusal.kind(), ConflictKind::ManagedConfigUnverifiable);
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
