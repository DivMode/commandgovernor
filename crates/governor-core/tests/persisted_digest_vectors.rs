//! Frozen pre-images for the digests that are already written to disk.
//!
//! The wake key, the mutation fingerprint and the resource identity are durable
//! identities: a row keyed on one of them, or a unique index over one, keeps
//! meaning only while the pre-image that produced it stays byte-for-byte the
//! same. Refactoring how the bytes are absorbed is allowed; changing *which*
//! bytes are absorbed is a protocol break, and these vectors are what turns that
//! break into a failing test rather than a silent divergence between a running
//! deployment and a new build.
//!
//! A failure here is never fixed by re-recording the vector.

use governor_core::delivery::DeliveryKey;
use governor_core::fence::{BindingGeneration, DeliveryRevision, SafeToken};
use governor_core::id::ObligationId;
use governor_core::lease::{ResourceIdentity, ResourceNamespace};
use governor_core::mutation::{MutationCommandKind, MutationFingerprint};
use uuid::Uuid;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn token(value: &str) -> SafeToken {
    SafeToken::new(value).expect("test token is safe")
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
