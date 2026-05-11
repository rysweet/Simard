//! Phase 1: drain in-flight engineer dispatches before a self-update.
//!
//! The orchestrator writes `state_dir/draining.flag` and then polls until
//! either no engineers are in-flight or `drain_timeout_seconds` elapses.
//! "In-flight" is approximated by counting subdirectories of
//! `~/.simard/engineer-worktrees/` whose owning Simard-spawned process is
//! still alive (a `/proc/<pid>/status` check). The brain's dispatch site
//! must consult [`super::state::is_draining`] before spawning a new
//! engineer; the wired check lives in
//! `src/engineer_loop/agent_spawn.rs`.

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
    /// In-flight count observed after the drain phase ended.
    pub in_flight_at_end: usize,
    pub elapsed: Duration,
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

/// Drive the drain phase: mark, poll-until-quiescent, then return.
///
/// Reads the engineer-worktrees root from `$HOME/.simard/engineer-worktrees/`.
/// For tests / non-default installs use [`drain_to_quiescence_with_root`].
///
/// On `DrainTimeout` the flag is *not* removed — the orchestrator wants
/// new dispatches to stay refused so the operator can investigate without
/// fighting a flood of new engineers.
pub fn drain_to_quiescence(
    state_dir: &Path,
    drain_timeout_seconds: u64,
) -> Result<DrainOutcome, SafeUpdateError> {
    drain_to_quiescence_with_root(state_dir, drain_timeout_seconds, &engineer_worktrees_root())
}

/// Same as [`drain_to_quiescence`] but with an explicit engineer-worktrees
/// root, so tests don't have to depend on the live `~/.simard/` directory.
pub fn drain_to_quiescence_with_root(
    state_dir: &Path,
    drain_timeout_seconds: u64,
    engineer_root: &Path,
) -> Result<DrainOutcome, SafeUpdateError> {
    let started = Instant::now();
    mark_draining(state_dir)?;

    let in_flight_at_start = count_in_flight_engineers_in(engineer_root);
    let deadline = started + Duration::from_secs(drain_timeout_seconds);
    let poll_interval = poll_interval_for(drain_timeout_seconds);

    loop {
        let in_flight = count_in_flight_engineers_in(engineer_root);
        if in_flight == 0 {
            return Ok(DrainOutcome {
                in_flight_at_start,
                in_flight_at_end: 0,
                elapsed: started.elapsed(),
            });
        }
        if Instant::now() >= deadline {
            return Err(SafeUpdateError::DrainTimeout {
                seconds: drain_timeout_seconds,
                in_flight,
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
    fn drain_times_out_with_explicit_root_holding_a_fake_engineer() {
        let dir = tempdir().unwrap();
        let engineers = tempdir().unwrap();
        // Fake engineer worktree without a pid file → counts as in-flight.
        std::fs::create_dir_all(engineers.path().join("eng-1")).unwrap();
        let err = drain_to_quiescence_with_root(dir.path(), 1, engineers.path()).unwrap_err();
        assert!(matches!(err, SafeUpdateError::DrainTimeout { .. }));
        // Flag deliberately remains set.
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
