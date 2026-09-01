//! Length-prefixed absorption shared by every digest pre-image in this crate.
//!
//! The wake key, the mutation fingerprint, the resource identity and the worker
//! loadout are all a SHA-256 over a domain label followed by the fields that
//! identify the thing. Prefixing each field with its length is what makes that
//! encoding injective: concatenated alone, `"ab" + "c"` and `"a" + "bc"` are one
//! pre-image, so two distinct tuples would share a digest that persisted state
//! treats as an identity.
//!
//! The rule lives here once because a second, subtly different copy of it would
//! stay invisible until two components disagreed about a digest already written
//! to disk.

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Absorbs one variable-length field: its byte length as a big-endian `u64`,
/// then its bytes.
pub(crate) fn absorb(hasher: &mut Sha256, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).expect("bounded digest field length fits in u64");
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
}

/// Absorbs an opaque identity as its 16 raw bytes.
///
/// The bytes are absorbed whole; no field of the UUID is read or given meaning.
pub(crate) fn absorb_uuid(hasher: &mut Sha256, value: Uuid) {
    absorb(hasher, value.as_bytes());
}

/// Absorbs a 64-bit counter in big-endian form.
pub(crate) fn absorb_u64(hasher: &mut Sha256, value: u64) {
    absorb(hasher, &value.to_be_bytes());
}

/// Absorbs a 32-bit counter in big-endian form.
///
/// The width is part of the pre-image: a `u32` field absorbs four bytes behind a
/// length of four, and widening it later would be a protocol break rather than a
/// silent re-encoding.
pub(crate) fn absorb_u32(hasher: &mut Sha256, value: u32) {
    absorb(hasher, &value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_of(fields: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for field in fields {
            absorb(&mut hasher, field);
        }
        hasher.finalize().into()
    }

    #[test]
    fn adjacent_variable_length_fields_cannot_be_reflowed() {
        // The whole reason for the length prefix: these two field lists share a
        // naive concatenation and must not share a digest.
        assert_ne!(digest_of(&[b"ab", b"c"]), digest_of(&[b"a", b"bc"]));
    }

    #[test]
    fn an_empty_field_is_still_a_field() {
        assert_ne!(digest_of(&[b"", b"a"]), digest_of(&[b"a"]));
    }

    #[test]
    fn counter_width_is_part_of_the_pre_image() {
        let mut wide = Sha256::new();
        absorb_u64(&mut wide, 1);
        let mut narrow = Sha256::new();
        absorb_u32(&mut narrow, 1);
        let wide: [u8; 32] = wide.finalize().into();
        let narrow: [u8; 32] = narrow.finalize().into();
        assert_ne!(wide, narrow);
    }
}
