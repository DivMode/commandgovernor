//! Deterministic fakes and failpoints for Command Governor tests.
//!
//! # Phase 1 role
//!
//! `governor-testkit` supplies the controlled environment the Phase 1 test
//! matrix in `docs/testing.md` is written against: a controllable clock, a
//! seeded identifier source, a fake browser delivery boundary, fake foreman/MCP
//! state, and injectable failpoints for crash, rename, and fsync ordering.
//!
//! # Boundary
//!
//! Fakes stand in for live adapters so that obligations, fences, artifact
//! durability, and delivery identity can be proven without ChatGPT, Claude, or
//! Herdr, and without any credential. Nothing here may reach the network or a
//! real browser: a passing test in this crate is evidence about the kernel, and
//! must never be mistaken for evidence about a live service.
//!
//! Determinism is the contract. A test that depends on wall-clock time, ambient
//! randomness, or host state does not belong here.
