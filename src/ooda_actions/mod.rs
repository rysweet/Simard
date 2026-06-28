//! Action dispatch for the OODA loop.
//!
//! Extracted from `ooda_loop.rs` to keep each module under 400 LOC.
//! Each [`ActionKind`] maps to a concrete subsystem call. Failures are
//! per-action, not cycle-wide (Pillar 11: honest degradation).

// `advance_goal` is `pub(crate)` so the issue-#1911 brain-failure marker
// constants and `is_brain_failure_marker` predicate in
// `advance_goal::spawn` are reachable from `crate::operator_cli::goal`
// (CLI bulk-unblock scoping) and from cross-module test modules.
pub(crate) mod advance_goal;
mod concurrent;
mod goal_session;
mod session;
mod simple_actions;

#[cfg(test)]
pub(crate) mod test_helpers;
#[cfg(test)]
mod tests_advance_goal;
#[cfg(test)]
mod tests_dispatch;
#[cfg(test)]
mod tests_dispatch_concurrency;
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
/// Spawn-path `AdvanceGoal` actions (goals with no live subordinate) run
/// concurrently, each with its own LLM session, so multiple engineers start
/// in a single OODA round instead of ~1 per round (see
/// [`crate::ooda_actions::concurrent`]). All other actions — `LaunchSession`
/// and `SafeUpdate` (independent), and the rest behind a short global lock —
/// run in the serialized phase, preserving today's behavior. Assigned-goal
/// `AdvanceGoal` (heartbeat) actions stay in the serialized phase too.
///
/// Each action is independent; a failure (or panic) in one does not abort the
/// others. Returns one [`ActionOutcome`] per input action, in the same order.
///
/// Concurrency of `AdvanceGoal` dispatch is unbounded here (bounded only by
/// the number of planned actions, itself capped upstream by coverage). The
/// Act phase calls [`dispatch_actions_bounded`] with the AIMD
/// `scaler.current_max()` cap.
pub fn dispatch_actions(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    dispatch_actions_bounded(actions, bridges, state, usize::MAX)
}

/// Like [`dispatch_actions`], but bounds the number of `AdvanceGoal`
/// dispatches running concurrently to `max_concurrency`.
///
/// `max_concurrency` is the AIMD safety cap (`scaler.current_max()`): a hard
/// ceiling on concurrent engineer starts per round, keeping the
/// resource-aware backoff intact. The global `bridges`+`state` lock is NEVER
/// held across the slow goal-action LLM call or the engineer spawn (see
/// [`crate::ooda_actions::concurrent`]).
pub fn dispatch_actions_bounded(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
    max_concurrency: usize,
) -> SimardResult<Vec<ActionOutcome>> {
    use std::sync::Mutex;

    let mut results: Vec<Option<ActionOutcome>> = (0..actions.len()).map(|_| None).collect();

    // Partition indices: spawn-path `AdvanceGoal` (concurrent) vs everything
    // else (serialized — incl. assigned-goal `AdvanceGoal` heartbeat path and
    // all non-`AdvanceGoal` kinds). Classification reads state immutably,
    // before any mutable borrow.
    let mut concurrent_idx: Vec<usize> = Vec::new();
    let mut serialized_idx: Vec<usize> = Vec::new();
    for (i, action) in actions.iter().enumerate() {
        if matches!(action.kind, ActionKind::AdvanceGoal)
            && concurrent::is_concurrent_advance_candidate(action, state)
        {
            concurrent_idx.push(i);
        } else {
            serialized_idx.push(i);
        }
    }

    // ── Phase 1: serialized actions (today's behavior over the subset) ───
    if !serialized_idx.is_empty() {
        let bridges_mx = Mutex::new(&mut *bridges);
        let state_mx = Mutex::new(&mut *state);

        std::thread::scope(|s| {
            let handles: Vec<(usize, std::thread::ScopedJoinHandle<'_, ActionOutcome>)> =
                serialized_idx
                    .iter()
                    .map(|&i| {
                        let action = &actions[i];
                        let bridges_mx = &bridges_mx;
                        let state_mx = &state_mx;
                        (
                            i,
                            s.spawn(move || match action.kind {
                                // LaunchSession and SafeUpdate are fully
                                // independent — no shared state.
                                ActionKind::LaunchSession => {
                                    session::dispatch_launch_session(action)
                                }
                                ActionKind::SafeUpdate => {
                                    simple_actions::dispatch_safe_update(action)
                                }
                                // All other serialized actions take both locks
                                // briefly (fast bridge calls). Recover from a
                                // poisoned lock instead of panicking so one
                                // failed action never crashes the daemon.
                                _ => {
                                    let mut bg =
                                        bridges_mx.lock().unwrap_or_else(|p| p.into_inner());
                                    let mut sg = state_mx.lock().unwrap_or_else(|p| p.into_inner());
                                    dispatch_one(action, &mut bg, &mut sg)
                                }
                            }),
                        )
                    })
                    .collect();

            for (i, handle) in handles {
                let outcome = handle.join().unwrap_or_else(|_| {
                    make_outcome(
                        &actions[i],
                        false,
                        "action dispatch thread panicked".to_string(),
                    )
                });
                results[i] = Some(outcome);
            }
        });
    }

    // ── Phase 2: concurrent spawn-path `AdvanceGoal` ─────────────────────
    if !concurrent_idx.is_empty() {
        concurrent::dispatch_advance_concurrent(
            actions,
            &concurrent_idx,
            bridges,
            state,
            max_concurrency,
            &mut results,
        );
    }

    Ok(results
        .into_iter()
        .map(|o| o.expect("every action index is assigned exactly one outcome"))
        .collect())
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
        ActionKind::SafeUpdate => simple_actions::dispatch_safe_update(action),
    }
}
