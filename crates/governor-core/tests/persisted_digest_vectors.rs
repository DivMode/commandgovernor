//! Frozen digest pre-images, in two classes that must not be confused.
//!
//! # Never change
//!
//! The wake key, the mutation fingerprint and the resource identity are durable
//! identities: a row keyed on one of them, or a unique index over one, keeps
//! meaning only while the pre-image that produced it stays byte-for-byte the
//! same. Refactoring how the bytes are absorbed is allowed; changing *which*
//! bytes are absorbed is a protocol break, and these vectors are what turns that
//! break into a failing test rather than a silent divergence between a running
//! deployment and a new build.
//!
//! **A failure in this class is never fixed by re-recording the vector.**
//!
//! # Frozen for mutation coverage
//!
//! The worker-loadout digest is not persisted anywhere yet — no store slice has
//! shipped — so it is pinned for a different reason: it is the only way to cover
//! a pre-image field that has no second value to differ from. [`ResumePolicy`]
//! has one variant, so no per-field difference test can show that its code is
//! absorbed; deleting that line would otherwise leave the suite green.
//!
//! A failure in this class **may** be fixed by re-recording the vector, before
//! the store slice lands and only with the reason for the pre-image change
//! stated in the commit. After that it joins the class above.

use governor_core::delivery::DeliveryKey;
use governor_core::fence::{BindingGeneration, DeliveryRevision, SafeToken};
use governor_core::id::{
    CapabilityProfileId, DelegationPolicyId, Id, IdKind, ManagedConfigArtifactId, ModelPolicyId,
    ObligationId, WorkerLoadoutId,
};
use governor_core::lease::{ResourceIdentity, ResourceNamespace};
use governor_core::mutation::{MutationCommandKind, MutationFingerprint};
use governor_core::session::{
    CapabilityName, CapabilityProfile, DelegationPolicy, HookContractEpoch, ManagedConfigDigest,
    ManagedConfigRef, ModelPolicyDigest, ModelPolicyRef, ResumePolicy, RuntimeKind, WorkerKind,
    WorkerLoadout, WorkerLoadoutSpec, WorkerRole,
};
use uuid::Uuid;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn token(value: &str) -> SafeToken {
    SafeToken::new(value).expect("test token is safe")
}

fn id<K: IdKind>(value: u128) -> Id<K> {
    Id::from_uuid(Uuid::from_u128(value))
}

#[test]
fn mutation_fingerprint_pre_image_is_frozen() {
    let kind = MutationCommandKind::new(token("ack_obligation"));
    // Deliberately adjacent parameters that share a naive concatenation with
    // `["a", "bc"]`, so a lost length prefix moves this vector.
    let first = token("ab");
    let second = token("c");
    assert_eq!(
        hex(MutationFingerprint::derive(&kind, &[&first, &second]).as_bytes()),
        "dbc1aa23f46cc9a7d98b012fdafc99e07f471c01ab9c15b322dd9601b4c38c87"
    );
}

#[test]
fn resource_identity_pre_image_is_frozen() {
    let namespace = ResourceNamespace::new(token("profile-dir"));
    assert_eq!(
        hex(ResourceIdentity::canonical(namespace, "/tmp/a b").digest()),
        "6a865fa73000ba9cac23420ca84cf22d049497d55c5054cad59cdd3df728abd5"
    );
}

#[test]
fn wake_key_pre_image_is_frozen() {
    let key = DeliveryKey::derive(
        ObligationId::from_uuid(Uuid::from_u128(0x1234)),
        BindingGeneration::new(9),
        DeliveryRevision::new(3),
    );
    assert_eq!(
        hex(key.as_bytes()),
        "6c4e0a797fec80a064cb8f613645485671f0e516b12632a0a953b476a4ce7333"
    );
}

#[test]
fn worker_loadout_pre_image_covers_every_absorbed_field() {
    // Frozen for mutation coverage, not for durability — see the module docs
    // before re-recording it. `resume_policy` is the field that needs this: it
    // has a single variant, so there is no second spec that differs only in it
    // and therefore no per-field difference test that can prove its code is
    // absorbed. Every other field is covered twice, here and by
    // `session::tests::every_spec_field_is_bound_by_the_loadout_digest`.
    let capabilities: CapabilityProfileId = id(30);
    let capabilities = CapabilityProfile::new(capabilities, [CapabilityName::new(token("read"))]);
    let delegation: DelegationPolicyId = id(40);
    let delegation = DelegationPolicy::new(delegation, [WorkerRole::new(token("scout"))]);
    let model_policy: ModelPolicyId = id(2);
    let managed_config: ManagedConfigArtifactId = id(5);
    let loadout_id: WorkerLoadoutId = id(1);

    let loadout = WorkerLoadout::resolve(WorkerLoadoutSpec {
        id: loadout_id,
        worker_kind: WorkerKind::new(token("claude")),
        runtime_kind: RuntimeKind::new(token("herdr")),
        role: WorkerRole::new(token("worker")),
        model_policy: ModelPolicyRef::new(model_policy, ModelPolicyDigest::from_persisted([2; 32])),
        capability_profile: capabilities.reference(),
        delegation_policy: delegation.reference(),
        managed_config: ManagedConfigRef::new(
            managed_config,
            ManagedConfigDigest::from_persisted([5; 32]),
            512,
        ),
        hook_contract_epoch: HookContractEpoch::new(7),
        resume_policy: ResumePolicy::ExactLoadout,
    });

    assert_eq!(
        hex(loadout.digest().as_bytes()),
        "8b4ab6da89ae9546870f2d158e323baacb2b4e2600747a88909299f0e020e73c"
    );
}
