//! Fences: the explicit data that makes a stale actor harmless.
//!
//! Per [`docs/data-model.md`] principle 4, *fences are explicit data*. Session
//! incarnation, turn, source event, obligation version, binding generation,
//! wake revision and foreman claim are all represented directly rather than
//! inferred from timestamps or names. This module holds the primitives every
//! machine fences with.
//!
//! It also holds [`SafeToken`], the only string-shaped value this crate will
//! carry. Its charset excludes spaces and path separators, so a prompt, a shell
//! command, a cwd, or a transcript path cannot be smuggled through a field that
//! was meant for a provider's opaque reference.
//!
//! [`docs/data-model.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/data-model.md

use core::fmt;
use std::collections::BTreeSet;

/// Longest accepted [`SafeToken`].
///
/// Provider identities are short. A generous-but-bounded limit means a caller
/// that tries to route free text through an identity field fails loudly.
pub const SAFE_TOKEN_MAX_LEN: usize = 128;

/// A bounded, opaque, redaction-safe token.
///
/// Accepted characters are ASCII alphanumerics and `-`, `_`, `.`, `:`, `@`,
/// `+`, `=`. Whitespace, control characters, quotes, and `/` are refused, which
/// structurally rules out prose, shell commands, and filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SafeToken(String);

impl SafeToken {
    /// Validates and wraps a provider-supplied opaque token.
    ///
    /// # Errors
    ///
    /// Returns [`UnsafeToken`] when the value is empty, longer than
    /// [`SAFE_TOKEN_MAX_LEN`], or contains a character outside the allowed set.
    pub fn new(value: &str) -> Result<Self, UnsafeToken> {
        if value.is_empty() {
            return Err(UnsafeToken::Empty);
        }
        if value.len() > SAFE_TOKEN_MAX_LEN {
            return Err(UnsafeToken::TooLong { len: value.len() });
        }
        if let Some(position) = value.bytes().position(|b| !Self::is_allowed(b)) {
            return Err(UnsafeToken::ForbiddenCharacter { position });
        }
        Ok(Self(value.into()))
    }

    /// Returns the token text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    const fn is_allowed(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@' | b'+' | b'=')
    }
}

impl fmt::Display for SafeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A value was rejected as unsafe for durable control-plane storage.
///
/// The rejected text is never included: reporting it would defeat the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UnsafeToken {
    /// The value was empty.
    #[error("token is empty")]
    Empty,
    /// The value exceeded [`SAFE_TOKEN_MAX_LEN`].
    #[error("token is {len} bytes, limit is {SAFE_TOKEN_MAX_LEN}")]
    TooLong {
        /// Length of the rejected value.
        len: usize,
    },
    /// The value contained a character outside the allowed set.
    #[error("token contains a forbidden character at byte {position}")]
    ForbiddenCharacter {
        /// Byte offset of the first forbidden character.
        position: usize,
    },
}

/// Identity of the external fact that justified a domain event.
///
/// Every accepted event carries one. The triple is unique in the durable
/// ledger, which is how a replayed provider callback becomes a no-op instead of
/// a second obligation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceRef {
    namespace: SafeToken,
    event: SafeToken,
    fence: SafeToken,
}

impl SourceRef {
    /// Builds a source identity from its three opaque parts.
    #[must_use]
    pub const fn new(namespace: SafeToken, event: SafeToken, fence: SafeToken) -> Self {
        Self {
            namespace,
            event,
            fence,
        }
    }

    /// Provider/adapter namespace that issued the fact.
    #[must_use]
    pub const fn namespace(&self) -> &SafeToken {
        &self.namespace
    }

    /// Opaque provider-native event identity within the namespace.
    #[must_use]
    pub const fn event(&self) -> &SafeToken {
        &self.event
    }

    /// Opaque fence distinguishing revisions of the same source event.
    #[must_use]
    pub const fn fence(&self) -> &SafeToken {
        &self.fence
    }
}

impl fmt::Display for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.namespace, self.event, self.fence)
    }
}

/// Declares a monotonic counter fence.
macro_rules! monotonic {
    ($( $(#[$doc:meta])* $name:ident ( $repr:ty ) starting $first:expr ),* $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
            pub struct $name($repr);

            impl $name {
                #[doc = "The value every sequence starts at."]
                pub const FIRST: Self = Self($first);

                #[doc = "Wraps a persisted counter value."]
                #[must_use]
                pub const fn new(value: $repr) -> Self {
                    Self(value)
                }

                #[doc = "Returns the counter value, for persistence and diagnostics."]
                #[must_use]
                pub const fn get(self) -> $repr {
                    self.0
                }

                #[doc = "Returns the next value in the sequence."]
                #[doc = ""]
                #[doc = "Saturates at the representation's maximum rather than wrapping:"]
                #[doc = "a wrapped generation would let a superseded actor look current."]
                #[must_use]
                pub const fn next(self) -> Self {
                    Self(self.0.saturating_add(1))
                }
            }

            impl fmt::Display for $name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    fmt::Display::fmt(&self.0, f)
                }
            }
        )*
    };
}

monotonic! {
    /// Monotonic generation of the single active foreman binding.
    ///
    /// A rebind increments it and supersedes every older generation, so an old
    /// conversation can no longer mutate current work.
    BindingGeneration(u64) starting 1,

    /// Compare-and-swap version of an obligation.
    ///
    /// Every accepted obligation transition advances it; a foreman mutation
    /// must present the exact current value.
    ObligationVersion(u64) starting 1,

    /// Revision of a browser wake for one obligation and binding generation.
    ///
    /// A later resume is a new revision — never a replay of an old one.
    DeliveryRevision(u32) starting 1,

    /// Attempt counter within one delivery revision.
    AttemptNo(u32) starting 1,

    /// Generation of a session incarnation. Runtime replacement increments it.
    IncarnationGeneration(u64) starting 1,

    /// Generation of a turn within a session incarnation.
    TurnGeneration(u64) starting 1,

    /// Revision of a worker continuation command.
    CommandRevision(u32) starting 1,

    /// Revision of an input request for one turn and source event.
    RequestRevision(u32) starting 1,

    /// Daemon-assigned authoritative ordering of the durable event ledger.
    EventSeq(u64) starting 1,

    /// Lifetime counter of the owning daemon process.
    ///
    /// Startup advances it once, and every mutation, external-effect intent and
    /// resource lease records the epoch it was made under. A daemon from an
    /// older epoch is superseded and cannot mutate current state, which is what
    /// keeps a stranded process from acting on work the current daemon owns.
    DaemonEpoch(u64) starting 1,
}

/// Pure index of source identities already applied.
///
/// The durable enforcement of this rule is the ledger's
/// `UNIQUE(source_namespace, source_event_id, source_event_fence)` index. This
/// type is the same rule at the pure level, so replay and duplicate-delivery
/// behaviour can be proven without a database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceLedger {
    seen: BTreeSet<SourceRef>,
}

impl SourceLedger {
    /// Creates an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a source identity, reporting whether it is new.
    ///
    /// Returns `true` when the caller should apply the event, and `false` when
    /// it is a duplicate that must be ignored.
    pub fn admit(&mut self, source: &SourceRef) -> bool {
        self.seen.insert(source.clone())
    }

    /// Reports whether a source identity has already been applied.
    #[must_use]
    pub fn contains(&self, source: &SourceRef) -> bool {
        self.seen.contains(source)
    }

    /// Number of distinct source identities recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Reports whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{SafeToken, SourceRef};

    /// Builds a source identity for tests from three short labels.
    pub(crate) fn source(namespace: &str, event: &str, fence: &str) -> SourceRef {
        SourceRef::new(
            SafeToken::new(namespace).expect("test namespace is safe"),
            SafeToken::new(event).expect("test event id is safe"),
            SafeToken::new(fence).expect("test fence is safe"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::source;
    use super::*;

    #[test]
    fn accepts_opaque_provider_identities() {
        for value in [
            "claude.hook",
            "toolu_01ABCdef",
            "turn-7",
            "conv:68e0c1",
            "cg@1.0+abc=",
        ] {
            assert!(SafeToken::new(value).is_ok(), "{value} should be accepted");
        }
    }

    #[test]
    fn refuses_shapes_that_could_carry_forbidden_content() {
        // Prose, shell commands, cwd, transcript paths and quoted payloads all
        // fail on the charset before length is even considered.
        let forbidden = [
            "rm -rf /tmp",
            "/Users/peter/.claude/transcript.jsonl",
            "please ACK this",
            "{\"prompt\":\"hi\"}",
            "line\nbreak",
        ];
        for value in forbidden {
            assert!(
                matches!(
                    SafeToken::new(value),
                    Err(UnsafeToken::ForbiddenCharacter { .. })
                ),
                "{value} should be refused"
            );
        }
    }

    #[test]
    fn refuses_empty_and_oversized_values() {
        assert_eq!(SafeToken::new(""), Err(UnsafeToken::Empty));
        let long = "a".repeat(SAFE_TOKEN_MAX_LEN + 1);
        assert_eq!(
            SafeToken::new(&long),
            Err(UnsafeToken::TooLong {
                len: SAFE_TOKEN_MAX_LEN + 1
            })
        );
    }

    #[test]
    fn rejection_never_echoes_the_rejected_value() {
        let err = SafeToken::new("secret-token value").unwrap_err();
        assert!(!err.to_string().contains("secret-token"));
    }

    #[test]
    fn generations_are_monotonic_and_saturate() {
        assert_eq!(BindingGeneration::FIRST.get(), 1);
        assert_eq!(BindingGeneration::FIRST.next().get(), 2);
        assert!(BindingGeneration::FIRST.next() > BindingGeneration::FIRST);
        let max = BindingGeneration::new(u64::MAX);
        assert_eq!(max.next(), max, "saturates rather than wrapping to zero");
    }

    #[test]
    fn source_ledger_admits_each_identity_once() {
        let mut ledger = SourceLedger::new();
        let terminal = source("claude.result", "run-1", "final");
        assert!(ledger.admit(&terminal));
        for _ in 0..100 {
            assert!(!ledger.admit(&terminal), "replay must not be admitted");
        }
        assert_eq!(ledger.len(), 1);

        // A different fence for the same event id is a different fact.
        assert!(ledger.admit(&source("claude.result", "run-1", "revised")));
        assert_eq!(ledger.len(), 2);
    }
}
