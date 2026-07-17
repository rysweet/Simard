//! Production [`EngineerRequeue`] wiring for the self-deploy drain.
//!
//! The self-deploy drain ([`crate::safe_update::drain::drain_by_requeue`])
//! never kills a producing engineer and never fails on a wall-clock timeout.
//! Instead it enumerates the LIVE engineer set from the on-disk worktree claim
//! sentinels and **requeues** each one's goal back onto the board so a
//! restarted binary re-picks it up. The engineer's own `SessionCheckpoint`
//! (written at every phase boundary — see
//! `src/engineer_loop/types.rs::SessionCheckpoint`) already persists its
//! progress, and the goal record persists as `Active`, so the requeue only has
//! to make the goal **re-pickable**.
//!
//! Re-pickability is gated by
//! [`crate::ooda_actions::advance_goal::find_live_engineer_for_goal`], which
//! treats a goal as claimed only while its worktree claim sentinel names a
//! *live* process. Removing the sentinel therefore releases the goal back onto
//! the board without touching the running engineer. We deliberately do **not**
//! signal or kill the process: rename-based atomic swap
//! ([`crate::safe_update::swap::atomic_install`]) is safe against a running
//! executable, so a producing engineer is free to finish its PR on the old
//! inode while the restarted binary picks up any goal that did not finish.

use std::path::{Path, PathBuf};

use crate::engineer_worktree::{ENGINEER_CLAIM_FILE, live_claimed_engineers};
use crate::safe_update::SafeUpdateError;
use crate::safe_update::drain::{EngineerRequeue, InFlightEngineer};

/// Production requeue effect. Reads the live engineer set from
/// `<state_root>/engineer-worktrees/` and releases each worktree's claim
/// sentinel so a restarted binary re-picks up the goal.
pub struct ProdEngineerRequeue {
    state_root: PathBuf,
}

impl ProdEngineerRequeue {
    /// Build a requeue effect rooted at `state_root` (the directory that holds
    /// the `engineer-worktrees/` subdir — i.e. `~/.simard`).
    pub fn new(state_root: PathBuf) -> Self {
        Self { state_root }
    }
}

impl EngineerRequeue for ProdEngineerRequeue {
    fn in_flight(&self) -> Vec<InFlightEngineer> {
        live_claimed_engineers(&self.state_root)
            .into_iter()
            .map(|e| InFlightEngineer {
                goal_id: e.goal_id,
                worktree: e.worktree_path,
                pid: Some(e.pid),
            })
            .collect()
    }

    fn requeue(&self, engineer: &InFlightEngineer) -> Result<(), SafeUpdateError> {
        release_claim(&engineer.worktree).map_err(|reason| SafeUpdateError::DrainIo {
            action: "release engineer claim".into(),
            path: engineer.worktree.join(ENGINEER_CLAIM_FILE),
            reason,
        })?;
        tracing::info!(
            goal_id = %engineer.goal_id,
            worktree = %engineer.worktree.display(),
            "[self-deploy] requeued engineer goal onto the board (claim released; process left running)"
        );
        Ok(())
    }
}

/// Remove the worktree claim sentinel, releasing the goal back onto the board.
/// A missing sentinel is success (the engineer may have already exited).
fn release_claim(worktree: &Path) -> Result<(), String> {
    let claim = worktree.join(ENGINEER_CLAIM_FILE);
    match std::fs::remove_file(&claim) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_claim(worktree: &Path, pid: i32) {
        std::fs::create_dir_all(worktree).unwrap();
        std::fs::write(worktree.join(ENGINEER_CLAIM_FILE), format!("{pid}\n")).unwrap();
    }

    #[test]
    fn requeue_releases_the_claim_sentinel_without_touching_the_worktree() {
        let wt = tempdir().unwrap();
        write_claim(wt.path(), 4321);
        // A marker file standing in for the engineer's in-progress work.
        std::fs::write(wt.path().join("work.txt"), b"in progress").unwrap();

        let requeue = ProdEngineerRequeue::new(PathBuf::from("/unused"));
        let engineer = InFlightEngineer {
            goal_id: "fix-thing".into(),
            worktree: wt.path().to_path_buf(),
            pid: Some(4321),
        };
        requeue.requeue(&engineer).unwrap();

        // Claim released → goal re-pickable; the rest of the worktree is intact.
        assert!(!wt.path().join(ENGINEER_CLAIM_FILE).exists());
        assert!(wt.path().join("work.txt").exists());
    }

    #[test]
    fn requeue_is_ok_when_claim_already_gone() {
        let wt = tempdir().unwrap();
        std::fs::create_dir_all(wt.path()).unwrap();
        let requeue = ProdEngineerRequeue::new(PathBuf::from("/unused"));
        let engineer = InFlightEngineer {
            goal_id: "already-exited".into(),
            worktree: wt.path().to_path_buf(),
            pid: None,
        };
        requeue.requeue(&engineer).unwrap();
    }

    #[test]
    fn in_flight_enumerates_live_claimed_engineers() {
        // Live claim = our own pid, which is alive with a matching starttime.
        let root = tempdir().unwrap();
        let worktrees = root.path().join(crate::engineer_worktree::WORKTREES_SUBDIR);
        let self_pid = std::process::id() as i32;
        let starttime = crate::engineer_worktree::read_pid_starttime_public(self_pid);
        let wt = worktrees.join("fix-abc-1700000000-abc123");
        std::fs::create_dir_all(&wt).unwrap();
        let sentinel = match starttime {
            Some(st) => format!("{self_pid}\n{st}\n"),
            None => format!("{self_pid}\n"),
        };
        std::fs::write(wt.join(ENGINEER_CLAIM_FILE), sentinel).unwrap();

        let requeue = ProdEngineerRequeue::new(root.path().to_path_buf());
        let live = requeue.in_flight();
        assert_eq!(live.len(), 1, "expected one live-claimed engineer");
        assert_eq!(live[0].goal_id, "fix-abc");
        assert_eq!(live[0].pid, Some(self_pid));
    }
}
