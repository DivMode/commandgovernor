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
//! Errors raised here are typed (`thiserror`) and machine-classifiable: a
//! domain conflict such as a stale generation or an ambiguous delivery must be
//! distinguishable by a caller, never flattened into an opaque string.
