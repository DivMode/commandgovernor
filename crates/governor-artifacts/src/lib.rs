//! The private immutable result-artifact store.
//!
//! # Phase 1 role
//!
//! A terminal obligation is useless if the actual result still lives only in a
//! PTY (`docs/architecture.md`, "Result durability"). This crate owns the file
//! half of making it durable: an owner-private root, opaque daemon-allocated
//! keys, crash-safe publication ordering, integrity-checked reads, and the two
//! sweeps that keep the root honest.
//!
//! It holds the **bounded final worker result required for review** and nothing
//! else. There is no raw provider-stream spool here and no shape that could
//! become one: [`PublishRequest::bytes`] is a slice of already-bounded bytes,
//! not a reader (`docs/data-model.md`, "Managed-run filesystem staging";
//! `docs/testing.md` ART-009).
//!
//! # Why this is its own crate
//!
//! Three boundaries meet here and none of the existing crates can hold all
//! three.
//!
//! - `governor-core` must stay pure: it performs no I/O at all, which is what
//!   makes every invariant it encodes provable without a filesystem.
//! - `governor-store-sqlite` must stay ambient-free: a test scans its `src/`
//!   for `std::fs` and fails the build if it appears, because "no external I/O
//!   while a transaction is held" is enforced structurally there rather than by
//!   review.
//! - `governor-daemon` is the composition crate, and will grow an async
//!   runtime, a CLI and adapters. A filesystem security boundary that other
//!   crates — the acceptance testkit above all — must depend on directly does
//!   not belong downstream of all that.
//!
//! So this is a leaf: `governor-core` for the domain rules, and
//! `governor-store-sqlite` for one plain data type,
//! [`DurableArtifact`](governor_store_sqlite::DurableArtifact), which is the
//! seam described below. It calls no store operation.
//!
//! # File before database
//!
//! ```text
//!  ArtifactStore::publish(bytes)          governor-store-sqlite
//!    create staging (O_EXCL, 0600)
//!    write
//!    fsync file
//!    link -> objects/<key>, unlink staging
//!    fsync objects/
//!         |
//!         v
//!    PublishedArtifact  --- .durable() --->  DurableArtifact
//!                                                 |
//!                                                 v
//!                                       publish_worker_result(..)
//!                                       terminal event + artifact row
//!                                       + completed_unprocessed, one txn
//! ```
//!
//! [`PublishedArtifact`] has private fields, no public constructor, and exactly
//! one producer: the line after the directory `fsync`. The database
//! transaction that opens a `completed_unprocessed` obligation needs a
//! `DurableArtifact`, and the artifact layer can only hand one over on that far
//! side. The forbidden outcome — a committed open obligation referencing an
//! artifact that was never made durable — is not reachable through this API.
//!
//! A crash anywhere before the commit leaves at worst an unreferenced file.
//! That is the safe direction, and [`ArtifactStore::scan_orphans`] sets such
//! files aside after a grace period rather than deleting them, because a
//! publication that is merely slow looks identical to a crashed one.
//!
//! # What owner-only modes do and do not claim
//!
//! Every directory is `0700` and every file is `0600`, forced with an explicit
//! `chmod` after creation so the host umask cannot weaken them
//! (`docs/testing.md` ART-005). Opens are `O_NOFOLLOW`, keys are validated
//! single components, and creation is `O_EXCL`.
//!
//! That protects the store **from other OS principals**. It is *not* a hostile
//! same-user sandbox, and no code or comment in this crate may be read as
//! claiming one (`SECURITY.md` "Local trust model", `docs/testing.md`
//! SEC-007). Claude and its tools normally run as the same OS user as the
//! daemon; a deliberately malicious process with that user's authority is not
//! contained by a file mode, and containing it needs a separate-user or broker
//! design this project has not built. What the checks here buy against a
//! same-user actor is *detection* — a digest mismatch, an `ELOOP`, an
//! unexpected link count — not prevention.

// Modes, `O_NOFOLLOW`, `link(2)` and directory `fsync` are the substance of
// this crate, not incidental detail. `docs/testing.md` ART-005 puts the Windows
// ACL equivalent in a separate platform suite, so refusing to build is honest
// where a stub would not be.
#[cfg(not(unix))]
compile_error!(
    "governor-artifacts implements the Unix owner-only artifact root; \
                the Windows ACL policy is a separate platform implementation"
);

mod error;
mod failpoint;
mod fs_secure;
mod gc;
mod key;
mod root;
mod store;

pub use error::{ArtifactError, ArtifactResult, FsOperation, UnsafePathReason};
pub use failpoint::{ArtifactFailpoint, ArtifactFailpointHook};
pub use gc::{
    CollectionReport, OrphanReason, OrphanScan, Quarantined, RetentionDecision, RetentionInput,
    decide,
};
pub use key::{InvalidStorageKey, MAX_STORAGE_KEY_LEN, StorageKey, StorageKeySource};
pub use root::ArtifactRoot;
pub use store::{
    ArtifactConfig, ArtifactStore, DEFAULT_MAX_ARTIFACT_BYTES, DEFAULT_ORPHAN_GRACE,
    DEFAULT_RETENTION_GRACE, OpenArtifactStore, PublishRequest, PublishedArtifact,
};
