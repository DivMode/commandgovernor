//! Opaque, daemon-allocated storage keys.
//!
//! `docs/data-model.md`: *the daemon allocates `storage_ref`; workers never
//! supply filesystem paths*. [`StorageKey`] is how that is enforced rather than
//! asserted — it is the only thing this crate will turn into a filename, and
//! there is no API anywhere that accepts a [`Path`](std::path::Path).
//!
//! # What the type refuses
//!
//! A key starts as a [`SafeToken`], whose charset already excludes whitespace,
//! quotes, control characters, `/` and `\`. That alone rules out traversal
//! through a separator and any absolute path. On top of it a key must not be:
//!
//! - `.` or `..` — the two names that mean "somewhere else";
//! - anything starting with `.` — hidden names, and the prefix of the two
//!   above;
//! - anything containing `:` — legacy HFS treats it as a path separator, and
//!   some macOS APIs still translate it;
//! - longer than [`MAX_STORAGE_KEY_LEN`], so the composed name stays inside
//!   `NAME_MAX` on every filesystem the daemon may sit on.
//!
//! The result is a single path component that can only ever name a child of
//! the directory it is joined to.

use core::fmt;

use governor_core::fence::{SafeToken, UnsafeToken};

/// Longest accepted storage key.
///
/// Well inside `NAME_MAX` (255 on APFS and ext4) even once the staging suffix
/// is appended, and inside [`SafeToken`]'s own 128-byte limit.
pub const MAX_STORAGE_KEY_LEN: usize = 96;

/// Longest staging suffix [`crate::ArtifactStore`] appends to a key.
pub(crate) const STAGING_SUFFIX_MAX_LEN: usize = 40;

/// A daemon-allocated opaque name for one stored artifact.
///
/// Cheap to clone, ordered, and printable. It is *not* a path and cannot be
/// turned into one except by this crate, joined to a directory it owns.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageKey(SafeToken);

impl StorageKey {
    /// Validates an already-safe token as a single-component storage key.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStorageKey`] for a dot name, a hidden name, a name
    /// containing `:`, or a name longer than [`MAX_STORAGE_KEY_LEN`].
    pub fn new(token: SafeToken) -> Result<Self, InvalidStorageKey> {
        let value = token.as_str();
        if value.len() > MAX_STORAGE_KEY_LEN {
            return Err(InvalidStorageKey::TooLong { len: value.len() });
        }
        if value == "." || value == ".." {
            return Err(InvalidStorageKey::DotName);
        }
        if value.starts_with('.') {
            return Err(InvalidStorageKey::HiddenName);
        }
        if let Some(position) = value.bytes().position(|byte| byte == b':') {
            return Err(InvalidStorageKey::ForbiddenCharacter { position });
        }
        Ok(Self(token))
    }

    /// Validates untrusted text as a storage key.
    ///
    /// This is the entry point for anything that did not come from the key
    /// source: a `storage_ref` read back out of SQLite, or a filename found
    /// during an orphan scan. Both are untrusted — a database row can be
    /// edited and a directory entry can be created by anyone with the
    /// directory's write bit.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStorageKey`] when the text is not a [`SafeToken`], or
    /// not a legal single component.
    pub fn parse(value: &str) -> Result<Self, InvalidStorageKey> {
        Self::new(SafeToken::new(value)?)
    }

    /// The key as the opaque token the durable model stores.
    #[must_use]
    pub const fn as_token(&self) -> &SafeToken {
        &self.0
    }

    /// The key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for StorageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A value was refused as a storage key.
///
/// The rejected text is never echoed: it may have come from an untrusted
/// surface, and reporting it would put it in a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InvalidStorageKey {
    /// The text is not a redaction-safe token: it contains whitespace, a
    /// quote, a control character, or a path separator.
    #[error("not a safe token: {0}")]
    Unsafe(#[from] UnsafeToken),
    /// The key was `.` or `..`.
    #[error("a dot name is not a storage key")]
    DotName,
    /// The key began with `.`.
    #[error("a hidden name is not a storage key")]
    HiddenName,
    /// The key contained a character legal in a token but not in a filename.
    #[error("forbidden filename character at byte {position}")]
    ForbiddenCharacter {
        /// Byte offset of the first offending character.
        position: usize,
    },
    /// The key was longer than [`MAX_STORAGE_KEY_LEN`].
    #[error("key is {len} bytes, limit is {MAX_STORAGE_KEY_LEN}")]
    TooLong {
        /// Length of the rejected key.
        len: usize,
    },
}

/// A source of fresh, unique storage keys.
///
/// # Why this crate ships no default
///
/// Same rule as `governor-store-sqlite`'s [`StorePorts`]: a component that
/// could reach entropy on its own makes "everything ambient is injected" a
/// convention again. The daemon supplies a real one at composition time; tests
/// supply a deterministic one, which is what makes the crash matrix
/// reproducible.
///
/// Uniqueness is the implementor's contract. It is not *trusted*: a key that
/// turns out to be taken fails closed with
/// [`ArtifactError::AlreadyPublished`](crate::ArtifactError::AlreadyPublished),
/// because publication reserves the immutable name with an exclusive create.
///
/// [`StorePorts`]: governor_store_sqlite::StorePorts
pub trait StorageKeySource: Send {
    /// Allocates the next opaque key.
    fn next_key(&mut self) -> StorageKey;
}

impl<T: StorageKeySource + ?Sized> StorageKeySource for Box<T> {
    fn next_key(&mut self) -> StorageKey {
        (**self).next_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(value: &str) -> InvalidStorageKey {
        StorageKey::parse(value).expect_err("must be refused")
    }

    #[test]
    fn a_plain_opaque_name_is_accepted() {
        assert_eq!(
            StorageKey::parse("ra-0001-abcdef").expect("valid").as_str(),
            "ra-0001-abcdef"
        );
    }

    #[test]
    fn traversal_and_separators_are_refused() {
        assert_eq!(refused(".."), InvalidStorageKey::DotName);
        assert_eq!(refused("."), InvalidStorageKey::DotName);
        assert!(matches!(
            refused("../../etc/passwd"),
            InvalidStorageKey::Unsafe(_)
        ));
        assert!(matches!(refused("a/b"), InvalidStorageKey::Unsafe(_)));
        assert!(matches!(
            refused("/etc/passwd"),
            InvalidStorageKey::Unsafe(_)
        ));
        assert!(matches!(refused("a\\b"), InvalidStorageKey::Unsafe(_)));
    }

    #[test]
    fn hidden_names_and_legacy_separators_are_refused() {
        assert_eq!(refused(".hidden"), InvalidStorageKey::HiddenName);
        assert_eq!(
            refused("volume:file"),
            InvalidStorageKey::ForbiddenCharacter { position: 6 }
        );
    }

    #[test]
    fn an_overlong_key_is_refused_before_it_reaches_a_filesystem() {
        let long = "a".repeat(MAX_STORAGE_KEY_LEN + 1);
        assert_eq!(
            refused(&long),
            InvalidStorageKey::TooLong {
                len: MAX_STORAGE_KEY_LEN + 1
            }
        );
        // `NAME_MAX` is 255 on APFS and on every filesystem the daemon may sit
        // on, and a staging name is a key plus the suffix budget.
        const { assert!(MAX_STORAGE_KEY_LEN + STAGING_SUFFIX_MAX_LEN <= 255) };
    }
}
