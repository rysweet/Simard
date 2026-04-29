//! OODA (Observe-Orient-Decide-Act) loop for continuous autonomous operation.
//!
//! The outer OODA cycle gathers observations from all subsystems, orients by
//! ranking priorities, decides on actions within concurrency limits, and
//! dispatches them. If any bridge is unavailable, the cycle degrades honestly
//! (Pillar 11): the observation records `None` for that subsystem.

mod bridge_factory;
mod curate;
mod decide;
mod observe;
mod orient;
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
mod tests_types;

// Re-export all public items so `crate::ooda_loop::X` still works.
pub use bridge_factory::{bridges_from_state_root, connect_memory};
pub use curate::{check_meeting_handoffs, promote_from_backlog};
pub use decide::decide;
pub use observe::{gather_environment, observe};
pub use orient::orient;
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

mod cycle;
pub use cycle::run_ooda_cycle;
