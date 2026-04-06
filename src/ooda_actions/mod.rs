//! Action dispatch for the OODA loop.
//!
//! Extracted from `ooda_loop.rs` to keep each module under 400 LOC.
//! Each [`ActionKind`] maps to a concrete subsystem call. Failures are
//! per-action, not cycle-wide (Pillar 11: honest degradation).

mod advance_goal;
mod goal_session;
mod session;
mod simple_actions;
mod verification;

#[cfg(test)]
mod test_helpers;
#[cfg(test)]
mod tests_goal_session;

use crate::error::SimardResult;
use crate::goal_curation::GoalProgress;
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaBridges, OodaState, PlannedAction};

/// Minimum procedure usage count required for skill extraction.
const SKILL_MIN_USAGE: u32 = 3;

/// Advance a goal's progress by one step: `NotStarted → InProgress(10)`,
/// `InProgress(N) → InProgress(N+10)` or `Completed` at 100.
fn next_progress(current: &GoalProgress) -> GoalProgress {
    match current {
        GoalProgress::NotStarted => GoalProgress::InProgress { percent: 10 },
        GoalProgress::InProgress { percent } => {
            let next = (*percent + 10).min(100);
            if next >= 100 {
                GoalProgress::Completed
            } else {
                GoalProgress::InProgress { percent: next }
            }
        }
        other => other.clone(),
    }
}

/// Construct an [`ActionOutcome`] from the shared action reference.
///
/// Centralises the single unavoidable clone of the [`PlannedAction`] so
/// dispatch helpers only need `(action, success, detail)`.
#[inline]
fn make_outcome(action: &PlannedAction, success: bool, detail: String) -> ActionOutcome {
    ActionOutcome {
        action: action.clone(),
        success,
        detail,
    }
}

/// Dispatch a batch of planned actions against live bridges and state.
///
/// Each action is dispatched independently; a failure in one does not
/// abort the others. Returns one [`ActionOutcome`] per input action.
pub fn dispatch_actions(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    let mut outcomes = Vec::with_capacity(actions.len());
    for action in actions {
        let outcome = dispatch_one(action, bridges, state);
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// Dispatch a single planned action and return its outcome.
fn dispatch_one(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> ActionOutcome {
    match action.kind {
        ActionKind::ConsolidateMemory => {
            simple_actions::dispatch_consolidate_memory(action, bridges)
        }
        ActionKind::ResearchQuery => simple_actions::dispatch_research_query(action, bridges),
        ActionKind::RunImprovement => simple_actions::dispatch_run_improvement(action, bridges),
        ActionKind::AdvanceGoal => advance_goal::dispatch_advance_goal(action, bridges, state),
        ActionKind::RunGymEval => simple_actions::dispatch_run_gym_eval(action, bridges),
        ActionKind::BuildSkill => simple_actions::dispatch_build_skill(action, bridges),
        ActionKind::LaunchSession => session::dispatch_launch_session(action),
    }
}
