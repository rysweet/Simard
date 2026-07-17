//! Phase 1: drain in-flight engineer dispatches before a self-update.
//!
//! The orchestrator writes `state_dir/draining.flag` so the brain's dispatch
//! site refuses to spawn **new** engineers (the wired check lives in
//! `src/engineer_loop/agent_spawn.rs`, via [`super::state::is_draining`]).
//!
//! ## Drain doctrine (never kill, never abort on a timeout)
//!
//! The drain **never kills a producing engineer** and **never aborts a deploy
//! on a wall-clock timeout**. Because Simard almost always has long-running
//! engineers in flight, a timeout-and-fail drain meant deploys essentially
//! never succeeded while busy, leaving her many commits behind her own merged
//! improvements. Instead the load-bearing self-deploy path
//! ([`drain_by_requeue`]) marks draining and then **gracefully checkpoints and
//! requeues** each in-flight engineer's goal back onto the board. Their goal
//! state already persists, so once the daemon restarts the new binary
//! re-picks-up the requeued goals. rename-based atomic swap is safe against a
//! still-running executable, so no engineer has to be killed to swap the
//! binary.
//!
//! The legacy [`drain_to_quiescence_with_root`] path (used by the download-
//! based `safe_update` orchestrator, which has no goal board wired) keeps a
//! best-effort grace window so engineers that are *naturally* finishing can
//! wrap up — but it too returns success when the grace elapses rather than
//! failing the update.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::errors::SafeUpdateError;
use super::state::draining_flag_path;

/// Result of the drain phase.
#[derive(Debug, Clone)]
pub struct DrainOutcome {
    /// In-flight count observed when the drain phase began.
    pub in_flight_at_start: usize,
    /// In-flight count observed after the drain phase ended. With
    /// [`drain_by_requeue`] this is the number that could not be requeued (0
    /// on the happy path); with [`drain_to_quiescence_with_root`] it is the
    /// number still in flight after the best-effort grace window.
    pub in_flight_at_end: usize,
    /// Number of in-flight engineers whose goals were checkpointed and requeued
    /// onto the board (0 for the grace-only path). These resume under the
    /// restarted binary rather than being lost.
    pub requeued: usize,
    pub elapsed: Duration,
}

/// One engineer observed in flight at drain time.
///
/// Injected into [`drain_by_requeue`] by an [`EngineerRequeue`] implementation
/// so `safe_update` stays decoupled from the goal board and the on-disk
/// worktree layout (whose production wiring lives in `self_deploy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightEngineer {
    /// Goal id recovered from the engineer's worktree.
    pub goal_id: String,
    /// Absolute path of the engineer's worktree.
    pub worktree: PathBuf,
    /// PID recorded in the worktree claim sentinel, if any.
    pub pid: Option<i32>,
}

/// Effect the drain uses to gracefully checkpoint + requeue in-flight engineers
/// instead of waiting for them or killing them.
///
/// Implementations MUST NOT kill or signal any process: a producing engineer is
/// left running (rename-based atomic swap is safe against a running binary) and
/// its goal is simply released back onto the board so a restarted binary
/// re-picks it up. Requeue is best-effort — returning `Ok(())` even when the
/// engineer has already exited is correct.
pub trait EngineerRequeue {
    /// Enumerate the engineers currently in flight.
    fn in_flight(&self) -> Vec<InFlightEngineer>;

    /// Requeue one engineer's goal onto the board so a restarted binary
    /// re-picks it up. MUST NOT kill the process.
    fn requeue(&self, engineer: &InFlightEngineer) -> Result<(), SafeUpdateError>;
}

/// Load-bearing self-deploy drain: mark draining, then checkpoint + requeue
/// every in-flight engineer's goal onto the board. **Never** waits on a
/// wall-clock timeout, **never** fails the deploy because engineers remain, and
/// **never** kills a producing engineer.
///
/// A per-engineer requeue failure is logged and skipped rather than aborting
/// the deploy: the goal record already persists on the board, and the restart
/// makes any still-live claim stale, so the goal is re-picked-up regardless.
pub fn drain_by_requeue<R: EngineerRequeue>(
    state_dir: &Path,
    requeue: &R,
) -> Result<DrainOutcome, SafeUpdateError> {
    let started = Instant::now();
    mark_draining(state_dir)?;

    let engineers = requeue.in_flight();
    let in_flight_at_start = engineers.len();
    let mut requeued = 0_usize;
    for engineer in &engineers {
        match requeue.requeue(engineer) {
            Ok(()) => requeued += 1,
            Err(err) => {
                tracing::warn!(
                    goal_id = %engineer.goal_id,
                    worktree = %engineer.worktree.display(),
                    error = %err,
                    "[self-deploy] requeue failed; continuing deploy (drain never aborts)"
                );
            }
        }
    }

    Ok(DrainOutcome {
        in_flight_at_start,
        in_flight_at_end: in_flight_at_start.saturating_sub(requeued),
        requeued,
        elapsed: started.elapsed(),
    })
}

/// Write `state_dir/draining.flag` so subsequent engineer dispatches refuse.
/// Idempotent; safe to call repeatedly.
pub fn mark_draining(state_dir: &Path) -> Result<(), SafeUpdateError> {
    fs::create_dir_all(state_dir).map_err(|e| SafeUpdateError::DrainIo {
        action: "create state_dir".into(),
        path: state_dir.to_path_buf(),
        reason: e.to_string(),
    })?;
    let path = draining_flag_path(state_dir);
    fs::write(&path, b"").map_err(|e| SafeUpdateError::DrainIo {
        action: "write".into(),
        path,
        reason: e.to_string(),
    })
}

/// Remove `state_dir/draining.flag`. Idempotent (missing is OK).
pub fn unmark_draining(state_dir: &Path) -> Result<(), SafeUpdateError> {
    let path = draining_flag_path(state_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SafeUpdateError::DrainIo {
            action: "remove".into(),
            path,
            reason: e.to_string(),
        }),
    }
}

/// Drive the best-effort grace drain: mark draining, let engineers that are
/// *naturally* finishing wrap up within the grace window, then return.
///
/// Reads the engineer-worktrees root from `$HOME/.simard/engineer-worktrees/`.
/// For tests / non-default installs use [`drain_to_quiescence_with_root`].
///
/// This path has no goal board wired (it backs the download-based
/// `safe_update` orchestrator), so it cannot requeue. It therefore **never
/// fails on a timeout** and **never kills** an engineer — when the grace
/// window elapses with engineers still in flight it returns success, leaving
/// `draining.flag` set so no *new* engineers start. The self-deploy path uses
/// [`drain_by_requeue`] instead, which additionally checkpoints and requeues
/// the in-flight goals.
pub fn drain_to_quiescence(
    state_dir: &Path,
    grace_seconds: u64,
) -> Result<DrainOutcome, SafeUpdateError> {
    drain_to_quiescence_with_root(state_dir, grace_seconds, &engineer_worktrees_root())
}

/// Same as [`drain_to_quiescence`] but with an explicit engineer-worktrees
/// root, so tests don't have to depend on the live `~/.simard/` directory.
///
/// `grace_seconds` is an upper bound on how long to *optionally* wait for
/// engineers to finish on their own; it is **not** a deadline that can fail the
/// drain. When it elapses with engineers still in flight the drain returns
/// `Ok` with `in_flight_at_end` recording how many remain.
pub fn drain_to_quiescence_with_root(
    state_dir: &Path,
    grace_seconds: u64,
    engineer_root: &Path,
) -> Result<DrainOutcome, SafeUpdateError> {
    let started = Instant::now();
    mark_draining(state_dir)?;

    let in_flight_at_start = count_in_flight_engineers_in(engineer_root);
    let deadline = started + Duration::from_secs(grace_seconds);
    let poll_interval = poll_interval_for(grace_seconds);

    loop {
        let in_flight = count_in_flight_engineers_in(engineer_root);
        if in_flight == 0 {
            return Ok(DrainOutcome {
                in_flight_at_start,
                in_flight_at_end: 0,
                requeued: 0,
                elapsed: started.elapsed(),
            });
        }
        if Instant::now() >= deadline {
            // Grace elapsed with engineers still in flight: proceed anyway.
            // We never fail the update and never kill a producing engineer.
            return Ok(DrainOutcome {
                in_flight_at_start,
                in_flight_at_end: in_flight,
                requeued: 0,
                elapsed: started.elapsed(),
            });
        }
        sleep(poll_interval);
    }
}

/// Return the directory the brain monitors for engineer worktrees.
fn engineer_worktrees_root() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home)
            .join(".simard")
            .join("engineer-worktrees")
    } else {
        PathBuf::from(".simard").join("engineer-worktrees")
    }
}

/// Count engineer dispatches that look in-flight under `root`. Best-effort:
/// returns 0 on any I/O error so a missing directory does not block a drain.
pub(crate) fn count_in_flight_engineers_in(root: &Path) -> usize {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut alive = 0_usize;
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        // Each engineer worktree may have a `pid` file written by the spawn
        // helper. If absent, fall back to "directory present == in-flight".
        let pid_file = p.join("pid");
        match fs::read_to_string(&pid_file) {
            Ok(s) => {
                if let Ok(pid) = s.trim().parse::<u32>()
                    && process_alive(pid)
                {
                    alive += 1;
                }
            }
            Err(_) => {
                // No pid file: be conservative — count as alive so we wait.
                alive += 1;
            }
        }
    }
    alive
}

/// Pick a polling interval that scales sensibly with the timeout. Caps at
/// 5s for human-friendly progress and at 100ms for the short timeouts used
/// in tests.
fn poll_interval_for(drain_timeout_seconds: u64) -> Duration {
    if drain_timeout_seconds == 0 {
        Duration::from_millis(50)
    } else if drain_timeout_seconds <= 2 {
        Duration::from_millis(100)
    } else if drain_timeout_seconds <= 30 {
        Duration::from_millis(500)
    } else {
        Duration::from_secs(5)
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}/status")).exists()
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    // Without /proc we cannot make a strong claim; assume alive so the
    // drain waits, which is the safe direction.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn mark_and_unmark_round_trip() {
        let dir = tempdir().unwrap();
        assert!(!draining_flag_path(dir.path()).exists());
        mark_draining(dir.path()).unwrap();
        assert!(draining_flag_path(dir.path()).exists());
        unmark_draining(dir.path()).unwrap();
        assert!(!draining_flag_path(dir.path()).exists());
    }

    #[test]
    fn unmark_is_idempotent_when_missing() {
        let dir = tempdir().unwrap();
        unmark_draining(dir.path()).unwrap();
        unmark_draining(dir.path()).unwrap();
    }

    #[test]
    fn drain_returns_immediately_when_no_engineers_in_flight() {
        // Use an isolated, empty engineer-worktrees root so this test does not
        // depend on the live ~/.simard/ directory.
        let dir = tempdir().unwrap();
        let engineers = tempdir().unwrap();
        let outcome = drain_to_quiescence_with_root(dir.path(), 1, engineers.path()).unwrap();
        assert_eq!(outcome.in_flight_at_end, 0);
        assert!(outcome.elapsed < Duration::from_secs(2));
    }

    #[test]
    fn drain_never_fails_with_a_fake_engineer_still_in_flight() {
        let dir = tempdir().unwrap();
        let engineers = tempdir().unwrap();
        // Fake engineer worktree without a pid file → counts as in-flight.
        std::fs::create_dir_all(engineers.path().join("eng-1")).unwrap();
        // Grace elapses with the engineer still in flight, but we NEVER fail
        // the drain and NEVER kill the engineer — the deploy proceeds.
        let outcome = drain_to_quiescence_with_root(dir.path(), 1, engineers.path()).unwrap();
        assert_eq!(outcome.in_flight_at_start, 1);
        assert_eq!(outcome.in_flight_at_end, 1);
        assert_eq!(outcome.requeued, 0);
        // The fake worktree is untouched (never killed / removed).
        assert!(engineers.path().join("eng-1").exists());
        // Flag remains set so new dispatches stay refused.
        assert!(draining_flag_path(dir.path()).exists());
    }

    /// Fake requeue that records the engineers it was asked to requeue.
    struct FakeRequeue {
        engineers: Vec<InFlightEngineer>,
        requeued: std::cell::RefCell<Vec<String>>,
        fail_goal: Option<String>,
    }

    impl EngineerRequeue for FakeRequeue {
        fn in_flight(&self) -> Vec<InFlightEngineer> {
            self.engineers.clone()
        }
        fn requeue(&self, engineer: &InFlightEngineer) -> Result<(), SafeUpdateError> {
            if self.fail_goal.as_deref() == Some(engineer.goal_id.as_str()) {
                return Err(SafeUpdateError::DrainIo {
                    action: "requeue".into(),
                    path: engineer.worktree.clone(),
                    reason: "boom".into(),
                });
            }
            self.requeued.borrow_mut().push(engineer.goal_id.clone());
            Ok(())
        }
    }

    fn eng(goal: &str) -> InFlightEngineer {
        InFlightEngineer {
            goal_id: goal.to_string(),
            worktree: PathBuf::from(format!("/tmp/{goal}")),
            pid: Some(1234),
        }
    }

    #[test]
    fn drain_by_requeue_marks_draining_and_requeues_all() {
        let dir = tempdir().unwrap();
        let requeue = FakeRequeue {
            engineers: vec![eng("goal-a"), eng("goal-b")],
            requeued: std::cell::RefCell::new(Vec::new()),
            fail_goal: None,
        };
        let outcome = drain_by_requeue(dir.path(), &requeue).unwrap();
        assert_eq!(outcome.in_flight_at_start, 2);
        assert_eq!(outcome.requeued, 2);
        assert_eq!(outcome.in_flight_at_end, 0);
        assert_eq!(&*requeue.requeued.borrow(), &["goal-a", "goal-b"]);
        // Draining flag is set so no NEW engineers start.
        assert!(draining_flag_path(dir.path()).exists());
    }

    #[test]
    fn drain_by_requeue_never_aborts_when_a_requeue_fails() {
        let dir = tempdir().unwrap();
        let requeue = FakeRequeue {
            engineers: vec![eng("goal-a"), eng("goal-b")],
            requeued: std::cell::RefCell::new(Vec::new()),
            fail_goal: Some("goal-a".into()),
        };
        // A per-engineer requeue failure is logged, not fatal.
        let outcome = drain_by_requeue(dir.path(), &requeue).unwrap();
        assert_eq!(outcome.in_flight_at_start, 2);
        assert_eq!(outcome.requeued, 1);
        assert_eq!(outcome.in_flight_at_end, 1);
        assert_eq!(&*requeue.requeued.borrow(), &["goal-b"]);
    }

    #[test]
    fn drain_by_requeue_no_engineers_is_a_clean_noop() {
        let dir = tempdir().unwrap();
        let requeue = FakeRequeue {
            engineers: Vec::new(),
            requeued: std::cell::RefCell::new(Vec::new()),
            fail_goal: None,
        };
        let outcome = drain_by_requeue(dir.path(), &requeue).unwrap();
        assert_eq!(outcome.in_flight_at_start, 0);
        assert_eq!(outcome.requeued, 0);
        assert_eq!(outcome.in_flight_at_end, 0);
        assert!(draining_flag_path(dir.path()).exists());
    }

    #[test]
    fn poll_interval_scales_with_budget() {
        assert_eq!(poll_interval_for(0), Duration::from_millis(50));
        assert_eq!(poll_interval_for(1), Duration::from_millis(100));
        assert_eq!(poll_interval_for(10), Duration::from_millis(500));
        assert_eq!(poll_interval_for(120), Duration::from_secs(5));
    }
}
