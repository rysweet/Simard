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

/// Check the worktree for commits produced by a subordinate since spawn time.
///
/// Returns the number of commits found after `since_epoch` on the current
/// branch in the subordinate's worktree.
pub fn count_commits_since(
    worktree_path: &std::path::Path,
    since_epoch: u64,
) -> u32 {
    let since_str = format!("@{{{since_epoch}}}");
    let output = Command::new("git")
        .args(["log", "--oneline", "--after", &since_str])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines().filter(|l| !l.trim().is_empty()).count() as u32
        }
        _ => 0,
    }
}

/// Check if any open PRs exist from the subordinate's branch.
///
/// Returns the number of open PRs found from the current branch in the
/// subordinate's worktree.
pub fn count_open_prs(worktree_path: &std::path::Path) -> u32 {
    let branch_output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(worktree_path)
        .output();

    let branch = match branch_output {
        Ok(o) if o.status.success() => {
            let b = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if b.is_empty() { return 0; }
            b
        }
        _ => return 0,
    };

    let pr_output = Command::new("gh")
        .args(["pr", "list", "--head", &branch, "--state", "open", "--json", "number"])
        .current_dir(worktree_path)
        .output();

    match pr_output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            // Count JSON array entries — each `"number":` is one PR.
            text.matches("\"number\"").count() as u32
        }
        _ => 0,
    }
}

/// Validate that a subordinate produced output artifacts (commits or PRs).
///
/// Logs clear warnings when a subordinate exits without producing any
/// artifacts. Returns `(commits, prs)` counts.
pub fn validate_subordinate_artifacts(
    handle: &SubordinateHandle,
) -> (u32, u32) {
    let commits = count_commits_since(&handle.worktree_path, handle.spawn_time);
    let prs = count_open_prs(&handle.worktree_path);

    if commits == 0 && prs == 0 {
        eprintln!(
            "[simard] WARNING: subordinate '{}' (pid={}) exited with no commits and no PRs \
             — goal '{}' produced no output artifacts",
            handle.agent_name, handle.pid, handle.goal,
        );
    } else {
        eprintln!(
            "[simard] subordinate '{}' artifact check: {} commit(s), {} PR(s)",
            handle.agent_name, commits, prs,
        );
    }

    (commits, prs)
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
            commits_produced: 0,
            prs_produced: 0,
            exit_status: None,
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
            commits_produced: 0,
            prs_produced: 0,
            exit_status: None,
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

    // -- has_artifacts --

    #[test]
    fn has_artifacts_true_with_commits() {
        let p = SubordinateProgress {
            sub_id: "a".to_string(),
            phase: "done".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "committed".to_string(),
            heartbeat_epoch: 0,
            outcome: Some("success".to_string()),
            commits_produced: 3,
            prs_produced: 0,
            exit_status: Some(0),
        };
        assert!(p.has_artifacts());
    }

    #[test]
    fn has_artifacts_true_with_prs() {
        let p = SubordinateProgress {
            sub_id: "b".to_string(),
            phase: "done".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "pr created".to_string(),
            heartbeat_epoch: 0,
            outcome: Some("success".to_string()),
            commits_produced: 0,
            prs_produced: 1,
            exit_status: Some(0),
        };
        assert!(p.has_artifacts());
    }

    #[test]
    fn has_artifacts_false_when_empty() {
        let p = SubordinateProgress {
            sub_id: "c".to_string(),
            phase: "done".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "exited".to_string(),
            heartbeat_epoch: 0,
            outcome: Some("success".to_string()),
            commits_produced: 0,
            prs_produced: 0,
            exit_status: Some(0),
        };
        assert!(!p.has_artifacts());
    }

    // -- with_artifacts / with_exit_status --

    #[test]
    fn with_artifacts_sets_counts() {
        let p = SubordinateProgress {
            sub_id: "d".to_string(),
            phase: "done".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "done".to_string(),
            heartbeat_epoch: 0,
            outcome: None,
            commits_produced: 0,
            prs_produced: 0,
            exit_status: None,
        };
        let p2 = p.with_artifacts(5, 2);
        assert_eq!(p2.commits_produced, 5);
        assert_eq!(p2.prs_produced, 2);
    }

    #[test]
    fn with_exit_status_sets_code() {
        let p = SubordinateProgress {
            sub_id: "e".to_string(),
            phase: "done".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "done".to_string(),
            heartbeat_epoch: 0,
            outcome: None,
            commits_produced: 0,
            prs_produced: 0,
            exit_status: None,
        };
        let p2 = p.with_exit_status(42);
        assert_eq!(p2.exit_status, Some(42));
    }

    // -- validate_subordinate_artifacts --

    #[test]
    fn validate_artifacts_returns_zero_for_nonexistent_path() {
        let handle = SubordinateHandle {
            pid: 0,
            agent_name: "test".to_string(),
            goal: "goal".to_string(),
            worktree_path: std::path::PathBuf::from("/nonexistent/path/12345"),
            spawn_time: 0,
            retry_count: 0,
            killed: false,
        };
        let (commits, prs) = validate_subordinate_artifacts(&handle);
        assert_eq!(commits, 0);
        assert_eq!(prs, 0);
    }
}
