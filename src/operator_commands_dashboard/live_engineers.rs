//! Authoritative live-engineer set for the dashboard "Active Engineers" panel
//! (issue #2580).
//!
//! ## Why this exists
//!
//! The panel used to read a single, incomplete source and reported ZERO
//! engineers while the daemon was actively running them:
//!
//! - `current_work` read the [`crate::agent_registry::FileBackedAgentRegistry`]
//!   (`agent_registry.json`), which nothing writes in production — so it was
//!   always empty.
//! - `workboard` read the `subagent_sessions` registry, which only records a
//!   spawn that obtained a tmux session name; a bare `bin/simard engineer run
//!   single-process` subprocess never appears there.
//!
//! Neither source sees an engineer that the daemon dispatched as a bare
//! subprocess, so the gauge read zero while the daemon believed several goals
//! were occupied (it uses the worktree claim sentinels — see below — to avoid
//! duplicate spawns).
//!
//! ## The true live set
//!
//! The union of both live sources, deduplicated by goal:
//!   (a) subagent sessions with no `ended_at` (tmux-tracked engineers), and
//!   (b) engineer worktrees whose `.simard-engineer-claim` sentinel still names
//!       a live process ([`crate::engineer_worktree::live_claimed_engineers`]) —
//!       the SAME claim the daemon's own `find_live_engineer_for_goal` trusts,
//!       which is what makes bare single-process engineers visible.

use std::path::Path;

/// One live engineer, from whichever source proved it live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveEngineer {
    /// Goal the engineer is working (dedup key + display label).
    pub goal_id: String,
    /// PID (the subagent session pid, or the worktree claim's allocating pid).
    pub pid: i32,
    /// Whether the pid is currently alive.
    pub alive: bool,
    /// Provenance: `"subagent_session"` or `"worktree_claim"`.
    pub source: &'static str,
    /// Session start time (epoch secs), when known (subagent sessions only).
    pub started_at: Option<i64>,
}

/// Compute the authoritative live-engineer set under `state_root`, deduplicated
/// by `goal_id`. Subagent-session evidence (which carries a start time) wins
/// over a bare worktree-claim for the same goal.
pub(crate) fn live_engineers(state_root: &Path) -> Vec<LiveEngineer> {
    // BTreeMap keeps a deterministic, goal-sorted order for a stable UI.
    let mut by_goal: std::collections::BTreeMap<String, LiveEngineer> =
        std::collections::BTreeMap::new();

    // (a) tmux-tracked subagent sessions still running (read from this state
    // root so the view is consistent and hermetically testable).
    let sessions = crate::subagent_sessions::load_from(
        &crate::subagent_sessions::registry_path_under(state_root),
    )
    .sessions;
    for s in sessions {
        if s.ended_at.is_some() {
            continue;
        }
        let goal_id = if s.goal_id.is_empty() {
            s.session_name.clone()
        } else {
            s.goal_id.clone()
        };
        let pid = s.pid as i32;
        by_goal.entry(goal_id.clone()).or_insert(LiveEngineer {
            goal_id,
            pid,
            alive: crate::engineer_worktree::is_pid_alive_public(pid),
            source: "subagent_session",
            started_at: Some(s.created_at),
        });
    }

    // (b) engineer worktrees with a live claim sentinel (starttime-validated).
    for claim in crate::engineer_worktree::live_claimed_engineers(state_root) {
        by_goal
            .entry(claim.goal_id.clone())
            .or_insert(LiveEngineer {
                goal_id: claim.goal_id,
                pid: claim.pid,
                alive: true, // already filtered to a live, starttime-matched pid
                source: "worktree_claim",
                started_at: None,
            });
    }

    by_goal.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineer_worktree::{ENGINEER_CLAIM_FILE, WORKTREES_SUBDIR};
    use std::fs;

    fn write_live_claim(state_root: &Path, dir_name: &str) {
        let dir = state_root.join(WORKTREES_SUBDIR).join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        // A PID-only claim (no starttime line) naming THIS live test process is
        // treated as live by `claim_is_live`'s PID-only fallback.
        fs::write(
            dir.join(ENGINEER_CLAIM_FILE),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
    }

    #[test]
    fn worktree_claims_make_bare_single_process_engineers_visible() {
        let tmp = tempfile::tempdir().unwrap();
        // Two goals with live worktree claims but NO subagent session recorded
        // (the exact bare `bin/simard engineer run single-process` gap).
        write_live_claim(tmp.path(), "goal-alpha-1783168109-a1b2c3");
        write_live_claim(tmp.path(), "goal-beta-1783168110-d4e5f6");

        let live = live_engineers(tmp.path());
        assert_eq!(
            live.len(),
            2,
            "both live worktree claims must be counted even with no subagent session: {live:?}"
        );
        let goals: Vec<&str> = live.iter().map(|e| e.goal_id.as_str()).collect();
        assert!(goals.contains(&"goal-alpha"));
        assert!(goals.contains(&"goal-beta"));
        assert!(live.iter().all(|e| e.source == "worktree_claim" && e.alive));
    }

    #[test]
    fn empty_state_root_reports_no_engineers() {
        let tmp = tempfile::tempdir().unwrap();
        // No subagent-sessions registry and no worktrees → zero (honest zero).
        assert!(live_engineers(tmp.path()).is_empty());
    }
}
