//! The transaction shape every write goes through.
//!
//! # Two phases, and the port boundary between them
//!
//! ```text
//! prepare(request, &mut StorePorts)   <- outside the transaction; clock, CSPRNG
//!   |                                    and identity minting are reachable here
//!   v
//! BEGIN IMMEDIATE                     <- the write lock is taken before the
//!   |                                    first read the mutation depends on
//!   v
//! commit(&Tx)                         <- inside the transaction; no ports are
//!   |                                    in scope, so no clock, no entropy,
//!   v                                    and no adapter can be reached
//! COMMIT
//!   |
//!   v
//! finish(self, committed)             <- strictly after COMMIT returned Ok
//! ```
//!
//! `BEGIN IMMEDIATE` is not an optimisation. `docs/data-model.md`: *transactions
//! that compare a projection and mutate it acquire the write lock before the
//! state read they depend on*. A deferred transaction would read under a shared
//! lock and could be beaten to the write, turning a compare-and-swap into a
//! lost update.
//!
//! # Why there is a third phase
//!
//! [`WriteOp::finish`] exists for exactly one thing: surrendering a
//! [`governor_core::effect::DurableIntentAccepted`]. That value is the store's
//! assertion that an intent row is committed, and `governor-core` cannot check
//! it. Putting the assertion in a phase the runner calls *only* after
//! `Transaction::commit` returned `Ok` makes the ordering structural rather than
//! a rule somebody has to remember. `finish` consumes the operation by value, so
//! the acceptance — and therefore the permit — moves out exactly once.
//!
//! Every other operation's `finish` is the identity function.

use rusqlite::{Connection, Transaction};

use crate::error::StoreResult;
use crate::ports::StorePorts;

/// One durable write, split across the port boundary.
///
/// Implementors are the value produced by `prepare`: everything the transaction
/// body will need, already gathered. That is the point — the body receives the
/// prepared value and a [`Tx`], and nothing else.
pub(crate) trait WriteOp: Sized {
    /// What the caller asks for.
    type Request;
    /// What the transaction body produces, before the commit is durable.
    type Committed;
    /// What the caller finally receives.
    type Output;

    /// Stable name of this operation, used in failpoint reports.
    const NAME: &'static str;

    /// Gathers everything the transaction will need.
    ///
    /// Runs **before** `BEGIN IMMEDIATE` and is the only place a port may be
    /// touched: identities minted here, instants read here, entropy drawn here.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::error::StoreError`] when the request itself cannot be
    /// turned into a durable intent.
    fn prepare(request: Self::Request, ports: &mut StorePorts) -> StoreResult<Self>;

    /// Performs the compare-then-mutate inside the write transaction.
    ///
    /// Note the signature: no [`StorePorts`], no `&mut self`, no adapter. A
    /// rejected fence returns [`crate::error::StoreError::Conflict`] and the
    /// caller rolls the transaction back, so zero rows change.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::error::StoreError`] for a stale fence, a corrupt row,
    /// or a SQLite failure.
    fn commit(&self, tx: &Tx<'_>) -> StoreResult<Self::Committed>;

    /// Runs strictly after the transaction is durable.
    ///
    /// The runner calls this only when `COMMIT` returned `Ok`, so a value that
    /// asserts durability may be produced here and nowhere else.
    fn finish(self, committed: Self::Committed) -> Self::Output;
}

/// A held write transaction.
///
/// The only capability it exposes is the SQLite connection it wraps. There is
/// deliberately no accessor for a clock, a random source, or an identity
/// generator: see the module docs.
pub(crate) struct Tx<'a> {
    inner: &'a Transaction<'a>,
    hook: Option<&'a dyn FailpointHook>,
    op: &'static str,
}

impl<'a> Tx<'a> {
    pub(crate) const fn new(
        inner: &'a Transaction<'a>,
        hook: Option<&'a dyn FailpointHook>,
        op: &'static str,
    ) -> Self {
        Self { inner, hook, op }
    }

    /// The connection inside the transaction.
    pub(crate) fn conn(&self) -> &Connection {
        self.inner
    }

    /// Announces that the transaction body reached a named point.
    ///
    /// With no hook installed this is a branch on `None`. With one installed, a
    /// test can make the transaction fail exactly here, which is the seam the
    /// deterministic crash matrix plugs into.
    ///
    /// # Errors
    ///
    /// Returns whatever the installed hook decides.
    pub(crate) fn reach(&self, point: Failpoint) -> StoreResult<()> {
        match self.hook {
            Some(hook) => hook.reached(self.op, point),
            None => Ok(()),
        }
    }
}

/// A named point inside a transaction body.
///
/// These are the boundaries a crash matrix cares about: either the whole
/// multi-row transition commits, or none of it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Failpoint {
    /// The immutable event row has been appended, nothing else has run.
    AfterEventAppend,
    /// Every projection row has been written, `COMMIT` has not run.
    AfterProjectionUpdate,
    /// Immediately before `COMMIT`.
    BeforeCommit,
    /// Immediately before a migration's `schema_migrations` row is written.
    BeforeMigrationRecorded,
    /// The external-effect intent row has been inserted, `COMMIT` has not run.
    ///
    /// Failing here is the "kill after intent decided, before it is durable"
    /// window: reopening must find no attempt at all, and therefore no permit
    /// was ever surrendered.
    AfterIntentInsert,
    /// The mutation-command `received` row has been inserted, `COMMIT` has not
    /// run. Failing here is the window before the journal can detect the
    /// command at all.
    AfterMutationReceived,
    /// The mutation-command safe result has been written, `COMMIT` has not run.
    ///
    /// Failing here is the "kill after the effect, before the outcome commit"
    /// window: reopening must find `received`, which recovery turns into
    /// `uncertain` and never redispatches.
    AfterMutationResult,
}

/// Injectable interruption for the transaction bodies.
///
/// The deterministic crash matrix (`docs/testing.md` DB-002 and DB-004) is the
/// testkit's to build; this crate supplies the seam it attaches to, and nothing
/// more. With no hook installed every point is inert.
pub trait FailpointHook: Send + Sync {
    /// Called when `op` reaches `point`.
    ///
    /// Return `Ok(())` to continue, or an error to abort the transaction. An
    /// aborted transaction rolls back, which is exactly the property under
    /// test.
    ///
    /// # Errors
    ///
    /// Returns the failure the hook wants injected.
    fn reached(&self, op: &'static str, point: Failpoint) -> StoreResult<()>;
}
