use super::lifecycle::*;
use crate::agent_goal_assignment::SubordinateProgress;

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

#[cfg(all(test, unix))]
mod reaper_tests {
    use super::reap_zombies;
    use serial_test::serial;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    // `reap_zombies()` harvests only PIDs explicitly registered via
    // `crate::util::spawn_retry::register_reapable_child` (the detached-child
    // registry). It no longer calls `waitpid(-1)`, so it can never steal a
    // child that another test (or production spawn+wait span) is itself waiting
    // on — the `ECHILD` race that previously reddened the canary (issue #1779)
    // is gone by construction.
    //
    // These reaper tests still share the single process-global registry and
    // each `reap_zombies()` call drains every registered PID, so they remain
    // serialized against one another through the `simard_process_reaper` group
    // to keep their per-test PID accounting deterministic. No *other* test in
    // the suite registers a PID (the detached-spawn sites are production-only
    // paths), so no cross-test contamination occurs.

    /// Drain any pre-existing registered zombies so each test starts clean.
    fn drain() {
        for _ in 0..32 {
            if reap_zombies() == 0 {
                break;
            }
        }
    }

    /// Spawn `/bin/true`, register its PID in the detached-child registry, drop
    /// its `Child` handle without `wait()`, and wait (up to ~2s) for the kernel
    /// to mark the process as exited. Returns when the child has had time to
    /// become a zombie that the targeted reaper can harvest.
    fn spawn_short_lived_unwaited() {
        let child = crate::util::spawn_retry::retry_spawn_sync(|| Command::new("true").spawn())
            .expect("spawn /bin/true should succeed on unix");
        // Register the detached PID, then drop the handle without wait() — this
        // is the fire-and-forget pattern the targeted reaper must clean up.
        crate::util::spawn_retry::register_reapable_child(child.id());
        drop(child);
        // Give the kernel a moment to transition the child to <defunct>.
        thread::sleep(Duration::from_millis(150));
    }

    #[test]
    #[serial(simard_process_reaper)]
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
    #[serial(simard_process_reaper)]
    fn idempotent_when_no_zombies() {
        drain();
        // With no unwaited children, two consecutive calls must both return 0.
        let first = reap_zombies();
        let second = reap_zombies();
        assert_eq!(
            first, 0,
            "expected 0 reaps on quiescent process, got {first}"
        );
        assert_eq!(
            second, 0,
            "second call must also return 0 (idempotent), got {second}",
        );
    }

    #[test]
    #[serial(simard_process_reaper)]
    fn never_blocks_when_live_child_exists() {
        drain();
        // Spawn a child that lives longer than the call to reap_zombies.
        // WNOHANG must guarantee non-blocking behaviour even when a child
        // exists but has not exited.
        let mut child =
            crate::util::spawn_retry::retry_spawn_sync(|| Command::new("sleep").arg("2").spawn())
                .expect("spawn /bin/sleep should succeed on unix");
        // Register so the targeted reaper actually queries this PID; a live
        // child must return WNOHANG=0 immediately (non-blocking).
        crate::util::spawn_retry::register_reapable_child(child.id());

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

// ─────────────────────────────────────────────────────────────────────────────
// TDD (issue #4243): PID-REUSE-SAFE subordinate reaping.
//
// `kill_subordinate` sends SIGTERM to the cached `handle.pid`. If the OS has
// recycled that PID to an unrelated process, the reaper can kill the wrong
// process. Fix: before signalling, cross-check the live tmux pane PID against
// the cached PID and REFUSE to signal on mismatch.
//
// Fix contract (additive; `kill_subordinate` keeps its public `-> ()` shape and
// becomes a thin wrapper that looks up the pane PID for non-empty
// `session_name` and delegates here):
//
//   pub enum KillAction { Signalled, RefusedPidReuse, SkippedNoPid }
//   pub fn kill_subordinate_with_pane_pid(
//       handle: &mut SubordinateHandle,
//       live_pane_pid: Option<u32>,
//   ) -> SimardResult<KillAction>
//
//   * live_pane_pid == Some(p), handle.pid > 0, p != handle.pid
//         -> RefusedPidReuse: DO NOT signal; mark handle.killed = true; Ok.
//   * live_pane_pid == Some(p), p == handle.pid, handle.pid > 0
//         -> Signalled: SIGTERM the (verified) pid; mark killed = true.
//   * live_pane_pid == None (empty session_name or tmux query failed)
//         -> fall back to today's behaviour: signal when pid > 0 (Signalled),
//            otherwise SkippedNoPid; mark killed = true. Liveness > false refusal.
//   * handle.pid == 0 (mock/test handle) -> SkippedNoPid; never signals.
//   * already killed -> Err (unchanged).
//
// These tests are written FIRST and MUST FAIL until `kill_subordinate_with_pane_pid`
// and `KillAction` exist.

fn handle_with(pid: u32, session_name: &str) -> SubordinateHandle {
    SubordinateHandle {
        pid,
        agent_name: "test-agent".to_string(),
        goal: "test".to_string(),
        worktree_path: std::path::PathBuf::from("/fake"),
        spawn_time: 0,
        retry_count: 0,
        killed: false,
        session_name: session_name.to_string(),
    }
}

/// A stale/reused cached PID (the pane reports a DIFFERENT live PID) must NOT be
/// signalled — the reaper refuses and leaves the innocent process untouched.
#[cfg(unix)]
#[test]
#[serial_test::serial(simard_process_reaper)]
fn kill_refuses_and_spares_reused_pid() {
    // An innocent long-lived process now owns the recycled PID.
    let mut innocent = crate::util::spawn_retry::retry_spawn_sync(|| {
        std::process::Command::new("sleep").arg("30").spawn()
    })
    .expect("spawn innocent sleep child");
    let innocent_pid = innocent.id();

    let mut handle = handle_with(innocent_pid, "ooda-session-x");
    // The live pane reports a pid that does NOT match the cached (recycled) pid.
    let live_pane_pid = Some(innocent_pid.wrapping_add(1));

    let action = kill_subordinate_with_pane_pid(&mut handle, live_pane_pid)
        .expect("mismatch is not an error — it is a safe refusal");
    assert_eq!(action, KillAction::RefusedPidReuse);
    assert!(
        handle.killed,
        "mismatch branch still marks the handle killed"
    );

    // Give any (erroneous) signal a moment, then prove the innocent survived.
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(
        matches!(innocent.try_wait(), Ok(None)),
        "the reaper must NOT have terminated the innocent reused-PID process"
    );

    let _ = innocent.kill();
    let _ = innocent.wait();
}

/// When the live pane PID matches the cached PID, the reaper proceeds and
/// signals the (verified) subordinate.
#[cfg(unix)]
#[test]
#[serial_test::serial(simard_process_reaper)]
fn kill_signals_matching_pane_pid() {
    let mut child = crate::util::spawn_retry::retry_spawn_sync(|| {
        std::process::Command::new("sleep").arg("30").spawn()
    })
    .expect("spawn subordinate sleep child");
    let pid = child.id();

    let mut handle = handle_with(pid, "ooda-session-y");
    let action = kill_subordinate_with_pane_pid(&mut handle, Some(pid))
        .expect("verified identity -> signal");
    assert_eq!(action, KillAction::Signalled);
    assert!(handle.killed);

    // The verified child must actually receive the termination signal.
    let mut terminated = false;
    for _ in 0..50 {
        match child.try_wait() {
            Ok(Some(_)) => {
                terminated = true;
                break;
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    assert!(
        terminated,
        "matching-identity kill must terminate the subordinate"
    );
    let _ = child.wait();
}

/// When identity cannot be verified (no pane pid — tmux unavailable/failed or
/// no session), the reaper falls back to today's behaviour rather than
/// refusing, so normal teardown is never regressed.
#[test]
fn kill_falls_back_when_pane_unverifiable() {
    // pid == 0 is a mock handle: never signals, but must still be marked killed.
    let mut handle = handle_with(0, "");
    let action = kill_subordinate_with_pane_pid(&mut handle, None)
        .expect("unverifiable identity falls back, does not error");
    assert_eq!(action, KillAction::SkippedNoPid);
    assert!(handle.killed);
}

/// The additive guard must not change the public `kill_subordinate` contract:
/// an empty-session, mock handle still succeeds and is marked killed.
#[test]
fn kill_subordinate_public_wrapper_preserves_behaviour() {
    let mut handle = handle_with(0, "");
    assert!(kill_subordinate(&mut handle).is_ok());
    assert!(handle.killed);
}
