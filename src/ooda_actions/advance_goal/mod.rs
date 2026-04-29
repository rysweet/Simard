//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use crate::goal_curation::GoalProgress;
use crate::ooda_loop::{ActionOutcome, OodaBridges, OodaState, PlannedAction};

use super::goal_session::GoalAction;
use super::make_outcome;

mod spawn;
mod subordinate;
use spawn::dispatch_spawn_engineer;
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
    match &goal.status {
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
            return dispatch_spawn_engineer(action, state, &goal_id, &task);
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
