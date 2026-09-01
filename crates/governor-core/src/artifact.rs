//! Immutable result artifacts and the retention rule that pins them.
//!
//! The bytes live outside this crate; what lives here is the metadata, the
//! integrity check, and invariant 2: **a required result artifact cannot be
//! released while an open obligation references it**. Retention is therefore
//! derived from the obligations, never set independently.

use crate::fence::SafeToken;
use crate::id::ResultArtifactId;
use crate::obligation::Obligation;
use crate::time::Timestamp;

/// SHA-256 digest of an artifact's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactDigest([u8; 32]);

impl ArtifactDigest {
    /// Wraps a computed digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Whether an artifact may be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetentionState {
    /// At least one open obligation needs it. Garbage collection must not run.
    Pinned,
    /// No open obligation needs it; policy delay then applies.
    Eligible,
}

/// Metadata for one immutable result artifact.
///
/// `storage_ref` is a daemon-allocated opaque key: a worker never supplies a
/// filesystem path, and none is representable here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultArtifact {
    id: ResultArtifactId,
    storage_ref: SafeToken,
    digest: ArtifactDigest,
    byte_len: u64,
    created_at: Timestamp,
}

impl ResultArtifact {
    /// Records metadata for an artifact that has already been made durable.
    #[must_use]
    pub const fn new(
        id: ResultArtifactId,
        storage_ref: SafeToken,
        digest: ArtifactDigest,
        byte_len: u64,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            storage_ref,
            digest,
            byte_len,
            created_at,
        }
    }

    /// Artifact identity.
    #[must_use]
    pub const fn id(&self) -> ResultArtifactId {
        self.id
    }

    /// Daemon-allocated opaque storage key.
    #[must_use]
    pub const fn storage_ref(&self) -> &SafeToken {
        &self.storage_ref
    }

    /// Expected digest of the stored bytes.
    #[must_use]
    pub const fn digest(&self) -> ArtifactDigest {
        self.digest
    }

    /// Expected length of the stored bytes.
    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Instant the artifact was published.
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Verifies observed bytes against the recorded digest and length.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactIntegrityError`] on any mismatch. Callers must fail
    /// closed: a mismatched artifact leaves its obligation open and raises
    /// [`crate::health::HealthConditionKind::ResultArtifactMissing`].
    pub fn verify(
        &self,
        observed_digest: ArtifactDigest,
        observed_len: u64,
    ) -> Result<(), ArtifactIntegrityError> {
        ArtifactIntegrityError::check(self.digest, self.byte_len, observed_digest, observed_len)
    }

    /// Derives retention from the obligations that reference this artifact.
    ///
    /// Passing every obligation in the projection is the intended use: the
    /// function selects the ones that reference this artifact, so a caller
    /// cannot accidentally omit the pinning one by filtering first.
    #[must_use]
    pub fn retention<'a>(
        &self,
        obligations: impl IntoIterator<Item = &'a Obligation>,
    ) -> RetentionState {
        let pinned = obligations.into_iter().any(|obligation| {
            obligation.result_artifact() == Some(self.id) && obligation.is_open()
        });
        if pinned {
            RetentionState::Pinned
        } else {
            RetentionState::Eligible
        }
    }
}

/// A stored artifact did not match its recorded metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactIntegrityError {
    /// The stored bytes are a different length than recorded.
    #[error("artifact length {observed} does not match recorded {expected}")]
    LengthMismatch {
        /// Length recorded at publication.
        expected: u64,
        /// Length observed now.
        observed: u64,
    },
    /// The stored bytes do not hash to the recorded digest.
    #[error("artifact digest does not match")]
    DigestMismatch,
}

impl ArtifactIntegrityError {
    /// Compares observed bytes against expected metadata.
    ///
    /// The rule [`ResultArtifact::verify`] applies, stated once and usable
    /// without a metadata row — the storage layer verifies bytes it read for a
    /// key whose identity and creation instant play no part in the check, and
    /// inventing a row for it would mean writing the comparison twice.
    ///
    /// Length is checked first deliberately: a truncated read has a perfectly
    /// valid digest *of the truncation*, and reporting that as a digest
    /// mismatch would hide which failure actually happened.
    ///
    /// # Errors
    ///
    /// Returns the mismatch that was found. Callers must fail closed and
    /// return no bytes at all.
    pub fn check(
        expected_digest: ArtifactDigest,
        expected_len: u64,
        observed_digest: ArtifactDigest,
        observed_len: u64,
    ) -> Result<(), Self> {
        if observed_len != expected_len {
            return Err(Self::LengthMismatch {
                expected: expected_len,
                observed: observed_len,
            });
        }
        if observed_digest != expected_digest {
            return Err(Self::DigestMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obligation::test_support as obligation_support;
    use uuid::Uuid;

    fn artifact() -> ResultArtifact {
        ResultArtifact::new(
            obligation_support::artifact_id(),
            SafeToken::new("ra-0001").unwrap(),
            ArtifactDigest::from_bytes([7u8; 32]),
            1_024,
            Timestamp::from_unix_millis(0),
        )
    }

    #[test]
    fn integrity_is_checked_on_both_length_and_digest() {
        let artifact = artifact();
        assert!(
            artifact
                .verify(ArtifactDigest::from_bytes([7u8; 32]), 1_024)
                .is_ok()
        );
        assert_eq!(
            artifact.verify(ArtifactDigest::from_bytes([7u8; 32]), 1_023),
            Err(ArtifactIntegrityError::LengthMismatch {
                expected: 1_024,
                observed: 1_023
            })
        );
        assert_eq!(
            artifact.verify(ArtifactDigest::from_bytes([9u8; 32]), 1_024),
            Err(ArtifactIntegrityError::DigestMismatch)
        );
    }

    #[test]
    fn the_rule_is_usable_without_a_metadata_row_and_checks_length_first() {
        // The storage layer verifies bytes it read for a key whose identity and
        // creation instant play no part in the check. A truncated read has a
        // perfectly valid digest *of the truncation*, so reporting length first
        // is what keeps the two failures distinguishable.
        assert_eq!(
            ArtifactIntegrityError::check(
                ArtifactDigest::from_bytes([7u8; 32]),
                1_024,
                ArtifactDigest::from_bytes([9u8; 32]),
                512,
            ),
            Err(ArtifactIntegrityError::LengthMismatch {
                expected: 1_024,
                observed: 512
            })
        );
        assert_eq!(
            ArtifactIntegrityError::check(
                ArtifactDigest::from_bytes([7u8; 32]),
                1_024,
                ArtifactDigest::from_bytes([7u8; 32]),
                1_024,
            ),
            Ok(())
        );
    }

    #[test]
    fn an_open_obligation_pins_its_artifact() {
        let artifact = artifact();
        let open = obligation_support::completed();
        assert_eq!(artifact.retention([&open]), RetentionState::Pinned);
    }

    #[test]
    fn only_a_closing_disposition_releases_the_artifact() {
        let artifact = artifact();
        let acknowledged = obligation_support::acknowledged();
        assert_eq!(
            artifact.retention([&acknowledged]),
            RetentionState::Eligible
        );
    }

    #[test]
    fn an_unrelated_obligation_does_not_pin() {
        let artifact = ResultArtifact::new(
            ResultArtifactId::from_uuid(Uuid::from_u128(999)),
            SafeToken::new("ra-9999").unwrap(),
            ArtifactDigest::from_bytes([1u8; 32]),
            10,
            Timestamp::from_unix_millis(0),
        );
        let open = obligation_support::completed();
        assert_eq!(artifact.retention([&open]), RetentionState::Eligible);
    }
}
