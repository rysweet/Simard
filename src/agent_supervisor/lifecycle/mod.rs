//! Subordinate spawning, heartbeat checking, and termination.

use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_goal_assignment::{SubordinateProgress, poll_progress};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::STALE_THRESHOLD_SECONDS;
use super::types::{HeartbeatStatus, SubordinateHandle};

/// Resolve the Simard state root the same way the dashboard does.
///
/// Duplicated locally to avoid a cross-module dependency on the dashboard
/// crate; both implementations honor `SIMARD_STATE_ROOT` then fall back to
/// `$HOME/.simard`.
pub(super) fn supervisor_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}

/// Open (or create+append) the per-agent stdio log file at
/// `<state_root>/agent_logs/<agent_name>.log` and return a clone-pair for
/// stdout/stderr. Returns `None` on any I/O error so callers can fail-open
/// (inherit stdio) rather than blocking spawn.
pub(super) fn open_agent_log(agent_name: &str) -> Option<(Stdio, Stdio)> {
    use std::fs::{OpenOptions, create_dir_all};
    let dir = supervisor_state_root().join("agent_logs");
    if let Err(e) = create_dir_all(&dir) {
        tracing::warn!(target: "simard::supervisor", agent = %agent_name, error = %e, "failed to create agent_logs dir; falling back to inherited stdio");
        return None;
    }
    let path = dir.join(format!("{agent_name}.log"));
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(target: "simard::supervisor", agent = %agent_name, path = %path.display(), error = %e, "failed to open agent log; falling back to inherited stdio");
            return None;
        }
    };
    let cloned = match file.try_clone() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(target: "simard::supervisor", agent = %agent_name, error = %e, "failed to clone agent log fd; falling back to inherited stdio");
            return None;
        }
    };
    Some((Stdio::from(file), Stdio::from(cloned)))
}

pub(super) fn query_pane_pid(session_name: &str) -> Option<u32> {
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let out = Command::new("tmux")
            .args(["list-panes", "-t", session_name, "-F", "#{pane_pid}"])
            .output()
            .ok()?;
        if !out.status.success() {
            continue;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = s.lines().next()
            && let Ok(pid) = line.trim().parse::<u32>()
        {
            return Some(pid);
        }
    }
    None
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
pub fn count_commits_since(worktree_path: &std::path::Path, since_epoch: u64) -> u32 {
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
            if b.is_empty() {
                return 0;
            }
            b
        }
        _ => return 0,
    };

    let pr_output = Command::new("gh")
        .args([
            "pr", "list", "--head", &branch, "--state", "open", "--json", "number",
        ])
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
pub fn validate_subordinate_artifacts(handle: &SubordinateHandle) -> (u32, u32) {
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

// ---------------------------------------------------------------------------
// Zombie reaper
// ---------------------------------------------------------------------------
//
// The OODA daemon spawns subordinate engineer processes whose `Child` handles
// are dropped without `wait()`. Without intervention, the kernel keeps those
// exited children as `<defunct>` entries indefinitely. `reap_zombies` is
// invoked once per OODA cycle to harvest exit statuses non-blockingly via
// `waitpid(-1, ..., WNOHANG)`.

/// Non-blockingly reap any exited child processes of the calling process.
///
/// Returns the number of children reaped during this call. On non-Unix
/// platforms this is a no-op that always returns `0`.
///
/// EINTR handling: the loop terminates on any `-1` return regardless of
/// `errno`. A missed reap on signal interruption is harmless because the
/// next OODA cycle (typically seconds later) will pick it up; retrying
/// inside the loop risks unbounded iteration.
#[cfg(unix)]
pub fn reap_zombies() -> usize {
    let mut reaped: usize = 0;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: `waitpid` with pid = -1 and WNOHANG is a non-blocking call
        // that inspects the kernel's child-process table for the calling
        // process. The `status` pointer points to a stack-allocated c_int we
        // own. No invariants beyond standard POSIX semantics are required.
        let pid = unsafe { libc::waitpid(-1, &mut status as *mut libc::c_int, libc::WNOHANG) };
        if pid > 0 {
            reaped += 1;
            continue;
        }
        // pid == 0: children exist but none have exited.
        // pid == -1: no children remain (ECHILD) or interrupted (EINTR).
        // In all non-positive cases, stop polling this cycle.
        break;
    }
    reaped
}

#[cfg(not(unix))]
pub fn reap_zombies() -> usize {
    0
}

// ---------------------------------------------------------------------------
// Zombie reaper tests (TDD: these tests describe the contract for
// `reap_zombies`, which prevents <defunct> child accumulation in the
// long-running OODA daemon).
// ---------------------------------------------------------------------------

mod spawn;
pub use spawn::spawn_subordinate;
