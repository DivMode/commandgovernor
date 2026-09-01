//! Typed, machine-classifiable store failures.
//!
//! The store distinguishes four kinds of failure, and callers must be able to
//! tell them apart without reading a string:
//!
//! 1. **Domain conflict** — a stale fence. [`StoreError::Conflict`] wraps the
//!    `governor-core` [`Conflict`] unchanged, and the transaction it came from
//!    rolled back, so *zero rows changed*.
//! 2. **Fail closed** — the database is newer than this binary, a migration
//!    checksum drifted, the connection policy is not in force, or a projection
//!    does not match its ledger. The daemon must refuse orchestration.
//! 3. **Corrupt row** — a persisted value cannot be rehydrated. Never echoes
//!    the offending value, which may have come from an untrusted surface.
//! 4. **SQLite** — an operational error from the engine itself.

use std::fmt;

use governor_core::error::Conflict;

/// Result alias for every fallible store operation.
pub type StoreResult<T> = Result<T, StoreError>;

/// A failure from the SQLite authority.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A domain fence rejected the operation. Nothing was mutated.
    #[error("domain conflict: {0}")]
    Conflict(#[from] Conflict),

    /// The database was written by a newer schema epoch than this binary knows.
    ///
    /// The daemon must not migrate down, must not mutate, and must expose an
    /// upgrade-required status (`docs/testing.md` DB-003).
    #[error("database schema epoch {found} is newer than the supported epoch {supported}")]
    SchemaEpochTooNew {
        /// Epoch recorded in the database.
        found: u32,
        /// Highest epoch this binary implements.
        supported: u32,
    },

    /// An already-applied migration's recorded checksum does not match the
    /// migration this binary carries.
    ///
    /// Either the file was edited after release or the database came from a
    /// different build. Applying anything on top would be guesswork.
    #[error("migration {version} ({name}) was applied with a different definition")]
    MigrationChecksumMismatch {
        /// Migration version that drifted.
        version: u32,
        /// Migration name recorded in the database.
        name: String,
    },

    /// The database records a migration version this binary does not carry.
    #[error("database has migration {version} applied, which this binary does not know")]
    UnknownAppliedMigration {
        /// Version recorded in the database.
        version: u32,
    },

    /// A required connection pragma is not actually in force.
    #[error("connection policy violated: {0}")]
    ConnectionPolicy(#[from] PolicyViolation),

    /// A persisted value could not be rehydrated into its domain type.
    #[error("corrupt persisted value: {0}")]
    Corrupt(#[from] CorruptValue),

    /// Stored projections disagree with the event ledger.
    ///
    /// `docs/architecture.md` startup order step 5: projection mismatch on
    /// startup fails closed. The daemon enters repair, it does not continue.
    #[error("{0}")]
    RepairNeeded(#[from] RepairNeeded),

    /// Startup quarantine found more orphaned effects than it prepared for.
    ///
    /// `docs/state-machines.md` invariant 12: an attempt left `claimed` or
    /// `activation_armed` still carries an I/O permit, so a quarantine that
    /// could not drain must refuse rather than report a partial success. The
    /// transaction rolled back and the next start re-counts.
    #[error("startup quarantine prepared {minted} identities and needed more")]
    QuarantineIncomplete {
        /// Identities the pass had minted before it ran out.
        minted: usize,
    },

    /// The writer actor is not running.
    #[error("store writer actor is not running")]
    WriterGone,

    /// An operational error from SQLite itself.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

impl StoreError {
    /// Returns the domain conflict, when this failure is one.
    ///
    /// Callers that must distinguish "stale fence, try again with fresh state"
    /// from "the store is broken" branch on this rather than on the message.
    #[must_use]
    pub const fn as_conflict(&self) -> Option<&Conflict> {
        match self {
            Self::Conflict(conflict) => Some(conflict),
            _ => None,
        }
    }

    /// Returns the stable `snake_case` conflict code, when this is a conflict.
    #[must_use]
    pub const fn conflict_code(&self) -> Option<&'static str> {
        match self {
            Self::Conflict(conflict) => Some(conflict.code()),
            _ => None,
        }
    }

    /// Reports whether the daemon must refuse orchestration until a human or an
    /// upgrade intervenes.
    #[must_use]
    pub const fn is_fail_closed(&self) -> bool {
        matches!(
            self,
            Self::SchemaEpochTooNew { .. }
                | Self::MigrationChecksumMismatch { .. }
                | Self::UnknownAppliedMigration { .. }
                | Self::ConnectionPolicy(_)
                | Self::Corrupt(_)
                | Self::RepairNeeded(_)
                | Self::QuarantineIncomplete { .. }
        )
    }
}

/// A pragma the store requires is not in force on the open connection.
///
/// Checked by querying the engine back, not by trusting that the `PRAGMA`
/// statement was accepted: several pragmas silently no-op in conditions the
/// application would otherwise never notice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("PRAGMA {pragma} is {observed}, required {required}")]
pub struct PolicyViolation {
    /// Pragma that was checked.
    pub pragma: &'static str,
    /// Value the engine reported.
    pub observed: String,
    /// Value the store requires.
    pub required: &'static str,
}

/// A stored value could not be rehydrated into its domain type.
///
/// Carries table, column and a bounded reason, never the value: a malformed
/// value may have arrived from a provider surface and must not be echoed into a
/// log line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{table}.{column}: {reason}")]
pub struct CorruptValue {
    /// Table the value came from.
    pub table: &'static str,
    /// Column the value came from.
    pub column: &'static str,
    /// Bounded description of what was wrong.
    pub reason: CorruptReason,
}

impl CorruptValue {
    /// Builds a corruption report for one column.
    #[must_use]
    pub const fn new(table: &'static str, column: &'static str, reason: CorruptReason) -> Self {
        Self {
            table,
            column,
            reason,
        }
    }
}

/// Why a stored value could not be rehydrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CorruptReason {
    /// The text is not a canonical opaque identity.
    #[error("not a canonical identity")]
    MalformedIdentity,
    /// The text is outside the redaction-safe token charset.
    #[error("not a redaction-safe token")]
    UnsafeToken,
    /// The text is not one of the closed set of labels for this column.
    #[error("not a known label for this column")]
    UnknownLabel,
    /// A signed SQLite integer does not fit the domain type.
    #[error("integer out of range for the domain type")]
    IntegerOutOfRange,
    /// The safe-metadata document is not the flat typed object this kind uses.
    #[error("malformed safe metadata document")]
    MalformedMetadata,
    /// An allowlisted safe-metadata field this event kind requires is absent.
    #[error("required safe metadata field is missing")]
    MissingMetadataField,
    /// A row that a foreign key or projection requires does not exist.
    #[error("referenced row is missing")]
    DanglingReference,
    /// The persisted evidence no longer proves what the recorded event claimed.
    #[error("persisted evidence no longer supports the recorded transition")]
    UnprovableEvidence,
}

/// Stored projections disagree with the ledger they are derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairNeeded {
    /// Every disagreement found, in discovery order.
    pub mismatches: Vec<ProjectionMismatch>,
}

impl fmt::Display for RepairNeeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "projection replay found {} disagreement(s) with the event ledger",
            self.mismatches.len()
        )?;
        if let Some(first) = self.mismatches.first() {
            write!(f, "; first: {first}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RepairNeeded {}

/// One stored projection field that does not match its replayed value.
///
/// # What may go in here
///
/// This struct is an *output* surface, not a diagnostic scratchpad: its
/// [`Display`](fmt::Display) reaches [`RepairNeeded`], which the daemon prints
/// to stderr and writes to its log. `table` and `column` are `&'static str`
/// chosen by the comparison, and `stored` and `replayed` are rendered from
/// closed domain labels — no free-form value reaches either.
///
/// `row` is a `String`, so it is the one field a caller could get wrong. It
/// must be a **non-secret** identity of the disagreeing row: an opaque domain
/// identity, or a deterministic key such as
/// [`governor_core::delivery::DeliveryKey`]. It must never be a possession
/// fence — in particular never a
/// [`DeliveryId`](governor_core::delivery::DeliveryId), whose hex a
/// `foreman_resume` caller can present as proof of possession, and which for
/// that reason has no `Display` of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionMismatch {
    /// Projection table that disagrees.
    pub table: &'static str,
    /// Non-secret identity of the disagreeing row. See the type docs.
    pub row: String,
    /// Column that disagrees.
    pub column: &'static str,
    /// What the projection row holds.
    pub stored: String,
    /// What replaying the ledger produces.
    pub replayed: String,
}

impl fmt::Display for ProjectionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}].{} stored {} but ledger replays to {}",
            self.table, self.row, self.column, self.stored, self.replayed
        )
    }
}
