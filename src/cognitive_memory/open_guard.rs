//! Cross-process open serialization for the lbug-backed cognitive store
//! (issue: "Stop lbug lock-contention from being mistaken for catalog
//! corruption and WIPING cognitive memory").
//!
//! # Why this exists
//!
//! The upstream `amplihack-memory-lib` `open_persistent` path treats **any**
//! failed strict open of the main database as *catalog corruption*: it
//! quarantines the DB to `<db>.corrupt-<ts>` and rebuilds a **fresh, empty**
//! store (`recovered_records = 0`). That self-heal is correct for a genuinely
//! unopenable catalog, but it does **not** distinguish a transient
//! cross-process **lock conflict** (`Could not set lock on file: .../cognitive
//! (Lock is held by PID N)`) from real corruption. lbug takes a POSIX/PID lock
//! on the store, so a *second process* opening a store that a first process
//! already holds open trips the "corruption" branch and **destroys all
//! memory**. On the daemon main store this produced dozens of
//! `cognitive.corrupt-*` quarantines.
//!
//! The mis-classification itself lives in the library and cannot be fixed from
//! Simard. What Simard *can* do — and what this module does — is **serialize
//! opens across processes** at the [`LibraryCognitiveMemory::open`] seam so the
//! library never sees a concurrent open on the same path, and therefore never
//! trips its lock-conflict-as-corruption branch.
//!
//! # Behaviour
//!
//! Before opening the store, [`CognitiveOpenGuard::acquire`] takes an exclusive
//! advisory `flock` on a **sidecar** lock file (`<state_root>/cognitive.open.lock`,
//! a sibling of the `cognitive` store directory so lbug never touches it):
//!
//!   * **Acquired** -> proceed to `open_persistent`. The guard is held for the
//!     entire lifetime of the [`LibraryCognitiveMemory`] handle and released
//!     (`flock(LOCK_UN)`) only *after* the inner store has closed, so no other
//!     process can slip in while lbug is still releasing its own PID lock.
//!   * **Contended past the budget** -> **FAIL LOUD** with
//!     [`SimardError::PersistentStoreIo`]. We return an error instead of
//!     proceeding, which is what prevents the library from quarantining and
//!     rebuilding empty. Failing loud is strictly better than silently wiping.
//!
//! Acquisition uses **bounded exponential backoff** with jitter. `flock`
//! auto-releases when the holding process dies (the kernel drops the lock on
//! FD close / process exit), so a crashed holder never wedges the store — no
//! manual stale-lock reaping is required.
//!
//! # Same-process re-entrancy
//!
//! lbug's PID lock is re-entrant for the *same* process (a second open in the
//! same PID succeeds), and Simard relies on that: e.g. a daemon writer and a
//! same-process reader view can both be live at once. `flock`, by contrast, is
//! per open-file-description and would *block a same-process* second open. To
//! preserve the library's semantics, this module keeps a **process-global
//! registry** keyed by the canonical lock path: the first open in a process
//! takes the real `flock`; concurrent same-process opens of the same path share
//! that one lock via a reference-counted handle (no second `flock`, no wait).
//! The `flock` is released once the last same-process handle drops.

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Arc, Mutex, OnceLock, Weak};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use crate::error::SimardError;
use crate::error::SimardResult;

/// Sidecar lock-file name, a sibling of the `cognitive` store directory under
/// `state_root`. Deliberately distinct from the legacy
/// `cognitive_memory.ladybug.open.lock` reaped by
/// [`crate::memory_ipc::reap_stale_open_lock`] so that reaper can never delete a
/// live lock file this module is holding.
#[cfg(unix)]
pub(crate) const OPEN_LOCK_FILE: &str = "cognitive.open.lock";

/// Default acquisition budget: the maximum time an opener will back off waiting
/// for a contended store before failing loud. Transient near-simultaneous open
/// races resolve in milliseconds; a long wait means another process is holding
/// the store open (it should be reached via the daemon IPC path, not a direct
/// second open), so failing loud after the budget is the correct outcome.
#[cfg(unix)]
const DEFAULT_BUDGET: Duration = Duration::from_millis(15_000);

/// Environment override for [`DEFAULT_BUDGET`], in milliseconds. Primarily for
/// tests, which set it low to assert the fail-loud path quickly.
#[cfg(unix)]
const BUDGET_ENV: &str = "SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS";

/// Process-global registry of live open-locks, keyed by canonical lock path.
/// A `Weak` so a dropped guard's entry can be cleaned up and the `flock`
/// released; a live entry lets same-process opens share one `flock`.
#[cfg(unix)]
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<ProcessOpenLock>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<ProcessOpenLock>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The one real `flock` held per (process, path). Dropping it releases the
/// advisory lock and removes the registry entry.
#[cfg(unix)]
#[derive(Debug)]
struct ProcessOpenLock {
    file: std::fs::File,
    lock_path: PathBuf,
}

#[cfg(unix)]
impl Drop for ProcessOpenLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // Release the advisory lock. The file itself is intentionally left on
        // disk: unlinking it would let a concurrent opener create a *new* inode
        // and `flock` a different file, defeating the mutual exclusion (the
        // classic lock-file + unlink race). A persistent zero-byte lock file is
        // the standard, safe pattern (mirrors `goal_board_store::StoreLock`).
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
        if let Ok(mut map) = registry().lock() {
            // Only remove if the entry is dead (no other live handle re-inserted
            // a fresh lock for this path in the meantime).
            if map.get(&self.lock_path).map(Weak::strong_count) == Some(0) {
                map.remove(&self.lock_path);
            }
        }
    }
}

/// RAII guard proving the caller holds the cross-process open-lock for a
/// cognitive store path. Held for the lifetime of the owning
/// [`crate::cognitive_memory::LibraryCognitiveMemory`].
///
/// On non-unix targets this is a zero-cost no-op (the project ships unix-only,
/// but the guard stays `cfg`-portable so doc/lint passes for other targets
/// still build).
pub(crate) struct CognitiveOpenGuard {
    #[cfg(unix)]
    _lock: Arc<ProcessOpenLock>,
}

#[cfg(unix)]
impl std::fmt::Debug for CognitiveOpenGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CognitiveOpenGuard")
            .field("lock_path", &self._lock.lock_path)
            .finish()
    }
}

#[cfg(not(unix))]
impl std::fmt::Debug for CognitiveOpenGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CognitiveOpenGuard").finish()
    }
}

impl CognitiveOpenGuard {
    /// Acquire the cross-process open-lock for the cognitive store under
    /// `state_root`, blocking with **bounded exponential backoff** until either
    /// the lock is held or the budget expires.
    ///
    /// # Errors
    ///
    /// Returns [`SimardError::PersistentStoreIo`] if the lock cannot be acquired
    /// within the budget (another live process holds the store open) or if the
    /// lock file / its parent directory cannot be created. Returning an error
    /// here is the whole point: it stops the caller from proceeding into the
    /// library's lock-conflict-as-corruption rebuild, which would wipe memory.
    #[cfg(unix)]
    pub(crate) fn acquire(state_root: &Path) -> SimardResult<Self> {
        use std::os::unix::io::AsRawFd;

        // Ensure `state_root` exists *before* canonicalizing so the registry key
        // (a canonical path) is stable even on the very first open of a fresh
        // root. Without this, `canonicalize` would fail on a missing dir and the
        // first opener would key off a non-canonical path while later openers
        // key off the canonical one, silently defeating same-process sharing.
        std::fs::create_dir_all(state_root).map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-open-lock".to_string(),
            action: "mkdir".to_string(),
            path: state_root.to_path_buf(),
            reason: e.to_string(),
        })?;

        let lock_path = lock_path_for(state_root);

        // Fast path: another handle in *this* process already holds the lock for
        // this path. Share it (re-entrant, matching lbug's per-PID semantics) —
        // no second `flock`, no wait.
        if let Some(existing) = live_lock_for(&lock_path) {
            return Ok(Self { _lock: existing });
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&lock_path)
            .map_err(|e| SimardError::PersistentStoreIo {
                store: "cognitive-open-lock".to_string(),
                action: "open_lock_file".to_string(),
                path: lock_path.clone(),
                reason: e.to_string(),
            })?;
        let fd = file.as_raw_fd();

        let budget = configured_budget();
        let start = Instant::now();
        let mut delay = Duration::from_millis(25);
        loop {
            // Hold the registry lock across the "is it already held in this
            // process?" check AND the non-blocking `flock` attempt, so two
            // same-process threads racing a *cold* open can never both proceed
            // to `flock` (one would otherwise spin to failure). The loser sees
            // the winner's registered entry on a subsequent iteration and shares
            // it instead of failing.
            {
                let mut map = registry()
                    .lock()
                    .map_err(|_| SimardError::PersistentStoreIo {
                        store: "cognitive-open-lock".to_string(),
                        action: "registry_lock".to_string(),
                        path: lock_path.clone(),
                        reason: "open-lock registry mutex poisoned".to_string(),
                    })?;

                // Re-check under the lock: another thread in this process may
                // have taken the lock since our pre-loop fast path.
                if let Some(weak) = map.get(&lock_path) {
                    match weak.upgrade() {
                        Some(existing) => return Ok(Self { _lock: existing }),
                        // Dead entry from a dropped guard: clear it so we can
                        // re-acquire below.
                        None => {
                            map.remove(&lock_path);
                        }
                    }
                }

                // Non-blocking exclusive lock: succeeds only if no other
                // open-file-description (in any process) holds it.
                let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if ret == 0 {
                    record_holder(&file);
                    let lock = Arc::new(ProcessOpenLock {
                        file,
                        lock_path: lock_path.clone(),
                    });
                    // Publish so concurrent same-process opens can share it.
                    map.insert(lock_path.clone(), Arc::downgrade(&lock));
                    return Ok(Self { _lock: lock });
                }

                let err = std::io::Error::last_os_error();
                // EWOULDBLOCK means "held by someone else" — retry until the
                // budget is spent. Any other errno is a real failure; surface
                // it loudly.
                if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
                    return Err(SimardError::PersistentStoreIo {
                        store: "cognitive-open-lock".to_string(),
                        action: "flock".to_string(),
                        path: lock_path.clone(),
                        reason: format!("flock(LOCK_EX|LOCK_NB) failed: {err}"),
                    });
                }
                // Drop the registry lock before sleeping so same-process
                // sharers are not blocked while we back off.
            }

            let elapsed = start.elapsed();
            if elapsed >= budget {
                let holder = current_holder(&lock_path);
                return Err(SimardError::PersistentStoreIo {
                    store: "cognitive-open-lock".to_string(),
                    action: "acquire_open_lock".to_string(),
                    path: lock_path.clone(),
                    reason: format!(
                        "cognitive store is held open by another process ({holder}); \
                         refusing to open a second concurrent handle after waiting {}ms. \
                         Opening anyway would trip the lbug lock-conflict-as-corruption \
                         path and wipe memory. Route access through the daemon IPC, or \
                         use an isolated state root for this run.",
                        budget.as_millis(),
                    ),
                });
            }

            // Exponential backoff, capped, with a little jitter, never sleeping
            // past the remaining budget.
            let remaining = budget - elapsed;
            let jitter = Duration::from_millis(u64::from(std::process::id() % 13));
            std::thread::sleep((delay + jitter).min(remaining));
            delay = (delay * 2).min(Duration::from_millis(500));
        }
    }

    /// Non-unix no-op acquisition.
    #[cfg(not(unix))]
    pub(crate) fn acquire(_state_root: &std::path::Path) -> SimardResult<Self> {
        Ok(Self {})
    }
}

/// Resolve the sidecar lock path for `state_root`, canonicalizing the parent so
/// two spellings of the same directory map to one registry key. Canonicalizing
/// the `state_root` (not the not-yet-created lock file) keeps the key stable on
/// first open.
#[cfg(unix)]
fn lock_path_for(state_root: &Path) -> PathBuf {
    let base = std::fs::canonicalize(state_root).unwrap_or_else(|_| state_root.to_path_buf());
    base.join(OPEN_LOCK_FILE)
}

/// Return a live shared handle for `lock_path` if this process already holds the
/// open-lock for it, cleaning up a dead entry otherwise.
#[cfg(unix)]
fn live_lock_for(lock_path: &Path) -> Option<Arc<ProcessOpenLock>> {
    let mut map = registry().lock().ok()?;
    match map.get(lock_path) {
        Some(weak) => match weak.upgrade() {
            Some(arc) => Some(arc),
            None => {
                map.remove(lock_path);
                None
            }
        },
        None => None,
    }
}

/// Budget from [`BUDGET_ENV`] (milliseconds) or [`DEFAULT_BUDGET`].
#[cfg(unix)]
fn configured_budget() -> Duration {
    std::env::var(BUDGET_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_BUDGET)
}

/// Best-effort record of the current holder into the lock file (for diagnostics
/// and the contended-error message). Failure to write is non-fatal — the
/// `flock` is what enforces exclusion, not the file contents.
#[cfg(unix)]
fn record_holder(file: &std::fs::File) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = file;
    let _ = f.set_len(0);
    let _ = f.seek(SeekFrom::Start(0));
    let _ = writeln!(
        f,
        "pid={}\nhost={}\nacquired={}",
        std::process::id(),
        crate::agent_registry::hostname(),
        chrono::Utc::now().to_rfc3339(),
    );
    let _ = f.flush();
}

/// Read back the recorded holder for a contended-lock error message.
#[cfg(unix)]
fn current_holder(lock_path: &Path) -> String {
    match std::fs::read_to_string(lock_path) {
        Ok(contents) => {
            let pid = contents
                .lines()
                .find_map(|l| l.strip_prefix("pid="))
                .unwrap_or("unknown");
            format!("PID {pid}")
        }
        Err(_) => "unknown PID".to_string(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simard-open-guard-{}-{}-{}",
            label,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        std::fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    /// A foreign `flock` holder simulating a *different process* holding the
    /// store open (a raw exclusive `flock` on the sidecar via its own FD).
    struct ForeignHolder {
        _file: std::fs::File,
    }

    impl ForeignHolder {
        fn hold(lock_path: &Path) -> Self {
            if let Some(parent) = lock_path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .read(true)
                .open(lock_path)
                .expect("open lock file");
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            assert_eq!(ret, 0, "foreign holder must acquire the flock");
            Self { _file: file }
        }
    }

    impl Drop for ForeignHolder {
        fn drop(&mut self) {
            unsafe {
                libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }

    #[test]
    fn acquire_and_release_roundtrip() {
        let root = temp_root("roundtrip");
        {
            let _g = CognitiveOpenGuard::acquire(&root).expect("acquire");
            // Registry has a live entry while the guard is alive.
            let key = lock_path_for(&root);
            assert!(live_lock_for(&key).is_some(), "guard should be registered");
        }
        // After drop, a fresh acquire still works (lock released).
        let _g2 = CognitiveOpenGuard::acquire(&root).expect("re-acquire after drop");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn same_process_reentrant_acquire_does_not_block() {
        let root = temp_root("reentrant");
        // Two live guards in the SAME process must both succeed (mirrors lbug's
        // per-PID re-entrancy: a daemon writer + same-process reader view).
        let g1 = CognitiveOpenGuard::acquire(&root).expect("first acquire");
        let g2 = CognitiveOpenGuard::acquire(&root).expect("re-entrant acquire");
        drop(g1);
        drop(g2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn concurrent_cold_open_race_all_succeed_same_process() {
        // A cold open (no prior holder) raced by many threads in one process
        // must NOT let the losers spin to failure: the registry check+flock is
        // atomic, so exactly one takes the real flock and the rest share it.
        // SAFETY: single-threaded set before spawning; short budget bounds the
        // worst case if the atomicity ever regressed.
        unsafe { std::env::set_var(BUDGET_ENV, "5000") };
        let root = temp_root("cold-race");
        let mut handles = Vec::new();
        for _ in 0..16 {
            let r = root.clone();
            handles.push(std::thread::spawn(move || {
                CognitiveOpenGuard::acquire(&r).is_ok()
            }));
        }
        let all_ok = handles.into_iter().all(|h| h.join().unwrap_or(false));
        unsafe { std::env::remove_var(BUDGET_ENV) };
        assert!(
            all_ok,
            "every concurrent same-process cold open must succeed (shared flock)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_entry_cleared_after_last_guard_drops() {
        let root = temp_root("registry-clear");
        let key = lock_path_for(&root);
        {
            let _g = CognitiveOpenGuard::acquire(&root).expect("acquire");
            assert!(live_lock_for(&key).is_some(), "entry live while guard held");
        }
        assert!(
            live_lock_for(&key).is_none(),
            "registry entry must be cleared once the last guard drops"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn contended_by_foreign_holder_fails_loud_within_budget() {
        // SAFETY: single-threaded test; env is scoped to this test's body.
        unsafe { std::env::set_var(BUDGET_ENV, "300") };
        let root = temp_root("contended");
        let key = lock_path_for(&root);
        let holder = ForeignHolder::hold(&key);

        let start = Instant::now();
        let err = CognitiveOpenGuard::acquire(&root)
            .expect_err("a store held open by another process must fail loud, not proceed");
        let waited = start.elapsed();

        match err {
            SimardError::PersistentStoreIo { action, reason, .. } => {
                assert_eq!(action, "acquire_open_lock");
                assert!(
                    reason.contains("held open by another process"),
                    "error must explain the contention, got: {reason}"
                );
            }
            other => panic!("expected PersistentStoreIo, got {other:?}"),
        }
        // Bounded: must not wait far past the configured budget.
        assert!(
            waited < Duration::from_secs(3),
            "acquire should give up near the budget, waited {waited:?}"
        );

        drop(holder);
        unsafe { std::env::remove_var(BUDGET_ENV) };
        // Once the foreign holder releases, acquire succeeds again.
        let _g = CognitiveOpenGuard::acquire(&root).expect("acquire after holder released");
        let _ = std::fs::remove_dir_all(&root);
    }
}
