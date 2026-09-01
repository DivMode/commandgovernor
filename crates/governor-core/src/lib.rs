//! Pure domain model and state machines for Command Governor.
//!
//! # Phase 1 role
//!
//! `governor-core` owns the typed domain identities, the immutable event and
//! state types, and the *pure* transition functions behind them: obligation
//! lifecycle, browser delivery lifecycle, binding-generation fencing, foreman
//! claims and ACK, worker answer/resume delivery, and watchdog attention.
//!
//! # Boundary
//!
//! This crate performs no I/O. It must stay independent of SQLite, browsers,
//! networking, process control, the filesystem, GitHub, MCP, Claude, and Herdr,
//! so that every invariant it encodes can be proven by pure tests without a
//! live service. Persistence is the responsibility of `governor-store-sqlite`;
//! composition is the responsibility of `governor-daemon`.
//!
//! Two consequences follow, and both are visible in the API:
//!
//! - **Nothing ambient.** Identity generation goes through [`id::IdSource`],
//!   randomness through [`random::SecureRandom`], and time arrives as a
//!   [`time::Timestamp`] argument. No function here reads a clock or an
//!   entropy source.
//! - **Nothing mutated on rejection.** Every transition borrows its state and
//!   returns a *new* value, so a typed conflict cannot have left a partial
//!   change behind. There is no mutable state for it to have touched.
//!
//! Errors raised here are typed (`thiserror`) and machine-classifiable: a
//! domain conflict such as a stale generation or an ambiguous delivery must be
//! distinguishable by a caller, never flattened into an opaque string. See
//! [`error::ConflictKind`].
//!
//! # Safe data only
//!
//! The durable model has nowhere to put a prompt, a tool argument, a shell
//! command, a cwd, a transcript path, or a credential. Provider-supplied text
//! enters only as [`fence::SafeToken`], whose charset excludes whitespace and
//! path separators, and every event payload is a closed enum of bounded typed
//! fields. This is a structural boundary, not a redaction pass.
//!
//! # Driving the machines
//!
//! Each machine exposes `apply(&self, &Event) -> Result<Transition<Self>,
//! Conflict>`, so a caller — the store, the daemon, or the acceptance testkit —
//! can fold an event sequence over an empty projection and replay it exactly.
//! [`fence::SourceLedger`] provides the same duplicate-source rule the durable
//! ledger enforces with a unique index.
//!
//! # Where the state-machine invariants live
//!
//! | Invariant ([`docs/state-machines.md`]) | Enforced by |
//! | --- | --- |
//! | 1. Open count needs a closing disposition | [`obligation::Obligation::apply`] |
//! | 2. Open obligation pins its artifact | [`artifact::ResultArtifact::retention`] |
//! | 3. Duplicate terminal source is idempotent | [`obligation`], [`fence::SourceLedger`] |
//! | 4. Stop callback alone is not completion | [`worker_evidence::ConfirmedFinalResult`] |
//! | 5. Vetoed Stop candidate is not completion | [`worker_evidence::ManagedRunEvidence::classify`] |
//! | 6. Clean defer needs a single-tool boundary | [`input::ConfirmedDefer`] |
//! | 7. `PermissionRequest` is not a pause identity | [`input::DeferShape`] carries the tool-use fence; permission evidence does not |
//! | 8. Old incarnation cannot mutate current | [`obligation::Obligation::apply`] |
//! | 9. Old generation cannot ACK or answer | [`binding::BindingLedger::fence`] |
//! | 10. `claimed` precedes browser I/O | [`outbound::IoPermit`] |
//! | 11. Send fence is durable before Send | [`outbound::SendActivation`] |
//! | 12. Startup quarantines orphans | [`outbound::DeliveryEvent::OrphanQuarantined`] |
//! | 13. Accepted/ambiguous never resent | [`outbound::DeliveryState::is_frozen`] |
//! | 14. accepted != settled != ACK | [`foreman_turn`] cannot close anything |
//! | 15. Confirmed worker truth beats stale runtime | [`worker_evidence::WorkerEvidenceClass`] |
//! | 16. Watchdog creates attention only | [`watchdog::WatchdogOutcome`] |
//! | 17. Deterministic metadata cannot derive the wake ID | [`delivery::DeliveryId`] |
//!
//! Invariants 2, 3, 10, 11 and 12 also have a durable half — retention sweeps,
//! the source-event unique index, and startup ordering — that belongs to
//! `governor-store-sqlite` and `governor-daemon`. This crate supplies the rule;
//! those crates supply the enforcement point at the I/O boundary.
//!
//! # Durable-execution primitives
//!
//! [`effect`], [`mutation`] and [`lease`] add the provider-independent half of
//! the durable-execution patterns in
//! [`docs/research/2026-08-31-durable-orchestration-pattern-review.md`]:
//!
//! | Rule | Enforced by |
//! | --- | --- |
//! | No consequential I/O before the intent is durable | [`effect::ExternalExecutionPermit`], reachable only from [`effect::DurableIntentAccepted`] |
//! | Unknown fate never projects success | [`effect::ExternalAttemptState::Ambiguous`], terminal, with [`effect::ReconciliationRequired`] |
//! | Retry needs a recorded contract, not a hopeful label | [`effect::ExternalAttempt::admit_retry`] |
//! | An exact retry replays its recorded result | [`mutation::MutationJournal::resolve`] |
//! | An uncertain retry never redispatches | [`mutation::MutationDisposition`] has no dispatch variant |
//! | A receipt ACK only unlocks retention | [`mutation::ReceiptAck`] reaches no obligation transition |
//! | A recycled process cannot impersonate a holder | [`lease::ProcessIncarnation`] |
//! | A superseded daemon cannot mutate ownership | [`fence::DaemonEpoch`] |
//!
//! These are the *generic* forms. Browser wake delivery ([`outbound`],
//! [`delivery`]) is the specialised, already-proven instance of the same
//! external-effect discipline for the one transport V1 ships, and stays as it
//! is; [`effect`] documents the correspondence.
//!
//! Their durable halves — the intent transaction, the
//! `PRIMARY KEY(actor_id, command_id)` journal, kill-window failpoints and
//! replay equivalence — belong to `governor-store-sqlite` and
//! `governor-testkit`.
//!
//! # Session lineage and worker loadouts
//!
//! [`session`] adds the provider-independent half of
//! [`docs/adr/0007-session-lineage-memory-and-analytics.md`] §4–6: a delegated
//! worker's authority is whatever its launch snapshot recorded, never whatever
//! today's configuration happens to resolve to.
//!
//! | Rule | Enforced by |
//! | --- | --- |
//! | Capabilities are whitelist-only; an omitted profile grants nothing | [`session::CapabilityProfile::allows`] over an explicit set, with no default branch |
//! | Delegation is whitelist-only; an omitted child role is not spawnable | [`session::DelegationPolicy::allows`], likewise |
//! | Resume needs the exact launch loadout, not today's | [`session::CommittedLoadout::admit_resume`], reachable only from [`session::CommittedLoadout::rehydrate`] |
//! | A persisted loadout is re-derived, never trusted | [`session::CommittedLoadout::rehydrate`] returns [`session::LoadoutIntegrityError`] |
//! | Resume re-verifies the managed config's bytes | [`session::ManagedConfigVerified`], whose only source is a freshly computed digest and length |
//! | A session cannot be its own parent | [`session::SessionEdge::new`] |
//!
//! Their durable halves — the loadout row and its `created_event_seq`, the
//! `session_edges` graph, and the private configuration artifact whose bytes the
//! witness re-reads — belong to `governor-store-sqlite` and `governor-artifacts`.
//!
//! [`docs/state-machines.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/state-machines.md
//! [`docs/research/2026-08-31-durable-orchestration-pattern-review.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/research/2026-08-31-durable-orchestration-pattern-review.md
//! [`docs/adr/0007-session-lineage-memory-and-analytics.md`]: https://github.com/DivMode/commandgovernor/blob/main/docs/adr/0007-session-lineage-memory-and-analytics.md

pub mod artifact;
pub mod binding;
pub mod claim;
pub mod delivery;
// Crate-internal: the one length-prefixed absorption rule every digest here
// shares. Not part of the public surface — callers consume digests, never the
// encoding that produced them.
mod digest;
pub mod effect;
pub mod error;
pub mod fence;
pub mod foreman_turn;
pub mod health;
pub mod id;
pub mod input;
pub mod lease;
pub mod mutation;
pub mod obligation;
pub mod outbound;
pub mod random;
pub mod session;
pub mod time;
pub mod watchdog;
pub mod worker_command;
pub mod worker_evidence;

pub use error::{Conflict, ConflictKind, Outcome, Transition};
