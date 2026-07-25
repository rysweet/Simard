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
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<HeartbeatStatus> {
    if handle.killed {
        return Ok(HeartbeatStatus::Dead);
    }

    let progress = poll_progress(&handle.agent_name, memory)?;

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

/// The outcome of a subordinate kill attempt (issue #4243). Distinguishes a
/// verified signal from a refusal to signal a recycled PID and from a no-op on a
/// handle that has no real process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillAction {
    /// A termination signal was sent to the (identity-verified) subordinate PID.
    Signalled,
    /// The live tmux pane PID did not match the cached PID: the OS recycled the
    /// PID to an unrelated process, so the reaper REFUSED to signal it.
    RefusedPidReuse,
    /// No real process to signal (mock/test handle with `pid == 0`).
    SkippedNoPid,
}

/// Kill a subordinate by sending SIGTERM (Unix) or terminating the process.
///
/// Marks the handle as killed and sends a termination signal to the real
/// child process. When the subordinate runs under a tmux session, the live pane
/// PID is cross-checked against the cached PID first (issue #4243) so a recycled
/// PID is never signalled. The handle is mutated in place so the supervisor can
/// track that it was explicitly terminated.
pub fn kill_subordinate(handle: &mut SubordinateHandle) -> SimardResult<()> {
    // Verify the cached PID against the live tmux pane before signalling. An
    // empty session name (no tmux) or a failed query yields `None`, which the
    // callee treats as "unverifiable" and falls back to today's behaviour.
    let live_pane_pid = if handle.session_name.is_empty() {
        None
    } else {
        query_pane_pid(&handle.session_name)
    };
    kill_subordinate_with_pane_pid(handle, live_pane_pid).map(|_| ())
}

/// PID-reuse-safe core of [`kill_subordinate`] (issue #4243).
///
/// `live_pane_pid` is the PID currently owning the subordinate's tmux pane (or
/// `None` when tmux is unavailable / the session is gone). Behaviour:
///
/// * `Some(p)` and `handle.pid > 0` and `p != handle.pid` → the cached PID was
///   recycled to an unrelated process: REFUSE to signal ([`KillAction::RefusedPidReuse`]).
/// * `Some(p)` and `p == handle.pid > 0` → identity verified: SIGTERM the PID
///   ([`KillAction::Signalled`]).
/// * `None` → identity unverifiable: fall back to signalling when `pid > 0`
///   ([`KillAction::Signalled`]) so normal teardown is never regressed.
/// * `handle.pid == 0` → mock handle: never signals ([`KillAction::SkippedNoPid`]).
///
/// In every non-error branch the handle is marked killed. An already-killed
/// handle is an error (unchanged contract).
pub fn kill_subordinate_with_pane_pid(
    handle: &mut SubordinateHandle,
    live_pane_pid: Option<u32>,
) -> SimardResult<KillAction> {
    if handle.killed {
        return Err(SimardError::InvalidIdentityComposition {
            identity: handle.agent_name.clone(),
            reason: "subordinate is already killed".to_string(),
        });
    }

    // Mock/test handle: nothing real to signal.
    if handle.pid == 0 {
        handle.killed = true;
        return Ok(KillAction::SkippedNoPid);
    }

    // PID-reuse guard: if the live pane reports a DIFFERENT pid, the cached pid
    // was recycled by the OS — refuse to signal the innocent process.
    if let Some(pane_pid) = live_pane_pid
        && pane_pid != handle.pid
    {
        tracing::warn!(
            target: "simard::supervisor",
            agent = %handle.agent_name,
            session = %handle.session_name,
            cached_pid = handle.pid,
            live_pane_pid = pane_pid,
            "refusing to reap subordinate: cached PID was recycled (pane pid mismatch)"
        );
        handle.killed = true;
        return Ok(KillAction::RefusedPidReuse);
    }

    // Either the identity is verified (pane pid == cached pid) or unverifiable
    // (no pane pid); in both cases signal the real child process.
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

    handle.killed = true;
    Ok(KillAction::Signalled)
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
// are dropped without `wait()`, and dispatches a detached `simard safe-update`
// child the same way. Without intervention the kernel keeps those exited
// children as `<defunct>` entries indefinitely. `reap_zombies` is invoked once
// per OODA cycle to harvest exit statuses non-blockingly.
//
// It reaps **only** PIDs explicitly registered via
// [`crate::util::spawn_retry::register_reapable_child`] (the detached-child
// registry), using `waitpid(pid, WNOHANG)` per PID. It never calls
// `waitpid(-1)`, so it can never steal a child that another owner (a `git`,
// `gh`, or Bash-tool spawn+wait span) is itself waiting on — which would
// otherwise fail that owner's wait with `ECHILD`.

/// Non-blockingly reap any exited, registered detached child processes.
///
/// Returns the number of children reaped during this call. On non-Unix
/// platforms this is a no-op that always returns `0`. See
/// [`crate::util::spawn_retry::reap_registered_children`] for the mechanism.
pub fn reap_zombies() -> usize {
    crate::util::spawn_retry::reap_registered_children()
}

// ---------------------------------------------------------------------------
// Zombie reaper tests (TDD: these tests describe the contract for
// `reap_zombies`, which prevents <defunct> child accumulation in the
// long-running OODA daemon).
// ---------------------------------------------------------------------------

mod spawn;
pub use spawn::spawn_subordinate;
