# ADR 0002: Rust daemon/CLI with one `rusqlite` SQLite authority

- **Status:** Proposed
- **Date:** 2026-08-31

## Context

V1 needs durable event append, fenced obligation transitions, crash recovery,
browser-delivery transactions, worker lifecycle ingestion, CLI control, and
cross-platform packaging. It does not need a conventional GUI or a horizontally
scaled database.

The central design is one local authoritative daemon. SQLite permits concurrent
readers in WAL mode but still serializes writes. Therefore the relevant question
is not which library can create the largest async pool; it is which boundary makes
the single-writer transaction protocol easiest to reason about and crash-test.

`rusqlite` and SQLx SQLite were compared. Both are capable. SQLx provides an async
API, pooling, compile-time query support in configured workflows, and multi-DB
abstraction. Those are useful in many services but are not the dominant V1 need.

## Decision

V1 uses:

- stable Rust, edition 2024;
- one `command-governor daemon` as orchestration authority;
- `command-governor` CLI as a local IPC client;
- `rusqlite` with bundled SQLite;
- one daemon-owned dedicated DB actor/thread;
- typed async request/reply messages between Tokio tasks and the DB actor;
- WAL, foreign keys, bounded busy timeout, and `synchronous=FULL` initially;
- explicit migrations, no ORM;
- Unix domain socket on macOS/Linux and named pipe on Windows for normal CLI IPC,
  subject to implementation proof.

No GUI framework is introduced for V1. Future UI is a projection/client and may
not own lifecycle state.

## Why `rusqlite`

### Correctness fit

The core transition pattern is:

```text
validate current fenced projection
append immutable event
update projection/create obligation
commit
return evidence
```

Serializing this through one explicit DB actor makes transaction ownership and
fault injection obvious. It also prevents an adapter from accidentally holding a
SQLite transaction across browser/process/network I/O.

### Packaging fit

Bundled SQLite gives the application a known SQLite feature/version baseline
rather than inheriting arbitrary host-library differences.

### What we do not gain from a pool

WAL improves reader concurrency, but SQLite still has one writer. A large async
pool cannot make two write transactions commit concurrently and can make lock
ordering/busy behavior less explicit. V1 has no requirement to abstract over
Postgres/MySQL.

## Tokio boundary

Synchronous `rusqlite` operations do not run on arbitrary Tokio worker threads.
The store crate owns a dedicated blocking DB actor with typed commands. Domain
operations expose async-friendly request APIs at the daemon boundary while keeping
actual transaction code sequential and testable.

If later profiling proves read concurrency needs separate read-only connections,
those may be added without permitting multiple independent state writers.

## Rust crate direction

Initial workspace boundary after architecture acceptance:

```text
governor-core
governor-store-sqlite
governor-runtime
governor-runtime-herdr
governor-worker-claude
governor-worker-codex
governor-browser
governor-chatgpt-web
governor-mcp
governor-github
governor-daemon
governor-testkit
command-governor
```

The exact number may shrink during implementation. `governor-core`, the store,
and the isolated ChatGPT-specific crate remain architectural boundaries.

## Error policy

Domain/storage crates expose typed errors with `thiserror`. `anyhow` may be used at
binary/composition boundaries to add operational context, but a domain conflict
such as stale generation or ambiguous delivery must remain machine-classifiable.

## Quality gates

From the first Rust commit:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
cargo deny check
```

Pin `rust-toolchain.toml`, commit `Cargo.lock`, run macOS/Linux/Windows CI where
supported, and review dependency/license updates.

## Alternatives

### SQLx SQLite

Rejected for V1, not rejected forever. If Command Governor later becomes a service
with materially different concurrency/storage needs, SQLx can be reconsidered.
No current requirement justifies its additional async/pool/macro surface for the
one-writer authority.

### Embedded key/value database

Rejected because relational uniqueness/foreign-key constraints and transactional
multi-row state transitions are directly useful to the lifecycle model.

### Postgres

Rejected for local-first V1 because it adds an external service and operational
state without solving a V1 requirement.

### GUI-first app framework

Rejected. Dioxus/Iced/Tauri/Wry/Electron do not contribute to correctness. A
future status UI must consume daemon state through the same API/IPC as the CLI.

## Consequences

Positive: small Rust foundation, explicit writes, deterministic packaging, easier
crash testing, no UI coupling.

Cost: the store actor is an application abstraction we own, and long-running DB
operations must be designed carefully so the single writer is not blocked by
unbounded work.
