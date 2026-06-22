//! OODA (Observe-Orient-Decide-Act) loop for continuous autonomous operation.
//!
//! The outer OODA cycle gathers observations from all subsystems, orients by
//! ranking priorities, decides on actions within concurrency limits, and
//! dispatches them. If any bridge is unavailable, the cycle degrades honestly
//! (Pillar 11): the observation records `None` for that subsystem.

pub mod adaptive_scaling;
mod bridge_factory;
mod curate;
pub mod cycle;
mod decide;
mod observe;
mod orient;
pub mod phase_weights;
mod priority_kind;
mod review;
mod summary;
mod types;

#[cfg(test)]
mod tests_observe;
#[cfg(test)]
mod tests_orient;
#[cfg(test)]
mod tests_orient_extra;
#[cfg(test)]
mod tests_parse_failure_1890;
#[cfg(test)]
mod tests_types;

// Issue #2329: Observe-vs-Decide phase weights yield different ranked-recall
// ordering of the same fact set, exercised against the real lbug-backed
// `LibraryCognitiveMemory` adapter.
#[cfg(test)]
mod tests_phase_recall;

// PR-C (issue #2281, problem 3): tests for the new `cycle.rs`
// helpers (`pattern_for`, `compose_procedure_name`,
// `derive_triggers_from_objective`).
#[cfg(test)]
mod tests_pr_c_procedures;

// Re-export all public items so `crate::ooda_loop::X` still works.
pub use bridge_factory::{bridges_from_state_root, connect_memory};
pub use curate::{
    check_meeting_handoffs, promote_from_backlog, reap_old_handoffs, tombstone_goals,
};
pub use decide::{decide, decide_with_brain};
pub use observe::{gather_environment, observe};
pub use orient::{orient, orient_with_brain};
pub use phase_weights::weights_for_phase;
pub use priority_kind::{SyntheticPriorityKind, is_synthetic_id};
pub use review::review_outcomes;
pub use summary::summarize_cycle_report;
pub use types::{
    ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
    OodaBridges, OodaConfig, OodaPhase, OodaState, OodaStateSnapshot, PlannedAction, Priority,
};

use crate::error::SimardResult;

/// Act: dispatch actions. Failures are per-action, not cycle-wide (Pillar 11).
///
/// Delegates to [`crate::ooda_actions::dispatch_actions`] which calls the
/// real subsystems (gym bridge, supervisor, skill builder, etc.).
/// Takes `&mut OodaBridges` so that the optional session can be used for
/// `run_turn` calls during `AdvanceGoal` actions.
pub fn act(
    actions: &[PlannedAction],
    bridges: &mut OodaBridges,
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    crate::ooda_actions::dispatch_actions(actions, bridges, state)
}

pub use cycle::run_ooda_cycle;
pub use cycle::{compose_procedure_name, derive_triggers_from_objective};
