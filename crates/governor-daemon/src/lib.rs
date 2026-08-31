//! Composition library wiring the Command Governor kernel to its store.
//!
//! # Phase 1 role
//!
//! `governor-daemon` is where the pure kernel in `governor-core` meets the
//! durable authority in `governor-store-sqlite`: it owns process startup and
//! shutdown, the single writer actor, projection replay and the fail-closed
//! startup check, recovery of claims and delivery attempts orphaned by a
//! previous process, and the watchdog that raises attention without ever
//! fabricating a completion or a failure.
//!
//! # Boundary
//!
//! This is a library, not a binary; `command-governor` is the only entry point.
//! Keeping composition here means the same wiring can be driven from a test
//! harness with `governor-testkit` fakes in place of live adapters.
//!
//! Phase 1 wires no live integrations. ChatGPT browser transport, the MCP
//! foreman connector, and the Claude and Herdr worker adapters are gated behind
//! their own live conformance gates and are deliberately absent here.
