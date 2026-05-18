//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use crate::goal_curation::GoalProgress;
use crate::ooda_loop::{ActionOutcome, OodaBridges, OodaState, PlannedAction};

use super::goal_session::GoalAction;
use super::make_outcome;

// `spawn` is `pub(crate)` so the issue-#1911 brain-failure marker
// constants (`BRAIN_FAILURE_BLOCKED_PREFIX`, `BRAIN_FAILURE_BLOCKED_SUFFIX`)
// and the `is_brain_failure_marker` predicate are reachable from
// `crate::operator_cli::goal` (bulk-unblock scoping) and from the
// cross-module tests in `crate::ooda_actions::tests_advance_goal` and
// `crate::operator_cli::tests_goal`.
pub(crate) mod spawn;
mod subordinate;
use spawn::{dispatch_spawn_engineer, is_brain_failure_marker};
// Dispatch-dedup helper introduced by PR #1228; intentionally re-exported so
// the daemon can scan engineer-worktrees for live sentinels before spawning.
// Clippy flags it as unused at the lib level — suppress to preserve the API.
#[allow(unused_imports)]
pub use spawn::find_live_engineer_for_goal;
use subordinate::advance_goal_with_subordinate;
// re-exported for cfg(test) consumers in ooda_actions/tests_advance_goal.rs (false-positive of clippy unused_imports on lib pass — see #1405)
#[allow(unused_imports)]
pub use subordinate::validate_subordinate_completion;

/// AdvanceGoal: progress the target goal on the board.
///
/// If the goal has a subordinate assigned, checks the subordinate's
/// heartbeat via the supervisor. If a base-type session is available
/// (e.g. RustyClawd), delegates the goal to the agent via `run_turn`
/// for real autonomous work. Otherwise, falls back to bumping the
/// progress percentage.
pub(super) fn dispatch_advance_goal(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    let goal_id = match &action.goal_id {
        Some(id) => id.clone(),
        None => {
            return make_outcome(action, false, "advance-goal requires a goal_id".to_string());
        }
    };

    // Find the goal on the board.
    let goal = match state.active_goals.active.iter().find(|g| g.id == goal_id) {
        Some(g) => g.clone(),
        None => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' not found on active board"),
            );
        }
    };

    // If the goal has a subordinate, check heartbeat.
    if let Some(ref sub_name) = goal.assigned_to {
        return advance_goal_with_subordinate(action, bridges, state, &goal_id, sub_name);
    }

    // Blocked and completed goals short-circuit before session dispatch.
    //
    // Issue #1911 — auto-recovery of brain-failure-blocked goals.
    // The deterministic safeguard in `dispatch_spawn_engineer` marks a goal
    // `Blocked(BRAIN_FAILURE_BLOCKED_*)` after 3 consecutive brain failures.
    // When the brain is healthy again the dispatcher would still short-
    // circuit on the persisted marker — that was the production lockout
    // (zero engineer activity for 44+ hours). The recovery branch below
    // clears the marker on first arrival here:
    //   1. confirm the `Blocked` reason was authored by the safeguard
    //      (sentinel-bearing prefix + suffix — operator-set, scope-blocked,
    //      dependency-blocked, and subordinate-blocked reasons are
    //      intentionally rejected so this branch never overrides them);
    //   2. drop the per-goal failure counter so cycle.rs's penalty stops
    //      demoting urgency;
    //   3. restore `GoalProgress::NotStarted` and fall through to the
    //      normal session-based dispatch path. The next cycle that fails
    //      will re-arm the counter from zero.
    match &goal.status {
        GoalProgress::Blocked(reason) if is_brain_failure_marker(reason) => {
            tracing::info!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                prior_failures = state.goal_failure_counts.get(&goal_id).copied().unwrap_or(0),
                "issue #1911 auto-recovery: brain-failure marker cleared; restoring goal to NotStarted",
            );
            eprintln!(
                "[simard] OODA auto-recovery: goal '{goal_id}' brain-failure marker cleared (issue #1911)"
            );
            state.goal_failure_counts.remove(&goal_id);
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.status = GoalProgress::NotStarted;
            }
            // Refresh local snapshot so downstream uses (e.g. session
            // dispatch) see the restored state.
            // NOTE: `goal` is a clone — we deliberately keep falling
            // through into the normal session path below.
        }
        GoalProgress::Blocked(reason) => {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' is blocked: {reason}"),
            );
        }
        GoalProgress::Completed => {
            return make_outcome(
                action,
                true,
                format!("goal '{goal_id}' is already completed"),
            );
        }
        _ => {}
    }

    // Clone the brain Arc up-front so we don't fight the borrow checker
    // when we mutably borrow `bridges.session` below (issue #1266).
    let brain = std::sync::Arc::clone(&bridges.brain);

    // If a base-type session is available, use run_turn for real agent work.
    if let Some(ref mut session) = bridges.session {
        let result =
            super::goal_session::advance_goal_with_session(action, session.as_mut(), state, &goal);

        // For spawn_engineer the dispatcher must perform the actual fork
        // (it owns the state mutation needed to set goal.assigned_to).
        if let Some(GoalAction::SpawnEngineer {
            task,
            files: _,
            issue: _,
        }) = result.action
        {
            return dispatch_spawn_engineer(action, state, &goal_id, &task, brain.as_ref());
        }

        return result.outcome;
    }

    // No session = cannot advance. Fail visibly per PHILOSOPHY.md.
    make_outcome(
        action,
        false,
        format!(
            "goal '{goal_id}' cannot advance: no LLM session available. Check SIMARD_LLM_PROVIDER and auth config."
        ),
    )
}
