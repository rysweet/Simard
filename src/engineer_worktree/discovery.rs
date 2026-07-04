//! Discovery of the LIVE engineer set from on-disk worktree claim sentinels
//! (issue #2580).
//!
//! This is the same claim-liveness contract the daemon's own
//! [`crate::ooda_actions::advance_goal::find_live_engineer_for_goal`] trusts to
//! avoid a duplicate spawn, generalized to enumerate EVERY live-claimed goal so
//! the dashboard's "Active Engineers" gauge reflects the true live set —
//! including a bare `bin/simard engineer run single-process` subprocess that
//! never registers a tmux subagent session (the telemetry gap that made the
//! gauge read ZERO while the daemon was actively running engineers).

use std::path::Path;

use super::WORKTREES_SUBDIR;
use super::claim::{claim_is_live, read_engineer_claim_full};

/// One live engineer discovered from a worktree claim sentinel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEngineerWorktree {
    /// Goal id recovered from the worktree directory name.
    pub goal_id: String,
    /// PID recorded in the claim sentinel (the allocating daemon/engineer).
    pub pid: i32,
}

/// Enumerate every engineer worktree under `<state_root>/engineer-worktrees/`
/// whose `.simard-engineer-claim` sentinel still names a live process
/// (starttime-validated via [`claim_is_live`]).
///
/// Tolerant of all I/O errors — a missing root or an unreadable entry yields no
/// rows, never a panic — so a dashboard read never fails because of transient
/// filesystem state.
pub fn live_claimed_engineers(state_root: &Path) -> Vec<LiveEngineerWorktree> {
    let root = state_root.join(WORKTREES_SUBDIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(claim) = read_engineer_claim_full(&path) else {
            continue;
        };
        if !claim_is_live(&claim) {
            continue;
        }
        let goal_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(goal_id_from_worktree_dir)
            .unwrap_or_default();
        out.push(LiveEngineerWorktree {
            goal_id,
            pid: claim.pid,
        });
    }
    out
}

/// Recover the goal id from a worktree directory name of the allocator's shape
/// `<goal-id>-<epoch_secs>-<hex6>` (see `engineer_worktree::cleanup::unique_suffix`).
///
/// Only strips the two-field suffix when it actually matches (a run of digits
/// then a 6-char hex tag); otherwise the whole directory name is returned so an
/// unexpected name still yields a stable, non-empty dedup key.
fn goal_id_from_worktree_dir(name: &str) -> String {
    let mut it = name.rsplitn(3, '-');
    let last = it.next(); // hex6
    let mid = it.next(); // epoch secs
    let head = it.next(); // goal id
    match (head, mid, last) {
        (Some(goal), Some(secs), Some(hex))
            if !goal.is_empty()
                && secs.len() >= 6
                && secs.bytes().all(|b| b.is_ascii_digit())
                && hex.len() == 6
                && hex.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            goal.to_string()
        }
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::ENGINEER_CLAIM_FILE;
    use super::super::claim::format_engineer_claim;
    use super::*;
    use std::fs;

    fn write_claim(dir: &Path, pid: u32) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(ENGINEER_CLAIM_FILE), format_engineer_claim(pid)).unwrap();
    }

    #[test]
    fn goal_id_recovered_from_allocator_dir_name() {
        assert_eq!(
            goal_id_from_worktree_dir("fix-the-thing-1783168109-b000d0"),
            "fix-the-thing"
        );
        // Goal ids that themselves contain hyphens (and a slug hash) survive.
        assert_eq!(
            goal_id_from_worktree_dir("advance-agent-parity-f29bb15c-1783168109-b000d0"),
            "advance-agent-parity-f29bb15c"
        );
    }

    #[test]
    fn goal_id_falls_back_to_whole_name_when_suffix_absent() {
        assert_eq!(
            goal_id_from_worktree_dir("no-suffix-here"),
            "no-suffix-here"
        );
        assert_eq!(goal_id_from_worktree_dir("plain"), "plain");
    }

    #[test]
    fn missing_worktrees_root_yields_no_engineers() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(live_claimed_engineers(tmp.path()).is_empty());
    }

    #[test]
    fn live_claim_is_reported_dead_claim_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(WORKTREES_SUBDIR);

        // A worktree whose claim names THIS live test process → reported.
        write_claim(
            &root.join("alive-goal-1783168109-a1b2c3"),
            std::process::id(),
        );
        // A worktree whose claim names an almost-certainly-dead PID → ignored.
        write_claim(&root.join("dead-goal-1783168110-d4e5f6"), 999_999_999);
        // A directory with no claim sentinel at all → ignored.
        fs::create_dir_all(root.join("unclaimed-goal-1783168111-000000")).unwrap();

        let live = live_claimed_engineers(tmp.path());
        assert_eq!(
            live.len(),
            1,
            "only the live-claim worktree counts: {live:?}"
        );
        assert_eq!(live[0].goal_id, "alive-goal");
        assert_eq!(live[0].pid, std::process::id() as i32);
    }
}
