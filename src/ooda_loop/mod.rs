//! OODA (Observe-Orient-Decide-Act) loop for continuous autonomous operation.
//!
//! The outer OODA cycle gathers observations from all subsystems, orients by
//! ranking priorities, decides on actions within concurrency limits, and
//! dispatches them. If any memory is unavailable, the cycle degrades honestly
//! (Pillar 11): the observation records `None` for that subsystem.

pub mod adaptive_scaling;
mod client_factory;
mod curate;
// Issue #2359 (BUG 2): per-cycle goal coverage allocator.
pub mod coverage;
pub mod cycle;
mod decide;
mod observe;
mod orient;
// Fix 3 (issue #1): no-progress breaker adapter for the curate phase.
mod no_progress;
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
mod tests_per_goal_cycle;
#[cfg(test)]
mod tests_record_decision_rail;
#[cfg(test)]
mod tests_types;

// Fix 3 (issue #1): integration tests for the no-progress breaker wiring.
#[cfg(test)]
mod tests_no_progress;

// Issue #16 (TDD): integration tests for the agentic root-cause upgrade — the
// investigated adapter routes a stuck goal down the self-resolving ladder and
// only ever authors a human block WITH the concrete why + evidence attached.
#[cfg(test)]
mod tests_no_progress_investigation;

// Issue #17 (TDD): integration tests for the already-blocked re-investigation
// pass — each cycle scans the board for goals parked in a BARE `[OODA-SAFEGUARD]
// … needs human review` block and re-runs the WHY reasoner + resolution ladder
// over them, so no goal is ever stranded with a bare, unexplained block.
#[cfg(test)]
mod tests_no_progress_reinvestigation;

// process_health (TDD): the churn-stopping side effects of the terminal-quarantine
// rung — the re-investigation pass skips quarantined goals (never re-investigates,
// re-schedules, or re-files them) and an at-bound evidence-less UNCLEAR-CRITERIA
// stall is durably marked + blocked with real evidence.
#[cfg(test)]
mod tests_quarantine_churn;

// Issue #16 (follow-up, TDD): direct unit tests for the production
// `DeterministicNoProgressReasoner` — pin the terminal-rung invariant that it
// never returns an empty-evidence WHY, so the breaker can never author a bare
// `evidence=[(none)]` block (the live-daemon defect that stranded the synthetic
// `simard-identity-*` goals). A no-artifact stall is `UNCLEAR-CRITERIA` with a
// named unmeasurable criterion; open work stays `GENUINELY-STUCK` with it.
#[cfg(test)]
mod tests_no_progress_reasoner;

// Issue #2329: Observe-vs-Decide phase weights yield different ranked-recall
// ordering of the same fact set, exercised against the real lbug-backed
// `LibraryCognitiveMemory` adapter.
#[cfg(test)]
mod tests_phase_recall;

// Issue #2395: parity for episodes — Observe-vs-Decide phase weights yield a
// different ranked-recall ordering of the same episode set (driven through the
// usage/text-relevance signals).
#[cfg(test)]
mod tests_phase_recall_episodes;

// PR-C (issue #2281, problem 3): tests for the new `cycle.rs`
// helpers (`pattern_for`, `compose_procedure_name`,
// `derive_triggers_from_objective`).
#[cfg(test)]
mod tests_pr_c_procedures;

// Re-export all public items so `crate::ooda_loop::X` still works.
pub use client_factory::{clients_from_state_root, connect_memory};
pub use curate::{
    check_meeting_handoffs, drain_overseer_whispers, load_tombstones, promote_from_backlog,
    reap_old_handoffs, tombstone_goals,
};
pub use decide::{decide, decide_with_brain};
pub use observe::{gather_environment, observe};
pub use orient::{orient, orient_with_brain};
pub use phase_weights::weights_for_phase;
pub use priority_kind::{SyntheticPriorityKind, is_synthetic_id};
pub use review::review_outcomes;
pub use summary::summarize_cycle_report;
pub use types::{
    ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, IdentityCognition,
    Observation, OodaClients, OodaConfig, OodaPhase, OodaState, OodaStateSnapshot,
    OrchestratorSessionFactory, PlannedAction, Priority,
};

use crate::error::SimardResult;

/// Act: dispatch actions. Failures are per-action, not cycle-wide (Pillar 11).
///
/// Delegates to [`crate::ooda_actions::dispatch_actions_bounded`] which calls
/// the real subsystems (gym memory, supervisor, skill builder, etc.).
/// Takes `&mut OodaClients` so that the optional session can be used for
/// `run_turn` calls during `AdvanceGoal` actions. `max_concurrency` is the
/// AIMD `scaler.current_max()` cap — the hard ceiling on concurrent engineer
/// starts this round.
pub fn act(
    actions: &[PlannedAction],
    memories: &mut OodaClients,
    state: &mut OodaState,
    max_concurrency: usize,
) -> SimardResult<Vec<ActionOutcome>> {
    crate::ooda_actions::dispatch_actions_bounded(actions, memories, state, max_concurrency)
}

pub use cycle::run_ooda_cycle;
pub use cycle::{compose_procedure_name, derive_triggers_from_objective};
