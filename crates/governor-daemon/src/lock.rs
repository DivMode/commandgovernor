//! The single-daemon instance lock over a state root.
//!
//! # Why not SQLite's writer serialization
//!
//! `docs/testing.md` DB-005 says it outright: *SQLite writer serialization
//! alone is not accepted as daemon election*, and `docs/threat-model.md`
//! repeats it — *SQLite's one-writer property is not the daemon-election
//! protocol*. The reason is that serialization is not exclusion. Two daemons
//! against one state root would both open the database legitimately, both
//! advance the daemon epoch, both run startup quarantine, both replay
//! projections, and both go on to schedule external work; SQLite would merely
//! order their transactions. Each of those writes is individually valid. What
//! is invalid is that there are two authorities.
//!
//! So election happens **before** the database is opened, at the state root,
//! and it alone decides who may proceed (`docs/architecture.md` startup order
//! step 1; `docs/threat-model.md` "owner-root instance lock before
//! DB/browser/runtime recovery").
//!
//! # The protocol
//!
//! One file, `<root>/daemon.lock`, owner-only, holding a one-line record of its
//! last holder. Two mechanisms doing two different jobs:
//!
//! 1. an **advisory lock held by the kernel** on the open file
//!    ([`std::fs::File::try_lock`]) is the authority. It is attached to the
//!    open file description, so the kernel releases it when the holder exits —
//!    including when the holder is killed, panics, or the machine loses power.
//!    There is no timeout in this module and no lease to expire;
//! 2. the **incarnation record** in the file is corroboration and diagnosis. It
//!    names the process number and start identity that last held the lock and
//!    whether that holder released it deliberately, so a reclaim can report
//!    *how* the previous holder differs and a refused second daemon can name
//!    the holder it lost to.
//!
//! # Why the file is never unlinked
//!
//! Unlinking is what makes lock files race. If acquisition could delete the
//! file, two processes could each end up holding a kernel lock on a *different*
//! inode — one of them already unlinked — and both would believe they were the
//! authority. Keeping exactly one inode at the path for the life of the state
//! root removes that class entirely: "the lock" is unambiguously the kernel
//! lock on the file at that path, and a leftover lock file after a clean
//! shutdown is the normal, expected state.
//!
//! # Reclaim, and the two refusals
//!
//! Obtaining the kernel lock *is* proof that no process holds it, which is the
//! only proof this module accepts; nothing here reclaims on age. On top of that
//! the previous record is checked:
//!
//! - the lock is **held** — a live daemon owns this state root and this process
//!   fails closed with [`DaemonError::AuthorityHeld`]. It does not wait and it
//!   does not degrade into a partial authority;
//! - the lock is **free**, the record says `held`, and the recorded holder's
//!   incarnation still re-derives — a holder mid-shutdown and a holder that
//!   somehow lost its lock look identical from here, so it fails closed with
//!   [`DaemonError::LockHolderStillAlive`] rather than guessing;
//! - the lock is **free** and the record says `released`, names a process
//!   number that resolves to nothing, or names one whose start identity no
//!   longer matches — the holder is provably gone and the lock is reclaimed,
//!   reporting how it differed;
//! - the file holds bytes that are **not a lock record** — refused. This binary
//!   will not overwrite state it cannot identify.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use governor_core::fence::SafeToken;
use governor_core::lease::{ProcessIncarnation, ProcessSlot, ProcessStartRef};

use crate::error::{DaemonError, LockDefect, ReclaimedLock};
use crate::incarnation::{self, START_UNAVAILABLE};
use crate::layout::PRIVATE_FILE_MODE;

/// Marker on the first field of a lock record.
const RECORD_MAGIC: &str = "cglock1";
/// Placeholder written when the platform derives no start identity.
const NO_START: &str = "-";
/// Longest lock record this reads. More than that is not a lock record.
const MAX_RECORD_BYTES: u64 = 512;

/// Whether the last holder gave the lock up on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HolderState {
    /// The holder was running when it wrote the record.
    Held,
    /// The holder released the lock as part of a clean shutdown.
    Released,
}

impl HolderState {
    const fn code(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Released => "released",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "held" => Some(Self::Held),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

/// The state root's single-daemon authority, held for the process's lifetime.
///
/// Dropping it marks the record released and drops the kernel lock, so an
/// ordinary return from `main` is a clean release and a crash is a kernel
/// release with the record still saying `held`.
#[derive(Debug)]
pub struct InstanceLock {
    path: PathBuf,
    file: File,
    holder: ProcessIncarnation,
    reclaimed: Option<ReclaimedLock>,
}

impl InstanceLock {
    /// Elects this process as the state root's authority, or refuses.
    ///
    /// # Errors
    ///
    /// - [`DaemonError::AuthorityHeld`] when a live daemon owns the root;
    /// - [`DaemonError::LockHolderStillAlive`] when the recorded holder is
    ///   running but does not hold the lock;
    /// - [`DaemonError::Lock`] for an unrecognised record or an I/O failure.
    pub fn acquire(path: &Path, holder: ProcessIncarnation) -> Result<Self, DaemonError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(PRIVATE_FILE_MODE)
            .open(path)
            .map_err(|_| DaemonError::Lock {
                reason: LockDefect::Unopenable,
            })?;

        // Read before locking: the bytes are the previous holder's, and a
        // failed lock still needs them to name who won.
        let previous = read_record(&mut file).map_err(|reason| DaemonError::Lock { reason })?;

        if file.try_lock().is_err() {
            return Err(DaemonError::AuthorityHeld {
                slot: previous.as_ref().map_or(ProcessSlot::new(0), |r| r.slot),
            });
        }

        let reclaimed = Self::reclaim_decision(previous, &holder)?;
        write_record(&mut file, &holder, HolderState::Held).map_err(|_| DaemonError::Lock {
            reason: LockDefect::Unwritable,
        })?;

        Ok(Self {
            path: path.to_path_buf(),
            file,
            holder,
            reclaimed,
        })
    }

    /// Decides whether the previous record permits taking over.
    ///
    /// The kernel has already said nobody holds the lock. This is the second,
    /// independent check: the recorded holder must be provably gone.
    fn reclaim_decision(
        previous: Option<LockRecord>,
        holder: &ProcessIncarnation,
    ) -> Result<Option<ReclaimedLock>, DaemonError> {
        let Some(record) = previous else {
            // An empty file: the lock this call just created. Nothing to
            // reclaim and nothing to prove.
            return Ok(None);
        };
        if record.state == HolderState::Released {
            return Ok(Some(ReclaimedLock {
                slot: record.slot,
                mismatch: None,
            }));
        }

        let mismatch = match (record.incarnation(), incarnation::for_slot(record.slot)) {
            // The recorded holder still re-derives, and it is not us. Alive but
            // not holding the lock is a contradiction this process cannot
            // resolve, so nothing is taken.
            (Some(recorded), Some(live)) if recorded == live && live != *holder => {
                return Err(DaemonError::LockHolderStillAlive { slot: record.slot });
            }
            (Some(recorded), Some(live)) => recorded.classify(&live),
            // Either the process number resolves to nothing, or this platform
            // derives no start identity. In both cases the kernel lock is the
            // only proof available, and it has already been given.
            _ => None,
        };
        Ok(Some(ReclaimedLock {
            slot: record.slot,
            mismatch,
        }))
    }

    /// The incarnation recorded in the lock.
    #[must_use]
    pub const fn holder(&self) -> &ProcessIncarnation {
        &self.holder
    }

    /// Where the lock file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The previous holder, when this acquisition reclaimed a stale lock.
    #[must_use]
    pub const fn reclaimed(&self) -> Option<&ReclaimedLock> {
        self.reclaimed.as_ref()
    }

    /// Releases the lock.
    ///
    /// Consuming, so the authority cannot be used after it is given up.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Record first, unlock second: a starting daemon that saw `released`
        // before this process had actually let go would reclaim under a live
        // holder. Doing it the other way round only ever loses the marker,
        // which degrades to the conservative refusal.
        let _ = write_record(&mut self.file, &self.holder, HolderState::Released);
        let _ = self.file.unlock();
    }
}

/// What a lock file says about its last holder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LockRecord {
    slot: ProcessSlot,
    start: Option<ProcessStartRef>,
    state: HolderState,
}

impl LockRecord {
    fn incarnation(&self) -> Option<ProcessIncarnation> {
        self.start
            .clone()
            .map(|start| ProcessIncarnation::new(self.slot, start))
    }
}

/// Reads the lock file's record, if it holds one.
///
/// `Ok(None)` means the file is empty — the normal state of a lock file this
/// call just created. Unparseable bytes are an error, not an empty record.
fn read_record(file: &mut File) -> Result<Option<LockRecord>, LockDefect> {
    use std::io::{Seek as _, SeekFrom};

    file.seek(SeekFrom::Start(0))
        .map_err(|_| LockDefect::Unopenable)?;
    let mut text = String::new();
    file.take(MAX_RECORD_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| LockDefect::Unreadable)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_record(&text).map(Some).ok_or(LockDefect::Unreadable)
}

fn write_record(
    file: &mut File,
    holder: &ProcessIncarnation,
    state: HolderState,
) -> std::io::Result<()> {
    use std::io::{Seek as _, SeekFrom};

    let start = holder.start().as_token().as_str();
    let start = if start == START_UNAVAILABLE {
        NO_START
    } else {
        start
    };
    let line = format!(
        "{RECORD_MAGIC} slot={} start={start} state={}\n",
        holder.slot(),
        state.code()
    );
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(line.as_bytes())?;
    file.flush()?;
    // The record has to survive a power loss, or a reboot would leave a lock
    // file naming nobody and this module would have nothing to diagnose with.
    file.sync_all()
}

fn parse_record(text: &str) -> Option<LockRecord> {
    let line = text.lines().next()?;
    let mut fields = line.split_whitespace();
    if fields.next()? != RECORD_MAGIC {
        return None;
    }
    let mut slot = None;
    let mut start = None;
    let mut state = None;
    for field in fields {
        let (key, value) = field.split_once('=')?;
        match key {
            "slot" => slot = value.parse::<u32>().ok().map(ProcessSlot::new),
            "start" if value == NO_START => start = None,
            "start" => start = Some(ProcessStartRef::new(SafeToken::new(value).ok()?)),
            "state" => state = HolderState::parse(value),
            _ => return None,
        }
    }
    Some(LockRecord {
        slot: slot?,
        start,
        state: state?,
    })
}

/// What a lock file says, for a reader that holds no authority.
///
/// `doctor` uses this against a state root another process may own, so it
/// takes no lock and changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LockStatus {
    /// There is no lock file: no daemon has ever run against this state root.
    Absent,
    /// A live process holds the lock.
    Held {
        /// The holder's process number, as recorded.
        slot: ProcessSlot,
    },
    /// A lock file exists and nothing holds it.
    Free {
        /// The last holder's process number, when the record names one.
        slot: Option<ProcessSlot>,
        /// Whether the last holder released it deliberately.
        released: bool,
    },
    /// The file exists but does not hold a record this binary understands.
    Unreadable,
}

impl LockStatus {
    /// Stable `snake_case` code for diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Held { .. } => "held",
            Self::Free { released: true, .. } => "free_after_clean_release",
            Self::Free { .. } => "free_after_unclean_exit",
            Self::Unreadable => "unreadable",
        }
    }

    /// Whether a daemon currently holds authority over the state root.
    #[must_use]
    pub const fn daemon_running(self) -> bool {
        matches!(self, Self::Held { .. })
    }
}

/// Reads a lock file's status without taking authority over it.
///
/// The liveness probe is a non-blocking lock attempt on a *read-only* handle,
/// released immediately. That is the only way to ask the kernel the question,
/// and it cannot become an acquisition: nothing is written, and the handle is
/// dropped before this returns.
#[must_use]
pub fn inspect(path: &Path) -> LockStatus {
    let Ok(mut file) = File::open(path) else {
        return LockStatus::Absent;
    };
    let record = match read_record(&mut file) {
        Ok(record) => record,
        Err(_) => return LockStatus::Unreadable,
    };

    // A shared lock is enough to answer "is somebody holding this
    // exclusively?", and it cannot displace an exclusive holder.
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            LockStatus::Free {
                slot: record.as_ref().map(|r| r.slot),
                released: record.is_none_or(|r| r.state == HolderState::Released),
            }
        }
        Err(_) => LockStatus::Held {
            slot: record.map_or(ProcessSlot::new(0), |r| r.slot),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incarnation_of(slot: u32, start: &str) -> ProcessIncarnation {
        ProcessIncarnation::new(
            ProcessSlot::new(slot),
            ProcessStartRef::new(SafeToken::new(start).expect("fixture token")),
        )
    }

    fn stage_record(path: &Path, holder: &ProcessIncarnation, state: HolderState) {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .mode(PRIVATE_FILE_MODE)
            .open(path)
            .expect("staged lock file");
        write_record(&mut file, holder, state).expect("staged record");
    }

    #[test]
    fn a_record_round_trips_through_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        let holder = incarnation::current();
        let lock = InstanceLock::acquire(&path, holder.clone()).expect("first acquisition");
        assert_eq!(lock.holder(), &holder);
        assert!(lock.reclaimed().is_none());

        let mut file = File::open(&path).expect("open");
        let record = read_record(&mut file).expect("readable").expect("a record");
        assert_eq!(record.slot, holder.slot());
        assert_eq!(record.state, HolderState::Held);
    }

    #[test]
    fn the_lock_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        let _lock = InstanceLock::acquire(&path, incarnation::current()).expect("acquired");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, PRIVATE_FILE_MODE, "mode was {mode:o}");
    }

    #[test]
    fn a_second_acquisition_in_this_process_fails_closed() {
        // The in-process half of DB-005; the acceptance suite proves the
        // cross-process half with two real binaries.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        let first = InstanceLock::acquire(&path, incarnation::current()).expect("first");
        match InstanceLock::acquire(&path, incarnation::current()) {
            Err(DaemonError::AuthorityHeld { slot }) => {
                assert_eq!(slot, first.holder().slot());
            }
            other => panic!("a second authority was handed out: {other:?}"),
        }
    }

    #[test]
    fn inspect_sees_a_held_lock_without_taking_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        assert_eq!(inspect(&path), LockStatus::Absent);

        let lock = InstanceLock::acquire(&path, incarnation::current()).expect("acquired");
        assert!(inspect(&path).daemon_running());
        assert!(inspect(&path).daemon_running(), "probing must not consume");

        lock.release();
        match inspect(&path) {
            LockStatus::Free { released, .. } => assert!(released),
            other => panic!("expected a released lock, got {other:?}"),
        }
    }

    #[test]
    fn a_clean_release_lets_the_next_start_in_without_a_liveness_probe() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        let holder = incarnation::current();
        InstanceLock::acquire(&path, holder.clone())
            .expect("acquired")
            .release();

        // The recorded holder is this very process, which is still alive. Only
        // the `released` marker makes this legal, which is exactly what stops a
        // stop/start cycle refusing itself.
        let again = InstanceLock::acquire(&path, holder).expect("reacquired");
        assert!(again.reclaimed().is_some());
    }

    #[test]
    fn a_dead_holders_lock_is_reclaimed_and_reported() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        // A process number that resolves to nothing: the holder is provably
        // gone, which is the only unclean reclaim this module performs.
        let dead = incarnation_of(u32::from(u16::MAX) * 4 + 3, "linux.deadbeefdeadbeef");
        stage_record(&path, &dead, HolderState::Held);

        let lock = InstanceLock::acquire(&path, incarnation::current()).expect("reclaimed");
        let reclaimed = lock.reclaimed().expect("the reclaim must be reported");
        assert_eq!(reclaimed.slot, dead.slot());
    }

    #[test]
    fn a_live_holder_that_did_not_release_is_not_displaced() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        // This test process is unquestionably alive and its incarnation
        // re-derives. Nothing holds the kernel lock, so this is the ambiguous
        // case, and ambiguity must not become a takeover.
        let alive = incarnation::current();
        stage_record(&path, &alive, HolderState::Held);

        let other = incarnation_of(alive.slot().get().wrapping_add(1), "linux.0000000000000000");
        match InstanceLock::acquire(&path, other) {
            Err(DaemonError::LockHolderStillAlive { slot }) => assert_eq!(slot, alive.slot()),
            other => panic!("a live holder was displaced: {other:?}"),
        }
    }

    #[test]
    fn a_recycled_process_number_is_reclaimed_with_the_mismatch_named() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        // Same process number as this live process, different start identity:
        // the number was recycled, so the recorded holder is gone.
        let alive = incarnation::current();
        let recycled = incarnation_of(alive.slot().get(), "linux.ffffffffffffffff");
        stage_record(&path, &recycled, HolderState::Held);

        let lock = InstanceLock::acquire(&path, alive).expect("reclaimed");
        let reclaimed = lock.reclaimed().expect("reported");
        assert_eq!(reclaimed.slot, recycled.slot());
        // On a platform that derives a start identity the mismatch is named;
        // where none is derivable there is nothing to compare, and the kernel
        // lock stands alone.
        if incarnation::start_ref(recycled.slot()).is_some() {
            assert_eq!(
                reclaimed.mismatch,
                Some(governor_core::lease::IncarnationMismatch::SlotReused)
            );
        }
    }

    #[test]
    fn an_unreadable_lock_file_is_refused_rather_than_overwritten() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.lock");
        std::fs::write(&path, b"this is not a lock record\n").expect("staged file");

        match InstanceLock::acquire(&path, incarnation::current()) {
            Err(DaemonError::Lock {
                reason: LockDefect::Unreadable,
            }) => {}
            other => panic!("an unknown lock file was not refused: {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).expect("still there"),
            b"this is not a lock record\n",
            "an unrecognised file must not be rewritten"
        );
        assert_eq!(inspect(&path), LockStatus::Unreadable);
    }

    #[test]
    fn a_record_without_a_start_identity_still_parses() {
        let record = parse_record("cglock1 slot=42 start=- state=held\n").expect("parsed");
        assert_eq!(record.slot, ProcessSlot::new(42));
        assert_eq!(record.start, None);
        assert_eq!(record.incarnation(), None);
    }

    #[test]
    fn a_record_with_an_unknown_field_or_marker_is_refused() {
        assert_eq!(parse_record("cglock1 slot=42 wat=1 state=held\n"), None);
        assert_eq!(parse_record("otherlock slot=42 state=held\n"), None);
        assert_eq!(parse_record("cglock1 slot=42\n"), None);
        assert_eq!(parse_record(""), None);
    }
}
