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
pub(crate) mod test_helpers;
#[cfg(test)]
mod tests_dispatch;
#[cfg(test)]
mod tests_goal_session;

use crate::error::SimardResult;
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaBridges, OodaState, PlannedAction};

/// Minimum procedure usage count required for skill extraction.
const SKILL_MIN_USAGE: u32 = 3;

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
/// Actions are dispatched concurrently using [`std::thread::scope`].
/// `LaunchSession` actions run fully in parallel (they spawn independent
/// PTY subprocesses with no shared state). All other actions serialise
/// through a [`Mutex`] on `bridges` and `state` — they are typically fast
/// bridge calls, so the lock is held briefly.
///
/// Each action is independent; a failure in one does not abort the others.
/// Returns one [`ActionOutcome`] per input action, in the same order.
pub fn dispatch_actions(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    use std::sync::Mutex;

    let bridges = Mutex::new(bridges);
    let state = Mutex::new(state);

    let outcomes = std::thread::scope(|s| {
        let handles: Vec<_> = actions
            .iter()
            .map(|action| {
                s.spawn(|| match action.kind {
                    // LaunchSession is fully independent — no shared state.
                    ActionKind::LaunchSession => session::dispatch_launch_session(action),
                    // All other actions need bridges and/or state.
                    _ => {
                        let mut bg = bridges.lock().expect("bridges lock poisoned");
                        let mut sg = state.lock().expect("state lock poisoned");
                        dispatch_one(action, &mut bg, &mut sg)
                    }
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().expect("action thread panicked"))
            .collect::<Vec<_>>()
    });

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
        ActionKind::PollDeveloperActivity => {
            simple_actions::dispatch_poll_developer_activity(action, bridges)
        }
        ActionKind::ExtractIdeas => simple_actions::dispatch_extract_ideas(action, bridges),
    }
}
