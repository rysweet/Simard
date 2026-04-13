use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::SimardError;

const LOCK_FILENAME: &str = "cargo_build.lock";
const STALE_THRESHOLD_SECS: u64 = 600; // 10 minutes — assume dead build

/// Manages exclusive access to cargo builds via a lock file.
///
/// Only one cargo build can run at a time on a host. Others block
/// with FIFO-like semantics (poll + sleep) until the lock is released.
pub struct BuildLock {
    lock_path: PathBuf,
}

/// RAII guard — releases the lock file on drop.
pub struct BuildLockGuard {
    lock_path: PathBuf,
}

impl Drop for BuildLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

impl BuildLock {
    pub fn new(state_root: &Path) -> Self {
        Self {
            lock_path: state_root.join(LOCK_FILENAME),
        }
    }

    /// Default state root (`~/.simard`).
    pub fn default_state_root() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
        PathBuf::from(home).join(".simard")
    }

    /// Attempt to acquire the build lock immediately. Returns `None` if held.
    pub fn try_acquire(&self) -> Result<Option<BuildLockGuard>, SimardError> {
        self.reap_stale()?;

        if self.lock_path.exists() {
            return Ok(None);
        }

        self.write_lock_file()?;
        Ok(Some(BuildLockGuard {
            lock_path: self.lock_path.clone(),
        }))
    }

    /// Block until the build lock is acquired (with timeout).
    pub fn acquire(&self, timeout: Duration) -> Result<BuildLockGuard, SimardError> {
        let start = Instant::now();
        let poll_interval = Duration::from_millis(500);

        loop {
            if let Some(guard) = self.try_acquire()? {
                return Ok(guard);
            }

            if start.elapsed() >= timeout {
                let holder = self.current_holder();
                return Err(SimardError::CommandTimeout {
                    action: format!(
                        "acquire cargo build lock (held by {})",
                        holder.unwrap_or_else(|| "unknown".into())
                    ),
                    timeout_secs: timeout.as_secs(),
                });
            }

            std::thread::sleep(poll_interval);
        }
    }

    /// Return information about the current lock holder.
    pub fn current_holder(&self) -> Option<String> {
        std::fs::read_to_string(&self.lock_path).ok()
    }

    /// Is the lock currently held?
    pub fn is_locked(&self) -> bool {
        self.lock_path.exists()
    }

    /// Force-release a stale lock (e.g., from a crashed process).
    pub fn force_release(&self) -> Result<bool, SimardError> {
        if self.lock_path.exists() {
            std::fs::remove_file(&self.lock_path).map_err(|e| SimardError::PersistentStoreIo {
                store: "build_lock".into(),
                action: "force_release".into(),
                path: self.lock_path.clone(),
                reason: e.to_string(),
            })?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn write_lock_file(&self) -> Result<(), SimardError> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
                store: "build_lock".into(),
                action: "mkdir".into(),
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }

        let info = format!(
            "pid={}\nhost={}\nstarted={}\n",
            std::process::id(),
            crate::agent_registry::hostname(),
            chrono::Utc::now().to_rfc3339(),
        );
        std::fs::write(&self.lock_path, info).map_err(|e| SimardError::PersistentStoreIo {
            store: "build_lock".into(),
            action: "write".into(),
            path: self.lock_path.clone(),
            reason: e.to_string(),
        })
    }

    fn reap_stale(&self) -> Result<(), SimardError> {
        if !self.lock_path.exists() {
            return Ok(());
        }
        let metadata =
            std::fs::metadata(&self.lock_path).map_err(|e| SimardError::PersistentStoreIo {
                store: "build_lock".into(),
                action: "stat".into(),
                path: self.lock_path.clone(),
                reason: e.to_string(),
            })?;

        let age = metadata
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or(Duration::ZERO);

        if age > Duration::from_secs(STALE_THRESHOLD_SECS) {
            tracing::warn!("Reaping stale cargo build lock (age: {}s)", age.as_secs());
            std::fs::remove_file(&self.lock_path).map_err(|e| SimardError::PersistentStoreIo {
                store: "build_lock".into(),
                action: "reap_stale".into(),
                path: self.lock_path.clone(),
                reason: e.to_string(),
            })?;
        } else {
            // Check if holding PID is still alive (local only)
            if let Ok(content) = std::fs::read_to_string(&self.lock_path)
                && let Some(pid_str) = content
                    .lines()
                    .find(|l| l.starts_with("pid="))
                    .and_then(|l| l.strip_prefix("pid="))
                && let Ok(pid) = pid_str.parse::<u32>()
                && !Path::new(&format!("/proc/{pid}")).exists()
            {
                tracing::warn!("Reaping build lock held by dead PID {pid}");
                std::fs::remove_file(&self.lock_path).map_err(|e| {
                    SimardError::PersistentStoreIo {
                        store: "build_lock".into(),
                        action: "reap_dead_holder".into(),
                        path: self.lock_path.clone(),
                        reason: e.to_string(),
                    }
                })?;
            }
        }

        Ok(())
    }
}
