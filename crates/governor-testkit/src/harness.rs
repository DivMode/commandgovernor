//! A throwaway state root that can be opened, killed and reopened.
//!
//! Real SQLite files in a real directory, because `:memory:` cannot express
//! WAL, `synchronous=FULL`, or reopening the same bytes, and all three are
//! under test. Every port is deterministic, so a scenario replays identically
//! from its seed.
//!
//! # The state root's shape
//!
//! ```text
//! <root>/governor.sqlite3[-wal][-shm]   the durable authority
//! <root>/artifacts/objects/             published immutable results
//! <root>/artifacts/incoming/            publication staging
//! <root>/artifacts/quarantine/          orphans the sweep set aside
//! <root>/logs/                          reserved for the daemon's diagnostics
//! ```
//!
//! `logs/` is created empty and holds nothing in Phase 1. It exists so
//! [`Harness::all_files`] — and through it the SEC-001 sentinel sweep — already
//! covers the surface the daemon will write to, instead of the sweep having to
//! be widened later by someone who remembers to.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use governor_artifacts::{
    ArtifactConfig, ArtifactFailpointHook, ArtifactStore, OpenArtifactStore, StorageKeySource,
};
use governor_store_sqlite::{
    FailpointHook, OpenStore, Store, StoreConfig, StorePorts, StoreResult,
};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use uuid::Uuid;

use crate::clock::{DEFAULT_CLOCK_START_MS, FakeClock};
use crate::keys::SeededKeys;
use crate::rng::{SeededIds, SeededRandom};

/// How far apart two simulated processes' seeds are placed.
///
/// A real CSPRNG never repeats itself across restarts, and a harness that did
/// would hide exactly the bug these suites look for: two "different"
/// correlation IDs or lease tokens that are in fact equal.
const SEED_STRIDE: u64 = 1_000_003;

/// One simulated Command Governor state root.
#[derive(Debug)]
pub struct Harness {
    dir: TempDir,
    seed: u64,
    opens: AtomicU64,
}

/// Everything one open produced, for a test that needs to move time.
#[derive(Debug)]
pub struct OpenedStore {
    /// The running store.
    pub store: Store,
    /// The clock that store reads, shared with the caller.
    pub clock: FakeClock,
}

impl Harness {
    /// Creates a state root with the default seed.
    #[must_use]
    pub fn new() -> Self {
        Self::with_seed(1)
    }

    /// Creates a state root whose every port derives from `seed`.
    ///
    /// Two harnesses with the same seed produce byte-identical scenarios; two
    /// with different seeds must not, and `determinism.rs` proves both halves.
    ///
    /// # Panics
    ///
    /// Panics when a temporary directory cannot be created or made private.
    #[must_use]
    pub fn with_seed(seed: u64) -> Self {
        let dir = TempDir::new().expect("temp state root");
        // The hostile-umask suites run under `umask 0777`, which strips every
        // bit from the directory `tempfile` asks for and leaves a root the test
        // itself cannot traverse. The state root's mode is the installer's
        // business; the artifact root's is `governor-artifacts`', and that is
        // what the permission suites assert about.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .expect("making the temp state root traversable");
        }
        std::fs::create_dir_all(dir.path().join("logs")).expect("creating the log directory");
        Self {
            dir,
            seed,
            opens: AtomicU64::new(0),
        }
    }

    /// The seed every port in this harness derives from.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The state root directory.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        self.dir.path()
    }

    /// Where the durable authority lives.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.dir.path().join("governor.sqlite3")
    }

    /// Where published artifacts live.
    #[must_use]
    pub fn artifact_root(&self) -> PathBuf {
        self.dir.path().join("artifacts")
    }

    /// Where the daemon's diagnostics will live.
    #[must_use]
    pub fn log_root(&self) -> PathBuf {
        self.dir.path().join("logs")
    }

    /// How many times this root has been opened.
    #[must_use]
    pub fn open_count(&self) -> u64 {
        self.opens.load(Ordering::Relaxed)
    }

    /// Opens the store, running the full startup sequence.
    ///
    /// # Errors
    ///
    /// Returns whatever [`OpenStore::start`] refused on: a policy violation, a
    /// newer schema epoch, a drifted migration, or a projection mismatch.
    pub fn open(&self) -> StoreResult<Store> {
        self.open_full(DEFAULT_CLOCK_START_MS, None)
            .map(|opened| opened.store)
    }

    /// Opens the store with a crash armed.
    ///
    /// # Errors
    ///
    /// As [`Harness::open`], plus the injected failure when the hook fires
    /// during startup itself.
    pub fn open_with(&self, hook: Option<Box<dyn FailpointHook>>) -> StoreResult<Store> {
        self.open_full(DEFAULT_CLOCK_START_MS, hook)
            .map(|opened| opened.store)
    }

    /// Opens the store as a process whose clock starts at `start_ms`.
    ///
    /// # Errors
    ///
    /// As [`Harness::open`].
    pub fn open_at(
        &self,
        start_ms: i64,
        hook: Option<Box<dyn FailpointHook>>,
    ) -> StoreResult<Store> {
        self.open_full(start_ms, hook).map(|opened| opened.store)
    }

    /// Opens the store and hands back its clock as well.
    ///
    /// # Errors
    ///
    /// As [`Harness::open`].
    pub fn open_full(
        &self,
        start_ms: i64,
        hook: Option<Box<dyn FailpointHook>>,
    ) -> StoreResult<OpenedStore> {
        let generation = self.opens.fetch_add(1, Ordering::Relaxed);
        let seed = self.seed.wrapping_add(generation.wrapping_mul(SEED_STRIDE));
        let clock = FakeClock::stepping(start_ms);
        let store = OpenStore {
            config: StoreConfig::new(self.database_path()),
            ports: StorePorts::new(
                Box::new(clock.clone()),
                Box::new(SeededRandom::new(seed)),
                Box::new(SeededIds::new(seed)),
            ),
            failpoints: hook,
            instance_id: Uuid::from_u128(0x00C0_FFEE),
        }
        .start()?;
        Ok(OpenedStore { store, clock })
    }

    /// Opens the artifact root with default policy and reproducible keys.
    ///
    /// # Panics
    ///
    /// Panics when the root cannot be opened or repaired.
    #[must_use]
    pub fn open_artifacts(&self) -> ArtifactStore {
        self.open_artifacts_with(ArtifactConfig::default(), Box::new(self.keys()), None)
    }

    /// The key source this harness hands the artifact store by default.
    #[must_use]
    pub const fn keys(&self) -> SeededKeys {
        SeededKeys::new(self.seed)
    }

    /// Opens the artifact root with explicit policy, keys and crash seam.
    ///
    /// # Panics
    ///
    /// Panics when the root cannot be opened or repaired.
    #[must_use]
    pub fn open_artifacts_with(
        &self,
        config: ArtifactConfig,
        keys: Box<dyn StorageKeySource>,
        failpoints: Option<Box<dyn ArtifactFailpointHook>>,
    ) -> ArtifactStore {
        OpenArtifactStore {
            root: self.artifact_root(),
            config,
            keys,
            failpoints,
        }
        .start()
        .expect("opening the artifact root")
    }

    /// A second, read-only connection for assertions about raw rows.
    ///
    /// WAL permits concurrent readers, so this observes exactly what a crashed
    /// process would leave behind without disturbing the writer.
    ///
    /// # Panics
    ///
    /// Panics when the database file cannot be opened for reading.
    #[must_use]
    pub fn inspect(&self) -> Connection {
        Connection::open_with_flags(self.database_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only inspection connection")
    }

    /// Database, write-ahead log and shared-memory index, concatenated.
    ///
    /// A value that was written and later overwritten still leaves its bytes in
    /// the WAL, so scanning only the main file would be too weak.
    #[must_use]
    pub fn database_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.database_path().into_os_string();
            path.push(suffix);
            if let Ok(bytes) = std::fs::read(Path::new(&path)) {
                out.extend_from_slice(&bytes);
            }
        }
        out
    }

    /// Every regular file under the state root, as `(relative path, bytes)`.
    ///
    /// This is the surface the SEC-001 sweep scans: the database and its
    /// sidecars, every artifact, every staging file, everything in quarantine,
    /// and anything the daemon later writes to `logs/`.
    ///
    /// # Panics
    ///
    /// Panics when a directory under the root cannot be listed.
    #[must_use]
    pub fn all_files(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        collect(self.state_root(), self.state_root(), &mut out);
        out.sort_by(|left, right| left.0.cmp(&right.0));
        out
    }

    /// Every regular file name directly inside one artifact-layout directory.
    #[must_use]
    pub fn files_in(&self, dir: &str) -> Vec<String> {
        let path = self.artifact_root().join(dir);
        let Ok(entries) = std::fs::read_dir(&path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect(root, &path, out);
        } else if metadata.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(&path).unwrap_or_default();
            out.push((name, bytes));
        }
    }
}
