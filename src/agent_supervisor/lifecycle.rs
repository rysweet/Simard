//! Subordinate spawning, heartbeat checking, and termination.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_goal_assignment::{SubordinateProgress, poll_progress};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

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
#[tracing::instrument(skip_all, fields(identity = %config.agent_name))]
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
        .arg("single-process")
        .arg(&config.worktree_path)
        .arg(&config.goal)
        .env("SIMARD_AGENT_NAME", &config.agent_name)
        .env(
            "SIMARD_SUBORDINATE_DEPTH",
            (config.current_depth + 1).to_string(),
        )
        // Limit concurrent cargo parallelism per agent to prevent OOM (issue #373).
        .env("CARGO_BUILD_JOBS", "4")
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
    bridge: &dyn CognitiveMemoryOps,
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- current_epoch_seconds --

    #[test]
    fn current_epoch_seconds_returns_reasonable_value() {
        let now = current_epoch_seconds().unwrap();
        // Should be after 2020-01-01 (epoch 1577836800).
        assert!(now > 1_577_836_800, "epoch {now} seems too small");
    }

    // -- is_goal_complete --

    #[test]
    fn is_goal_complete_true_when_outcome_present() {
        let progress = SubordinateProgress {
            sub_id: "test".to_string(),
            phase: "done".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "finished".to_string(),
            heartbeat_epoch: 0,
            outcome: Some("success".to_string()),
        };
        assert!(is_goal_complete(&progress));
    }

    #[test]
    fn is_goal_complete_false_when_no_outcome() {
        let progress = SubordinateProgress {
            sub_id: "test".to_string(),
            phase: "working".to_string(),
            steps_completed: 0,
            steps_total: 5,
            last_action: "coding".to_string(),
            heartbeat_epoch: 0,
            outcome: None,
        };
        assert!(!is_goal_complete(&progress));
    }

    // -- kill_subordinate --

    #[test]
    fn kill_subordinate_marks_handle_killed() {
        let mut handle = SubordinateHandle {
            pid: 0,
            agent_name: "test-agent".to_string(),
            goal: "test".to_string(),
            worktree_path: std::path::PathBuf::from("/fake"),
            spawn_time: 0,
            retry_count: 0,
            killed: false,
        };
        // pid=0 means we won't actually send a signal to a real process.
        let result = kill_subordinate(&mut handle);
        assert!(result.is_ok());
        assert!(handle.killed);
    }

    #[test]
    fn kill_subordinate_errors_when_already_killed() {
        let mut handle = SubordinateHandle {
            pid: 0,
            agent_name: "test-agent".to_string(),
            goal: "test".to_string(),
            worktree_path: std::path::PathBuf::from("/fake"),
            spawn_time: 0,
            retry_count: 0,
            killed: true,
        };
        let result = kill_subordinate(&mut handle);
        assert!(result.is_err());
    }
}
