//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::identity_composition::max_subordinate_depth;
use crate::ooda_brain::{
    EngineerLifecycleDecision, OodaBrain, apply_decision_to_state, gather_engineer_lifecycle_ctx,
};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

use crate::ooda_actions::make_outcome;

/// Spawn a subordinate engineer for a goal that the LLM picked
/// `spawn_engineer` for, then mutate the active board to record the
/// assignment.
///
/// Honours `SIMARD_SUBORDINATE_DEPTH` vs. `SIMARD_MAX_SUBORDINATE_DEPTH`
/// so a recursing supervisor does not spawn forever.
pub fn dispatch_spawn_engineer(
    action: &PlannedAction,
    state: &mut OodaState,
    goal_id: &str,
    task: &str,
    brain: &dyn OodaBrain,
) -> ActionOutcome {
    // Re-check assignment under exclusive state borrow to prevent a
    // double-spawn race (two cycles parsing spawn_engineer back-to-back).
    if let Some(g) = state.active_goals.active.iter().find(|g| g.id == goal_id)
        && g.assigned_to.is_some()
    {
        return make_outcome(
            action,
            true,
            format!(
                "spawn_engineer skipped: goal '{goal_id}' already assigned to subordinate '{}'",
                g.assigned_to.as_deref().unwrap_or("?"),
            ),
        );
    }

    // Defense-in-depth (issue #1227): check the on-disk engineer-worktrees
    // directory for any live worktree already pursuing this goal. The
    // `assigned_to` board check above can miss in-flight engineers if the
    // daemon was restarted between spawn and goal-status writeback (the
    // engineer subprocess survives systemd unit restart). Without this
    // check, we burn a second LLM session on the same goal.
    //
    // Issue #1266: instead of unconditionally returning success=true (which
    // clears the failure counter and makes FAILURE_PENALTY useless), consult
    // the prompt-driven brain. The brain reasons about whether to keep
    // skipping, reclaim, deprioritize, file an issue, or block the goal.
    let state_root_inflight = engineer_worktree_state_root();
    if let Some(live) = find_live_engineer_for_goal(&state_root_inflight, goal_id) {
        let ctx = gather_engineer_lifecycle_ctx(state, &state_root_inflight, goal_id, &live);
        let decision = match brain.decide_engineer_lifecycle(&ctx) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    error = %e,
                    "brain.decide_engineer_lifecycle failed; falling back to continue_skipping",
                );
                EngineerLifecycleDecision::ContinueSkipping {
                    rationale: format!("brain-error fallback: {e}"),
                }
            }
        };
        return apply_lifecycle_decision(action, state, goal_id, &live, decision);
    }

    // Recursion guard. Default current depth = 0 (top-level supervisor).
    let current_depth: u32 = std::env::var("SIMARD_SUBORDINATE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let depth_limit = max_subordinate_depth();
    if depth_limit < u32::MAX && current_depth >= depth_limit {
        eprintln!(
            "[simard] spawn_engineer DENIED for goal '{goal_id}': depth {current_depth} >= limit {depth_limit}"
        );
        return make_outcome(
            action,
            false,
            format!(
                "spawn_engineer denied for goal '{goal_id}': subordinate depth {current_depth} >= configured limit {depth_limit}"
            ),
        );
    }

    let agent_name = build_engineer_name(goal_id);
    let parent_repo = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!(
                    "spawn_engineer failed for goal '{goal_id}': cannot resolve current_dir: {e}"
                ),
            );
        }
    };

    // Allocate a per-engineer git worktree (issue #1197) so concurrent
    // engineers never share the same checkout. The worktree lives under
    // `<state_root>/engineer-worktrees/` and is cleaned up when the
    // subordinate is reaped (or via Drop as a safety net).
    let state_root = engineer_worktree_state_root();
    let worktree = match crate::engineer_worktree::EngineerWorktree::allocate(
        &parent_repo,
        &state_root,
        goal_id,
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "[simard] spawn_engineer FAILED for goal '{goal_id}': worktree allocation: {e}"
            );
            return make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': worktree allocation: {e}"),
            );
        }
    };
    let worktree_path = worktree.path().to_path_buf();

    let config = SubordinateConfig {
        agent_name: agent_name.clone(),
        goal: task.to_string(),
        role: AgentRole::Engineer,
        worktree_path,
        current_depth,
    };

    match spawn_subordinate(&config) {
        Ok(handle) => {
            // Record the assignment so subsequent cycles take the
            // heartbeat-checking path instead of re-spawning.
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.assigned_to = Some(agent_name.clone());
            }
            // Take ownership of the worktree on the OODA state so the reaper
            // path can clean it up after the subordinate exits. Drop is the
            // safety net if the entry leaves the map without explicit cleanup.
            state
                .engineer_worktrees
                .insert(goal_id.to_string(), worktree);

            // WS-2: persist the tmux session into the dashboard registry so
            // the Recent Actions feed can render Attach deep-links. Failures
            // are logged but never block subagent execution.
            if !handle.session_name.is_empty() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let record = crate::subagent_sessions::SubagentSession {
                    agent_id: agent_name.clone(),
                    session_name: handle.session_name.clone(),
                    host: "local".to_string(),
                    pid: handle.pid,
                    created_at: now,
                    ended_at: None,
                    goal_id: goal_id.to_string(),
                };
                if let Err(e) = crate::subagent_sessions::record_spawn(record) {
                    tracing::warn!(
                        target: "simard::subagent_sessions",
                        agent = %agent_name,
                        session = %handle.session_name,
                        error = %e,
                        "failed to persist subagent session registry entry; spawn proceeds",
                    );
                }
            }

            eprintln!(
                "[simard] spawn_engineer dispatched: goal='{goal_id}', agent='{agent_name}', pid={}",
                handle.pid,
            );
            make_outcome(
                action,
                true,
                format!(
                    "spawn_engineer dispatched: agent='{agent_name}', task='{}' (goal '{goal_id}', pid={})",
                    truncate_for_log(task),
                    handle.pid,
                ),
            )
        }
        Err(e) => {
            // Explicitly cleanup the worktree we just allocated; Drop is the
            // safety net but explicit cleanup gives observable failure logs.
            if let Err(ce) = worktree.cleanup() {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    goal = %goal_id,
                    error = %ce,
                    "explicit worktree cleanup after spawn failure failed",
                );
            }
            eprintln!("[simard] spawn_engineer FAILED for goal '{goal_id}': {e}");
            make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': {e}"),
            )
        }
    }
}

/// Resolve the supervisor state root for engineer worktrees.
///
/// Honors `SIMARD_STATE_ROOT` then falls back to `$HOME/.simard`, matching
/// the supervisor's own resolution to keep all per-engineer state in a
/// single discoverable tree.
fn engineer_worktree_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}

/// Scan `<state_root>/engineer-worktrees/` for any directory whose name
/// starts with `<goal_id>-` and whose `.simard-engineer-claim` sentinel
/// names a live PID. Returns the first such path, or None if no live
/// engineer is currently pursuing this goal.
///
/// This is a defense-in-depth check used by `dispatch_spawn_engineer`
/// to prevent duplicate engineer subprocesses on the same goal across
/// daemon restarts (see issue #1227). Stateless: relies only on the
/// on-disk worktree dir and the per-worktree PID sentinel introduced
/// by issue #1213.
pub fn find_live_engineer_for_goal(
    state_root: &std::path::Path,
    goal_id: &str,
) -> Option<std::path::PathBuf> {
    let worktrees_root = state_root.join(crate::engineer_worktree::WORKTREES_SUBDIR);
    let entries = std::fs::read_dir(&worktrees_root).ok()?;
    let prefix = format!("{goal_id}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let claim_path = path.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE);
        let raw = match std::fs::read_to_string(&claim_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Sentinel format (issue #1238): `<pid>\n<starttime>\n` (starttime
        // optional for backwards compat with pre-#1238 sentinels).
        let mut lines = raw.lines();
        let pid: i32 = match lines.next().and_then(|s| s.trim().parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let recorded_starttime: Option<u64> = lines.next().and_then(|s| s.trim().parse().ok());
        if !crate::engineer_worktree::is_pid_alive_public(pid) {
            continue;
        }
        // Starttime guard: if the sentinel records a starttime, it must
        // still match the live process. Mismatch → recycled PID, treat as
        // dead. Pre-#1238 sentinels have no starttime → fall back to PID-only.
        if let Some(recorded) = recorded_starttime {
            match crate::engineer_worktree::read_pid_starttime_public(pid) {
                Some(current) if current == recorded => {}
                _ => continue,
            }
        }
        return Some(path);
    }
    None
}

/// Build a unique subordinate agent name for a goal.
///
/// The epoch suffix prevents collisions when a goal's previous engineer
/// died and a fresh one needs to be spawned in the same process.
fn build_engineer_name(goal_id: &str) -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("engineer-{goal_id}-{epoch}")
}

/// Truncate a user-derived string for safe inclusion in outcome detail / logs.
fn truncate_for_log(s: &str) -> String {
    const MAX: usize = 256;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Apply a brain decision at the engineer-lifecycle skip site (issue #1266).
///
/// Wraps the pure state mutation in `apply_decision_to_state` with the IO
/// side effects each variant requires: numeric kill of the sentinel pid +
/// `git worktree remove` for `ReclaimAndRedispatch`, `gh issue create` for
/// `OpenTrackingIssue`. `success` is `true` only for `ContinueSkipping` so
/// every other branch lets the existing FAILURE_PENALTY engage in the next
/// orient phase (see `src/ooda_loop/orient.rs:12`).
fn apply_lifecycle_decision(
    action: &PlannedAction,
    state: &mut OodaState,
    goal_id: &str,
    live_worktree: &std::path::Path,
    decision: EngineerLifecycleDecision,
) -> ActionOutcome {
    let success = matches!(decision, EngineerLifecycleDecision::ContinueSkipping { .. });

    if let EngineerLifecycleDecision::ReclaimAndRedispatch { .. } = &decision {
        if let Some(pid) = read_sentinel_pid(live_worktree)
            && let Err(e) = numeric_kill(pid)
        {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                pid,
                error = %e,
                "reclaim_and_redispatch: failed to kill engineer pid",
            );
        }
        if let Err(e) = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(live_worktree)
            .status()
        {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                worktree = %live_worktree.display(),
                error = %e,
                "reclaim_and_redispatch: git worktree remove failed",
            );
        }
    }

    if let EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } = &decision {
        let result = std::process::Command::new("gh")
            .args([
                "issue",
                "create",
                "--title",
                title,
                "--body",
                body,
                "--label",
                "ooda-stuck",
            ])
            .status();
        if let Err(e) = result {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                error = %e,
                "open_tracking_issue: gh issue create failed",
            );
        }
    }

    let detail = apply_decision_to_state(&decision, state, goal_id);
    make_outcome(action, success, detail)
}

/// Read the sentinel pid file written by the engineer-worktree allocator.
/// Returns `None` if the file is missing or unparseable.
fn read_sentinel_pid(worktree: &std::path::Path) -> Option<i32> {
    let claim = worktree.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE);
    let raw = std::fs::read_to_string(claim).ok()?;
    raw.lines().next()?.trim().parse().ok()
}

/// Numeric SIGTERM via `libc::kill`. Per repo shell policy and the #1266
/// spec we never shell out to name-based process terminators.
fn numeric_kill(pid: i32) -> std::io::Result<()> {
    // SAFETY: libc::kill is FFI but the call is well-defined for any i32.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
