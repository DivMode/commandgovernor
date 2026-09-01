//! Deterministic fakes and failpoints for Command Governor tests.
//!
//! # Phase 1 role
//!
//! `governor-testkit` supplies the controlled environment the Phase 1 test
//! matrix in `docs/testing.md` is written against: a controllable clock, a
//! seeded identifier source, a fake browser delivery boundary, fake foreman/MCP
//! state, and injectable failpoints for crash, rename, and fsync ordering.
//!
//! The acceptance suites themselves live in this crate's `tests/`, one file per
//! documented family, and every test is named after the ID it proves:
//!
//! | Suite | Family |
//! | --- | --- |
//! | `obl_acceptance.rs` | OBL-001 … OBL-010 |
//! | `art_acceptance.rs` | ART-001 … ART-005 |
//! | `del_acceptance.rs` | DEL-001 … DEL-018 |
//! | `gpt_acceptance.rs` | GPT-001 … GPT-009 |
//! | `db_acceptance.rs` | DB-001 … DB-008 |
//! | `sec_acceptance.rs` | SEC-001 … SEC-010 |
//! | `research_acceptance.rs` | durable-orchestration review, tests 1 … 12 |
//! | `determinism.rs` | one seed replays exactly; two seeds diverge |
//!
//! Each file opens with a coverage table mapping its tests to the documented
//! IDs, including the ones another crate's suite already proves and the ones
//! deferred to a later gate, so coverage can be audited without grepping.
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
//!
//! # How the fakes enforce rather than assert
//!
//! Three of them look at the committed database through their own read-only
//! connection and **panic** rather than acting when the store does not already
//! show the required state:
//!
//! | Fake | Refuses to act until |
//! | --- | --- |
//! | [`browser::FakeBrowser`] | the attempt is `claimed`; Send needs `activation_armed` |
//! | [`effect::FakeExternalDestination`] | the intent row is committed and the dispatch fence is set |
//! | [`foreman::bootstrap`] | — it cannot disclose an identity, because no query selects one |
//!
//! That is what makes DEL-003, DEL-005 and research test 1 properties of the
//! boundary rather than assertions a test remembered to write.
//!
//! # What this crate cannot prove, and does not claim to
//!
//! Phase 1 has no store write path to `needs_input`, no worker-command
//! projection, no health-condition raise other than startup reconciliation, and
//! no reconciliation operation that promotes an ambiguous wake. Where a
//! documented test needs one of those, the suites implement the pure or fake
//! half and say so in their coverage table. None of them pretends the durable
//! half passed.

pub mod browser;
pub mod clock;
pub mod dump;
pub mod effect;
pub mod failpoints;
pub mod foreman;
pub mod harness;
pub mod keys;
pub mod restart;
pub mod rng;
pub mod scenario;
pub mod sentinels;

pub use clock::{DEFAULT_CLOCK_START_MS, FakeClock};
pub use harness::{Harness, OpenedStore};
pub use rng::{SeededIds, SeededPorts, SeededRandom, SplitMix64};
