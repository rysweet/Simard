//! Production [`EngineerRequeue`] wiring for the self-deploy drain.
//!
//! The self-deploy drain ([`crate::safe_update::drain::drain_by_requeue`])
//! never kills a producing engineer and never fails on a wall-clock timeout.
//! Instead it enumerates the LIVE engineer set from the on-disk worktree claim
//! sentinels and reconciles each one's goal lease so a restarted binary can
//! safely reason about it. The engineer's own `SessionCheckpoint`
//! (written at every phase boundary — see
//! `src/engineer_loop/types.rs::SessionCheckpoint`) already persists its
//! progress, and the goal record persists as `Active`.
//!
//! Re-pickability is gated by
//! [`crate::ooda_actions::advance_goal::find_live_engineer_for_goal`], which
//! treats a goal as claimed only while its worktree claim sentinel names a
//! *live* process. Therefore a still-live engineer must keep its sentinel so
//! the restarted daemon does not duplicate the same goal; only a dead or
//! pid-less claim is removed to free the goal. We deliberately do **not** signal
//! or kill the process: rename-based atomic swap
//! ([`crate::safe_update::swap::atomic_install`]) is safe against a running
//! executable, so a producing engineer is free to finish its PR on the old
//! inode.

use std::path::{Path, PathBuf};

use crate::engineer_worktree::{
    ENGINEER_CLAIM_FILE, WORKTREES_SUBDIR, live_claimed_engineers_in_worktrees,
};
use crate::safe_update::SafeUpdateError;
use crate::safe_update::drain::{EngineerRequeue, InFlightEngineer};

/// Production requeue effect. Scans the engineer-worktrees directory for the
/// live engineer set. Live claims are left intact so a restarted binary does
/// not duplicate a producing engineer's goal.
pub struct ProdEngineerRequeue {
    /// The directory that directly contains the per-engineer worktrees
    /// (i.e. `<state_root>/engineer-worktrees`).
    worktrees_root: PathBuf,
}

impl ProdEngineerRequeue {
    /// Build a requeue effect rooted at `state_root` (the directory that holds
    /// the `engineer-worktrees/` subdir — i.e. `~/.simard`).
    pub fn new(state_root: PathBuf) -> Self {
        let worktrees_root = state_root.join(WORKTREES_SUBDIR);
        Self { worktrees_root }
    }

    /// Build a requeue effect directly from the worktrees directory (the dir
    /// that contains the per-engineer worktrees). Used when a caller already
    /// holds that path — e.g. the `engineer_worktrees_root` config override,
    /// which may not be named `engineer-worktrees`.
    pub fn from_worktrees_root(worktrees_root: PathBuf) -> Self {
        Self { worktrees_root }
    }
}

impl EngineerRequeue for ProdEngineerRequeue {
    fn in_flight(&self) -> Vec<InFlightEngineer> {
        live_claimed_engineers_in_worktrees(&self.worktrees_root)
            .into_iter()
            .map(|e| InFlightEngineer {
                goal_id: e.goal_id,
                worktree: e.worktree_path,
                pid: Some(e.pid),
            })
            .collect()
    }

    fn requeue(&self, engineer: &InFlightEngineer) -> Result<(), SafeUpdateError> {
        if let Some(pid) = engineer.pid
            && crate::engineer_worktree::is_pid_alive_public(pid)
        {
            tracing::info!(
                goal_id = %engineer.goal_id,
                pid = pid,
                "[self-deploy] left live engineer's claim intact (survives atomic swap; keeps goal lease)"
            );
            return Ok(());
        }

        release_claim(&engineer.worktree).map_err(|reason| SafeUpdateError::DrainIo {
            action: "release engineer claim".into(),
            path: engineer.worktree.join(ENGINEER_CLAIM_FILE),
            reason,
        })?;
        tracing::info!(
            goal_id = %engineer.goal_id,
            worktree = %engineer.worktree.display(),
            "[self-deploy] released dead engineer's claim so the goal can be picked up"
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
    fn requeue_leaves_live_claim_sentinel_without_touching_the_worktree() {
        let wt = tempdir().unwrap();
        let self_pid = std::process::id() as i32;
        write_claim(wt.path(), self_pid);
        // A marker file standing in for the engineer's in-progress work.
        std::fs::write(wt.path().join("work.txt"), b"in progress").unwrap();

        let requeue = ProdEngineerRequeue::new(PathBuf::from("/unused"));
        let engineer = InFlightEngineer {
            goal_id: "fix-thing".into(),
            worktree: wt.path().to_path_buf(),
            pid: Some(self_pid),
        };
        requeue.requeue(&engineer).unwrap();

        // Claim retained → restarted daemon sees the live lease and does not
        // duplicate this producing engineer's goal.
        assert!(wt.path().join(ENGINEER_CLAIM_FILE).exists());
        assert!(wt.path().join("work.txt").exists());
    }

    #[test]
    fn requeue_releases_dead_claim_sentinel_without_touching_the_worktree() {
        let wt = tempdir().unwrap();
        let dead_pid = i32::MAX;
        write_claim(wt.path(), dead_pid);
        std::fs::write(wt.path().join("work.txt"), b"in progress").unwrap();

        let requeue = ProdEngineerRequeue::new(PathBuf::from("/unused"));
        let engineer = InFlightEngineer {
            goal_id: "fix-dead".into(),
            worktree: wt.path().to_path_buf(),
            pid: Some(dead_pid),
        };
        requeue.requeue(&engineer).unwrap();

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

    #[test]
    fn from_worktrees_root_scans_the_dir_directly_even_when_oddly_named() {
        // An override directory that is NOT named `engineer-worktrees` must be
        // scanned directly, not via a WORKTREES_SUBDIR re-join.
        let worktrees = tempdir().unwrap();
        let self_pid = std::process::id() as i32;
        let starttime = crate::engineer_worktree::read_pid_starttime_public(self_pid);
        let wt = worktrees.path().join("build-thing-1700000000-def456");
        std::fs::create_dir_all(&wt).unwrap();
        let sentinel = match starttime {
            Some(st) => format!("{self_pid}\n{st}\n"),
            None => format!("{self_pid}\n"),
        };
        std::fs::write(wt.join(ENGINEER_CLAIM_FILE), sentinel).unwrap();

        let requeue = ProdEngineerRequeue::from_worktrees_root(worktrees.path().to_path_buf());
        let live = requeue.in_flight();
        assert_eq!(live.len(), 1, "expected one live-claimed engineer");
        assert_eq!(live[0].goal_id, "build-thing");
    }
}
