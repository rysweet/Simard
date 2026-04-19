//! Subordinate spawning, heartbeat checking, and termination.

use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_goal_assignment::{SubordinateProgress, poll_progress};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::STALE_THRESHOLD_SECONDS;
use super::tmux::build_tmux_wrapped_command;
use super::types::{HeartbeatStatus, SubordinateConfig, SubordinateHandle};
use crate::subagent_sessions::{session_name_for, state_root as supervisor_state_root};

/// Open (or create+append) the per-agent stdio log file at
/// `<state_root>/agent_logs/<agent_name>.log` and return a clone-pair for
/// stdout/stderr. Returns `None` on any I/O error so callers can fail-open
/// (inherit stdio) rather than blocking spawn.
fn open_agent_log(agent_name: &str) -> Option<(Stdio, Stdio)> {
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

/// Spawn a subordinate agent as a real child process.
///
/// Forks a new Simard process via `Command::new(current_exe())` in the
/// given worktree, passing `--agent-name`, `--goal`, and `--depth` as
/// arguments. The child process inherits the parent's environment.
///
/// stdout and stderr are redirected to
/// `<state_root>/agent_logs/<agent_name>.log` (append mode) so the
/// dashboard's `/ws/agent_log/{agent_name}` endpoint can tail the live
/// output. If the log file cannot be opened the spawn proceeds with
/// inherited stdio (fail-open, see `open_agent_log`).
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

    let mut cmd = Command::new(&exe);
    cmd.arg("engineer")
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
        .current_dir(&config.worktree_path);

    if let Some((out, err)) = open_agent_log(&config.agent_name) {
        cmd.stdout(out).stderr(err);
    }

    // --- WS-2: Wrap inner command in a detached tmux session when tmux is
    //     available, so the dashboard can offer `tmux attach` deep-links.
    //     If tmux is not on PATH, fall back to direct exec (preserves the
    //     pre-WS-2 behavior).
    let session_name = session_name_for(&config.agent_name);
    let log_path = supervisor_state_root()
        .join("agent_logs")
        .join(format!("{}.log", config.agent_name));

    let (child_pid, applied_session_name) = if tmux_is_available() {
        spawn_via_tmux(&exe, config, &session_name, &log_path)?
    } else {
        tracing::warn!(
            target: "simard::supervisor",
            agent = %config.agent_name,
            "tmux not available; spawning subordinate directly (no attach support)",
        );
        let child = cmd.spawn().map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "subordinate".to_string(),
            reason: format!(
                "failed to spawn subordinate '{}' at '{}': {e}",
                config.agent_name,
                exe.display()
            ),
        })?;
        (child.id(), String::new())
    };

    Ok(SubordinateHandle {
        pid: child_pid,
        agent_name: config.agent_name.clone(),
        goal: config.goal.clone(),
        worktree_path: config.worktree_path.clone(),
        spawn_time: now,
        retry_count: 0,
        killed: false,
        session_name: applied_session_name,
    })
}

/// Returns true when the `tmux` binary is available and reports a version.
///
/// The result is cached for the lifetime of the process via `OnceLock`:
/// tmux availability does not change at runtime, and `spawn_subordinate`
/// is on the hot path for engineer dispatch — re-forking `tmux -V` on
/// every spawn would be wasted work.
fn tmux_is_available() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Spawn the subordinate inside a detached tmux session. Returns the
/// engineer pane pid (or 0 if it could not be queried) and the session name.
fn spawn_via_tmux(
    exe: &std::path::Path,
    config: &SubordinateConfig,
    session_name: &str,
    log_path: &std::path::Path,
) -> SimardResult<(u32, String)> {
    // Inner argv must mirror the direct-exec path in `spawn_subordinate`.
    let inner_argv: Vec<String> = vec![
        exe.to_string_lossy().into_owned(),
        "engineer".to_string(),
        "run".to_string(),
        "single-process".to_string(),
        config.worktree_path.to_string_lossy().into_owned(),
        config.goal.clone(),
    ];
    let argv = build_tmux_wrapped_command(session_name, &inner_argv, log_path);

    // `tmux new-session -d` returns immediately after the session is
    // created; the inner shell runs detached inside.
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .env("SIMARD_AGENT_NAME", &config.agent_name)
        .env(
            "SIMARD_SUBORDINATE_DEPTH",
            (config.current_depth + 1).to_string(),
        )
        .env("CARGO_BUILD_JOBS", "4")
        .current_dir(&config.worktree_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "subordinate".to_string(),
            reason: format!(
                "failed to spawn tmux-wrapped subordinate '{}': {e}",
                config.agent_name
            ),
        })?;

    if !status.success() {
        return Err(SimardError::BridgeSpawnFailed {
            bridge: "subordinate".to_string(),
            reason: format!(
                "tmux new-session for subordinate '{}' exited with {status}",
                config.agent_name
            ),
        });
    }

    // Query the engineer pid via the pane's pane_pid (with a brief retry).
    let pid = query_pane_pid(session_name).unwrap_or(0);
    Ok((pid, session_name.to_string()))
}

/// Query the pane_pid (the engineer process) for a tmux session. Retries once
/// after 100ms because `tmux new-session -d` returns before the inner shell
/// has finished forking. Returns None if the session isn't queryable.
fn query_pane_pid(session_name: &str) -> Option<u32> {
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
            session_name: String::new(),
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
            session_name: String::new(),
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
            session_name: String::new(),
        };
        let (commits, prs) = validate_subordinate_artifacts(&handle);
        assert_eq!(commits, 0);
        assert_eq!(prs, 0);
    }
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

#[cfg(all(test, unix))]
mod reaper_tests {
    use super::reap_zombies;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Drain any pre-existing zombies left behind by other tests in the same
    /// process so each test starts from a clean baseline.
    fn drain() {
        for _ in 0..32 {
            if reap_zombies() == 0 {
                break;
            }
        }
    }

    /// Spawn `/bin/true`, drop its `Child` handle without `wait()`, and wait
    /// (up to ~2s) for the kernel to mark the process as exited. Returns when
    /// the child has had time to become a zombie.
    fn spawn_short_lived_unwaited() {
        let child = Command::new("true")
            .spawn()
            .expect("spawn /bin/true should succeed on unix");
        // Intentionally drop without wait() — this is the bug pattern the
        // reaper must clean up.
        drop(child);
        // Give the kernel a moment to transition the child to <defunct>.
        thread::sleep(Duration::from_millis(150));
    }

    #[test]
    fn reaps_dropped_child_within_one_cycle() {
        drain();
        spawn_short_lived_unwaited();

        // Poll briefly to tolerate slow CI scheduling — but the contract is
        // "reaped within one OODA cycle", so a single call should typically
        // suffice. Bound the wait to 2s.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut total = 0usize;
        loop {
            total += reap_zombies();
            if total >= 1 || Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            total >= 1,
            "reap_zombies() must reap the dropped child within one cycle (got {total})",
        );
    }

    #[test]
    fn idempotent_when_no_zombies() {
        drain();
        // With no unwaited children, two consecutive calls must both return 0.
        let first = reap_zombies();
        let second = reap_zombies();
        assert_eq!(first, 0, "expected 0 reaps on quiescent process, got {first}");
        assert_eq!(
            second, 0,
            "second call must also return 0 (idempotent), got {second}",
        );
    }

    #[test]
    fn never_blocks_when_live_child_exists() {
        drain();
        // Spawn a child that lives longer than the call to reap_zombies.
        // WNOHANG must guarantee non-blocking behaviour even when a child
        // exists but has not exited.
        let mut child = Command::new("sleep")
            .arg("2")
            .spawn()
            .expect("spawn /bin/sleep should succeed on unix");

        let start = Instant::now();
        let _ = reap_zombies();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(500),
            "reap_zombies() must not block on live children (took {elapsed:?})",
        );

        // Cleanup: kill and wait the live child so it doesn't leak into other
        // tests in the same process.
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(all(test, not(unix)))]
mod reaper_stub_tests {
    use super::reap_zombies;

    #[test]
    fn stub_returns_zero() {
        assert_eq!(reap_zombies(), 0);
    }
}
