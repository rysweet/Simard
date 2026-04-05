//! Subordinate spawning, heartbeat checking, and termination.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_goal_assignment::{SubordinateProgress, poll_progress};
use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

use super::STALE_THRESHOLD_SECONDS;
use super::types::{HeartbeatStatus, SubordinateConfig, SubordinateHandle};

/// Spawn a subordinate agent as a real child process.
///
/// Forks a new Simard process via `Command::new(current_exe())` in the
/// given worktree, passing `--agent-name`, `--goal`, and `--depth` as
/// arguments. The child process inherits the parent's environment.
///
/// The function validates the configuration (depth limits, non-empty
/// fields) before spawning.
pub fn spawn_subordinate(config: &SubordinateConfig) -> SimardResult<SubordinateHandle> {
    config.validate()?;

    let now = current_epoch_seconds()?;

    let exe = std::env::current_exe().map_err(|e| SimardError::BridgeSpawnFailed {
        bridge: "subordinate".to_string(),
        reason: format!("cannot resolve current executable: {e}"),
    })?;

    let child = Command::new(&exe)
        .arg("engineer")
        .arg("run")
        .arg("local")
        .arg(&config.worktree_path)
        .arg(&config.goal)
        .env("SIMARD_AGENT_NAME", &config.agent_name)
        .env(
            "SIMARD_SUBORDINATE_DEPTH",
            (config.current_depth + 1).to_string(),
        )
        .current_dir(&config.worktree_path)
        .spawn()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "subordinate".to_string(),
            reason: format!(
                "failed to spawn subordinate '{}' at '{}': {e}",
                config.agent_name,
                exe.display()
            ),
        })?;

    let pid = child.id();

    Ok(SubordinateHandle {
        pid,
        agent_name: config.agent_name.clone(),
        goal: config.goal.clone(),
        worktree_path: config.worktree_path.clone(),
        spawn_time: now,
        retry_count: 0,
        killed: false,
    })
}

/// Check the heartbeat of a subordinate by polling progress from the hive.
///
/// Returns `HeartbeatStatus::Alive` if a recent progress report exists,
/// `Stale` if the last report is older than the threshold, or `Dead` if
/// no progress has ever been reported.
pub fn check_heartbeat(
    handle: &SubordinateHandle,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<HeartbeatStatus> {
    if handle.killed {
        return Ok(HeartbeatStatus::Dead);
    }

    let progress = poll_progress(&handle.agent_name, bridge)?;

    match progress {
        None => Ok(HeartbeatStatus::Dead),
        Some(progress) => {
            let now = current_epoch_seconds()?;
            let elapsed = now.saturating_sub(progress.heartbeat_epoch);

            if elapsed > STALE_THRESHOLD_SECONDS {
                Ok(HeartbeatStatus::Stale {
                    seconds_since: elapsed,
                })
            } else {
                Ok(HeartbeatStatus::Alive {
                    last_epoch: progress.heartbeat_epoch,
                    phase: progress.phase,
                })
            }
        }
    }
}

/// Kill a subordinate by sending SIGTERM (Unix) or terminating the process.
///
/// Marks the handle as killed and sends a termination signal to the real
/// child process. On Unix, this sends SIGTERM via `kill(2)`. The handle
/// is mutated in place so the supervisor can track that it was explicitly
/// terminated.
pub fn kill_subordinate(handle: &mut SubordinateHandle) -> SimardResult<()> {
    if handle.killed {
        return Err(SimardError::InvalidIdentityComposition {
            identity: handle.agent_name.clone(),
            reason: "subordinate is already killed".to_string(),
        });
    }

    // Send SIGTERM to the real child process (pid > 0).
    if handle.pid > 0 {
        #[cfg(unix)]
        {
            // SAFETY: kill(2) is safe to call with a valid PID and signal.
            let ret = unsafe { libc::kill(handle.pid as libc::pid_t, libc::SIGTERM) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                // ESRCH means the process already exited — that's fine.
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(SimardError::ActionExecutionFailed {
                        action: format!("kill subordinate '{}'", handle.agent_name),
                        reason: format!("SIGTERM to pid {} failed: {err}", handle.pid),
                    });
                }
            }
        }
    }

    handle.killed = true;
    Ok(())
}

/// Determine whether a subordinate's progress indicates completion.
pub fn is_goal_complete(progress: &SubordinateProgress) -> bool {
    progress.outcome.is_some()
}

/// Get the current unix epoch in seconds.
pub(super) fn current_epoch_seconds() -> SimardResult<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
        SimardError::ClockBeforeUnixEpoch {
            reason: e.to_string(),
        }
    })?;
    Ok(duration.as_secs())
}
