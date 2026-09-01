//! Composition library wiring the Command Governor kernel to its store.
//!
//! # Phase 1 role
//!
//! `governor-daemon` is where the pure kernel in `governor-core` meets the
//! durable authority in `governor-store-sqlite` and the private artifact root
//! in `governor-artifacts`. It owns four things none of them can:
//!
//! - the **production ports** — a real clock, the OS entropy source, and UUIDv7
//!   identity minting. Neither the core nor the store ships one, deliberately,
//!   which is what keeps every replay and crash suite reproducible
//!   ([`ports`]);
//! - **single-daemon election** over a state root, taken before the database is
//!   opened and independent of SQLite's writer serialization ([`lock`]);
//! - the **startup order** `docs/architecture.md` fixes, where a failed step is
//!   a typed refusal to serve rather than a warning ([`startup`]);
//! - the **owner-local control socket** and the safe-diagnostics surfaces the
//!   command line reads ([`ipc`], [`report`], [`doctor`], [`logging`]).
//!
//! # Boundary
//!
//! This is a library, not a binary; `command-governor` is the only entry point,
//! and it holds no domain logic. Keeping composition here means the same wiring
//! can be driven from a test harness.
//!
//! Phase 1 wires no live integrations. ChatGPT browser transport, the MCP
//! foreman connector, and the Claude and Herdr worker adapters are gated behind
//! their own live conformance gates and are deliberately absent here. Nothing
//! in this crate schedules external work at all: it opens the state root,
//! proves what it can, and answers questions about it.
//!
//! # The trust model this crate does not overstate
//!
//! Every directory and file the daemon creates is owner-only, and the layout is
//! checked for ownership, mode and symbolic links on every start. That protects
//! the state root **from other OS principals**. It is not a hostile same-user
//! sandbox, no code or comment here may be read as claiming one, and `doctor`
//! reports the distinction as data (`docs/testing.md` SEC-007).

// The state root's mode bits, `symlink_metadata`, the Unix control socket and
// the kernel-held instance lock are the substance of this crate, not incidental
// detail. A Windows port needs named pipes and ACLs, which is a separate
// implementation rather than a stub.
#[cfg(not(unix))]
compile_error!(
    "governor-daemon implements the Unix owner-only state root and control \
     socket; the Windows named-pipe and ACL policy is a separate platform \
     implementation"
);

pub mod doctor;
pub mod error;
pub mod incarnation;
pub mod ipc;
pub mod layout;
pub mod lock;
pub mod logging;
pub mod ports;
pub mod report;
pub mod startup;
pub mod worker;

pub use doctor::{Check, Diagnosis, diagnose};
pub use error::{DaemonError, LockDefect, PathDefect, ReclaimedLock};
pub use ipc::{IpcError, Request};
pub use layout::{PathClass, StateRoot};
pub use lock::{InstanceLock, LockStatus};
pub use logging::{Fields, Level, SafeLog};
pub use ports::{OsRandom, SystemClock, Uuidv7Ids, Uuidv7Keys, production_ports};
pub use startup::{Daemon, DaemonConfig, ReadyReport};
pub use worker::{
    ResumeRefusal, ResumeWorkerRequest, WorkerSpawnAuthorization, authorize_worker_resume,
};
