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
    use std::process::Command;
    use std::sync::{Mutex, MutexGuard};
    use std::thread;
    use std::time::{Duration, Instant};

    // `reap_zombies()` is process-wide: it reaps ANY <defunct> child of this
    // process, regardless of which test spawned it. Without serialization,
    // peer tests in this module race with each other — e.g. one test's
    // `drain()` can steal another test's zombie before the assertion runs,
    // producing flaky "got 0" failures in CI. Serialize all reaper tests
    // through this module-local mutex so each test owns the process state
    // for its duration.
    static REAPER_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_reaper() -> MutexGuard<'static, ()> {
        // If a previous test panicked while holding the lock the mutex is
        // poisoned — that's still safe to use here because the protected
        // state is process-global and we always re-drain at the start of
        // each test.
        REAPER_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
        let _guard = lock_reaper();
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
        let _guard = lock_reaper();
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
    fn never_blocks_when_live_child_exists() {
        let _guard = lock_reaper();
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
