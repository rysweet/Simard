//! Coordinated leader handoff: build canary → verify gates → spawn child →
//! confirm healthy → transfer leadership → old exits.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{SimardError, SimardResult};
use crate::self_relaunch::{
    RelaunchConfig, RelaunchGate, all_gates_passed, build_canary, default_gates, verify_canary,
};

use super::semaphore::LeaderSemaphore;

/// Maximum time to wait for a child to signal readiness.
const DEFAULT_CHILD_READY_TIMEOUT: Duration = Duration::from_secs(45);

/// Polling interval when waiting for child readiness.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);

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
pub(crate) fn wait_for_ready(ready_path: &Path, timeout: Duration) -> SimardResult<()> {
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
