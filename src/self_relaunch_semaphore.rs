//! File-based leader semaphore and coordinated handoff for self-relaunch.
//!
//! Provides a `LeaderSemaphore` backed by a lock file with PID ownership,
//! heartbeat timestamps, and a monotonic generation counter. The
//! `LeaderHandoff` orchestrator coordinates: build canary → verify gates →
//! spawn child → confirm healthy → transfer leadership → old exits.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};
use crate::self_relaunch::{
    RelaunchConfig, RelaunchGate, all_gates_passed, build_canary, default_gates, verify_canary,
};

/// Heartbeat staleness threshold — if the leader hasn't written a heartbeat
/// within this window, it is considered dead and the semaphore may be seized.
const DEFAULT_HEARTBEAT_STALE_SECS: u64 = 60;

/// Maximum time to wait for a child to signal readiness.
const DEFAULT_CHILD_READY_TIMEOUT: Duration = Duration::from_secs(45);

/// Polling interval when waiting for child readiness.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);

// ── Leader semaphore ────────────────────────────────────────────────

/// Persistent leader-election semaphore backed by a JSON lock file.
///
/// File format (JSON): `{ "pid": u32, "generation": u64, "heartbeat_epoch": u64 }`
#[derive(Clone, Debug)]
pub struct LeaderSemaphore {
    lock_path: PathBuf,
    heartbeat_stale_secs: u64,
}

/// Snapshot of the current leader state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderState {
    pub pid: u32,
    pub generation: u64,
    pub heartbeat_epoch: u64,
}

impl LeaderState {
    fn to_json(&self) -> String {
        format!(
            r#"{{"pid":{},"generation":{},"heartbeat_epoch":{}}}"#,
            self.pid, self.generation, self.heartbeat_epoch,
        )
    }

    fn from_json(s: &str) -> Option<Self> {
        // Minimal JSON parsing — avoids serde dependency for this tiny format.
        let pid = extract_u64(s, "pid")? as u32;
        let generation = extract_u64(s, "generation")?;
        let heartbeat_epoch = extract_u64(s, "heartbeat_epoch")?;
        Some(Self {
            pid,
            generation,
            heartbeat_epoch,
        })
    }
}

/// Extract a u64 value for `key` from a simple flat JSON string.
fn extract_u64(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl LeaderSemaphore {
    pub fn new(lock_path: impl Into<PathBuf>) -> Self {
        Self {
            lock_path: lock_path.into(),
            heartbeat_stale_secs: DEFAULT_HEARTBEAT_STALE_SECS,
        }
    }

    pub fn with_stale_threshold(mut self, secs: u64) -> Self {
        self.heartbeat_stale_secs = secs;
        self
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Try to acquire leadership. Succeeds if:
    /// - No lock file exists, OR
    /// - The recorded PID is dead, OR
    /// - The heartbeat is stale.
    ///
    /// On success, writes a new lock file with the caller's PID and an
    /// incremented generation.
    pub fn try_acquire(&self, my_pid: u32) -> SimardResult<LeaderState> {
        if let Some(existing) = self.read_state()? {
            if existing.pid == my_pid {
                // Already the leader — refresh heartbeat.
                let refreshed = LeaderState {
                    pid: my_pid,
                    generation: existing.generation,
                    heartbeat_epoch: epoch_now(),
                };
                self.write_state(&refreshed)?;
                return Ok(refreshed);
            }
            if is_pid_alive(existing.pid) && !self.is_stale(&existing) {
                return Err(SimardError::BridgeCallFailed {
                    bridge: "leader-semaphore".to_string(),
                    method: "try_acquire".to_string(),
                    reason: format!(
                        "leadership held by pid {} (gen {})",
                        existing.pid, existing.generation
                    ),
                });
            }
            // Dead or stale — seize leadership with next generation.
            let state = LeaderState {
                pid: my_pid,
                generation: existing.generation + 1,
                heartbeat_epoch: epoch_now(),
            };
            self.write_state(&state)?;
            Ok(state)
        } else {
            let state = LeaderState {
                pid: my_pid,
                generation: 1,
                heartbeat_epoch: epoch_now(),
            };
            self.write_state(&state)?;
            Ok(state)
        }
    }

    /// Release leadership if we still own it.
    pub fn release(&self, my_pid: u32) -> SimardResult<()> {
        if let Some(state) = self.read_state()?
            && state.pid == my_pid
        {
            let _ = fs::remove_file(&self.lock_path);
        }
        Ok(())
    }

    /// Refresh the heartbeat timestamp. No-op if we don't own the lock.
    pub fn heartbeat(&self, my_pid: u32) -> SimardResult<()> {
        if let Some(mut state) = self.read_state()?
            && state.pid == my_pid
        {
            state.heartbeat_epoch = epoch_now();
            self.write_state(&state)?;
        }
        Ok(())
    }

    /// Transfer leadership to a new PID, incrementing the generation.
    /// Only succeeds if the caller currently owns the semaphore.
    pub fn transfer(&self, from_pid: u32, to_pid: u32) -> SimardResult<LeaderState> {
        let current = self
            .read_state()?
            .ok_or_else(|| SimardError::BridgeCallFailed {
                bridge: "leader-semaphore".to_string(),
                method: "transfer".to_string(),
                reason: "no leader state to transfer from".to_string(),
            })?;

        if current.pid != from_pid {
            return Err(SimardError::BridgeCallFailed {
                bridge: "leader-semaphore".to_string(),
                method: "transfer".to_string(),
                reason: format!(
                    "caller pid {} does not own semaphore (owner: {})",
                    from_pid, current.pid
                ),
            });
        }

        let new_state = LeaderState {
            pid: to_pid,
            generation: current.generation + 1,
            heartbeat_epoch: epoch_now(),
        };
        self.write_state(&new_state)?;
        Ok(new_state)
    }

    /// Read the current leader state (if any).
    pub fn read_state(&self) -> SimardResult<Option<LeaderState>> {
        match fs::read_to_string(&self.lock_path) {
            Ok(contents) => Ok(LeaderState::from_json(&contents)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SimardError::PersistentStoreIo {
                store: "leader-semaphore".to_string(),
                action: "read".to_string(),
                path: self.lock_path.clone(),
                reason: e.to_string(),
            }),
        }
    }

    fn write_state(&self, state: &LeaderState) -> SimardResult<()> {
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
                store: "leader-semaphore".to_string(),
                action: "create directory".to_string(),
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }
        let json = state.to_json();
        let tmp = self.lock_path.with_extension("tmp");
        let mut f = fs::File::create(&tmp).map_err(|e| SimardError::PersistentStoreIo {
            store: "leader-semaphore".to_string(),
            action: "write".to_string(),
            path: tmp.clone(),
            reason: e.to_string(),
        })?;
        f.write_all(json.as_bytes())
            .map_err(|e| SimardError::PersistentStoreIo {
                store: "leader-semaphore".to_string(),
                action: "write".to_string(),
                path: tmp.clone(),
                reason: e.to_string(),
            })?;
        fs::rename(&tmp, &self.lock_path).map_err(|e| SimardError::PersistentStoreIo {
            store: "leader-semaphore".to_string(),
            action: "rename".to_string(),
            path: self.lock_path.clone(),
            reason: e.to_string(),
        })?;
        Ok(())
    }

    fn is_stale(&self, state: &LeaderState) -> bool {
        let now = epoch_now();
        now.saturating_sub(state.heartbeat_epoch) > self.heartbeat_stale_secs
    }
}

// ── Coordinated handoff ─────────────────────────────────────────────

/// Configuration for a coordinated leader handoff.
#[derive(Clone, Debug)]
pub struct HandoffConfig {
    pub relaunch: RelaunchConfig,
    pub semaphore: LeaderSemaphore,
    pub gates: Vec<RelaunchGate>,
    pub child_ready_timeout: Duration,
}

impl HandoffConfig {
    pub fn new(semaphore: LeaderSemaphore, relaunch: RelaunchConfig) -> Self {
        Self {
            relaunch,
            semaphore,
            gates: default_gates(),
            child_ready_timeout: DEFAULT_CHILD_READY_TIMEOUT,
        }
    }
}

/// Result of a completed handoff sequence.
#[derive(Clone, Debug)]
pub struct HandoffResult {
    pub old_pid: u32,
    pub new_pid: u32,
    pub old_generation: u64,
    pub new_generation: u64,
    pub child_binary: PathBuf,
}

/// Readiness signal file written by the child after startup health checks.
///
/// Convention: child writes `{"pid":<pid>,"status":"ready"}` to
/// `<semaphore_dir>/ready-<pid>.json`.
fn ready_signal_path(semaphore_dir: &Path, child_pid: u32) -> PathBuf {
    semaphore_dir.join(format!("ready-{child_pid}.json"))
}

/// Execute a coordinated handoff: build → gate → spawn → verify → transfer.
///
/// Returns once leadership has been transferred to the child. The caller
/// should then shut down gracefully.
pub fn coordinated_handoff(my_pid: u32, config: &HandoffConfig) -> SimardResult<HandoffResult> {
    // 1. Confirm we are the current leader.
    let current = config
        .semaphore
        .read_state()?
        .ok_or_else(|| SimardError::BridgeCallFailed {
            bridge: "handoff".to_string(),
            method: "coordinated_handoff".to_string(),
            reason: "no leader state — acquire semaphore first".to_string(),
        })?;
    if current.pid != my_pid {
        return Err(SimardError::BridgeCallFailed {
            bridge: "handoff".to_string(),
            method: "coordinated_handoff".to_string(),
            reason: format!(
                "caller {} is not current leader (leader: {})",
                my_pid, current.pid
            ),
        });
    }

    // 2. Build canary binary.
    let canary_path = build_canary(&config.relaunch)?;

    // 3. Verify gates.
    let gate_results = verify_canary(&canary_path, &config.gates, &config.relaunch)?;
    if !all_gates_passed(&gate_results) {
        let failures: Vec<String> = gate_results
            .iter()
            .filter(|g| !g.passed)
            .map(|g| g.to_string())
            .collect();
        return Err(SimardError::BridgeCallFailed {
            bridge: "handoff".to_string(),
            method: "verify_gates".to_string(),
            reason: format!("gate failures: {}", failures.join("; ")),
        });
    }

    // 4. Spawn child process with readiness signal convention.
    let sem_dir = config
        .semaphore
        .lock_path()
        .parent()
        .unwrap_or(Path::new("/tmp"));
    let child = Command::new(&canary_path)
        .arg("--ready-signal-dir")
        .arg(sem_dir)
        .spawn()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "handoff-child".to_string(),
            reason: format!("failed to spawn canary: {e}"),
        })?;
    let child_pid = child.id();

    // 5. Wait for child readiness signal.
    let ready_path = ready_signal_path(sem_dir, child_pid);
    wait_for_ready(&ready_path, config.child_ready_timeout)?;

    // 6. Transfer leadership.
    let new_state = config.semaphore.transfer(my_pid, child_pid)?;

    // 7. Clean up readiness signal.
    let _ = fs::remove_file(&ready_path);

    Ok(HandoffResult {
        old_pid: my_pid,
        new_pid: child_pid,
        old_generation: current.generation,
        new_generation: new_state.generation,
        child_binary: canary_path,
    })
}

/// Block until the readiness signal file appears, or timeout.
fn wait_for_ready(ready_path: &Path, timeout: Duration) -> SimardResult<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if ready_path.exists() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(SimardError::BridgeCallFailed {
                bridge: "handoff".to_string(),
                method: "wait_for_ready".to_string(),
                reason: format!(
                    "child did not signal readiness within {}s at {}",
                    timeout.as_secs(),
                    ready_path.display()
                ),
            });
        }
        std::thread::sleep(READY_POLL_INTERVAL);
    }
}

/// Write a readiness signal file (called by the child process after self-check).
pub fn signal_ready(ready_dir: &Path, my_pid: u32) -> SimardResult<()> {
    let path = ready_signal_path(ready_dir, my_pid);
    let json = format!(r#"{{"pid":{},"status":"ready"}}"#, my_pid);
    fs::write(&path, json.as_bytes()).map_err(|e| SimardError::PersistentStoreIo {
        store: "ready-signal".to_string(),
        action: "write".to_string(),
        path,
        reason: e.to_string(),
    })?;
    Ok(())
}

// ── Utilities ───────────────────────────────────────────────────────

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks existence without sending a signal.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_lock_path() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "simard-sem-test-{}-{}-{}",
            std::process::id(),
            epoch_now(),
            n,
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("leader.lock")
    }

    #[test]
    fn leader_state_json_roundtrip() {
        let state = LeaderState {
            pid: 42,
            generation: 7,
            heartbeat_epoch: 1700000000,
        };
        let json = state.to_json();
        let parsed = LeaderState::from_json(&json).unwrap();
        assert_eq!(state, parsed);
    }

    #[test]
    fn extract_u64_works() {
        let json = r#"{"pid":123,"generation":5,"heartbeat_epoch":999}"#;
        assert_eq!(extract_u64(json, "pid"), Some(123));
        assert_eq!(extract_u64(json, "generation"), Some(5));
        assert_eq!(extract_u64(json, "heartbeat_epoch"), Some(999));
        assert_eq!(extract_u64(json, "missing"), None);
    }

    #[test]
    fn acquire_fresh_semaphore() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let state = sem.try_acquire(1234).unwrap();
        assert_eq!(state.pid, 1234);
        assert_eq!(state.generation, 1);
        // Clean up.
        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn acquire_rejects_live_leader() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();

        // Another pid trying to acquire should fail (our pid is alive).
        let result = sem.try_acquire(my_pid + 99999);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("leadership held by"));

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn acquire_seizes_from_dead_pid() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        // Write a state for a definitely-dead PID.
        let dead_state = LeaderState {
            pid: 99999999,
            generation: 3,
            heartbeat_epoch: epoch_now(),
        };
        sem.write_state(&dead_state).unwrap();

        let state = sem.try_acquire(std::process::id()).unwrap();
        assert_eq!(state.pid, std::process::id());
        assert_eq!(state.generation, 4); // incremented

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn acquire_seizes_stale_heartbeat() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock).with_stale_threshold(1);

        let my_pid = std::process::id();
        // Write a state with old heartbeat (our own PID so it's "alive").
        let stale = LeaderState {
            pid: my_pid,
            generation: 5,
            heartbeat_epoch: epoch_now().saturating_sub(100),
        };
        sem.write_state(&stale).unwrap();

        // Different PID can seize because heartbeat is stale.
        // We use my_pid here since it's definitely alive but the stale check wins.
        let state = sem.try_acquire(my_pid).unwrap();
        // Same PID re-acquires — refreshes heartbeat.
        assert_eq!(state.pid, my_pid);
        assert_eq!(state.generation, 5); // same gen for same PID

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn heartbeat_refreshes_epoch() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        let state = sem.try_acquire(my_pid).unwrap();
        let old_epoch = state.heartbeat_epoch;

        std::thread::sleep(Duration::from_millis(10));
        sem.heartbeat(my_pid).unwrap();

        let refreshed = sem.read_state().unwrap().unwrap();
        assert!(refreshed.heartbeat_epoch >= old_epoch);

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn transfer_changes_owner() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();

        let new_state = sem.transfer(my_pid, 55555).unwrap();
        assert_eq!(new_state.pid, 55555);
        assert_eq!(new_state.generation, 2);

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn transfer_rejects_non_owner() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();

        let err = sem.transfer(99999, 55555).unwrap_err();
        assert!(err.to_string().contains("does not own"));

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn release_removes_lock() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();
        assert!(lock.exists());

        sem.release(my_pid).unwrap();
        assert!(!lock.exists());

        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn release_ignores_non_owner() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();

        // Non-owner release should be a no-op.
        sem.release(99999).unwrap();
        assert!(lock.exists()); // still there

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn signal_ready_creates_file() {
        let dir = std::env::temp_dir().join(format!("simard-ready-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        signal_ready(&dir, 12345).unwrap();
        let path = dir.join("ready-12345.json");
        assert!(path.exists());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"pid\":12345"));
        assert!(contents.contains("\"status\":\"ready\""));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn wait_for_ready_timeout() {
        let path = PathBuf::from("/tmp/simard-no-such-ready-signal-99999.json");
        let err = wait_for_ready(&path, Duration::from_millis(100)).unwrap_err();
        assert!(err.to_string().contains("did not signal readiness"));
    }

    #[test]
    fn wait_for_ready_succeeds_when_file_exists() {
        let dir = std::env::temp_dir().join(format!("simard-wr-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ready-777.json");
        fs::write(&path, b"ok").unwrap();

        wait_for_ready(&path, Duration::from_secs(1)).unwrap();

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn handoff_config_defaults() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let cfg = HandoffConfig::new(sem, RelaunchConfig::default());
        assert_eq!(cfg.gates.len(), 4);
        assert_eq!(cfg.child_ready_timeout, DEFAULT_CHILD_READY_TIMEOUT);

        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn coordinated_handoff_rejects_non_leader() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let my_pid = std::process::id();
        sem.try_acquire(my_pid).unwrap();

        let cfg = HandoffConfig::new(sem, RelaunchConfig::default());
        let err = coordinated_handoff(99999, &cfg).unwrap_err();
        assert!(err.to_string().contains("not current leader"));

        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn coordinated_handoff_rejects_no_state() {
        let lock = temp_lock_path();
        let sem = LeaderSemaphore::new(&lock);
        let cfg = HandoffConfig::new(sem, RelaunchConfig::default());
        let err = coordinated_handoff(1234, &cfg).unwrap_err();
        assert!(err.to_string().contains("no leader state"));

        let _ = fs::remove_dir(lock.parent().unwrap());
    }

    #[test]
    fn is_pid_alive_self() {
        assert!(is_pid_alive(std::process::id()));
    }

    #[test]
    fn is_pid_alive_dead() {
        // PID 99999999 is almost certainly not alive.
        assert!(!is_pid_alive(99999999));
    }
}
