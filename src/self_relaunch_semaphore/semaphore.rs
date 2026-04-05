//! Leader semaphore backed by a JSON lock file with PID ownership,
//! heartbeat timestamps, and a monotonic generation counter.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// Heartbeat staleness threshold — if the leader hasn't written a heartbeat
/// within this window, it is considered dead and the semaphore may be seized.
const DEFAULT_HEARTBEAT_STALE_SECS: u64 = 60;

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
    pub(crate) fn to_json(&self) -> String {
        format!(
            r#"{{"pid":{},"generation":{},"heartbeat_epoch":{}}}"#,
            self.pid, self.generation, self.heartbeat_epoch,
        )
    }

    pub(crate) fn from_json(s: &str) -> Option<Self> {
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
pub(crate) fn extract_u64(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

pub(crate) fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn is_pid_alive(pid: u32) -> bool {
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

    pub(crate) fn write_state(&self, state: &LeaderState) -> SimardResult<()> {
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
