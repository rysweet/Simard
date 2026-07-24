//! Top goal board with active goals and backlog curation.
//!
//! `GoalBoard` maintains a strict maximum of [`MAX_ACTIVE_GOALS`] active goals.
//! Promotion from backlog to active enforces the cap, and progress updates
//! track completion. Issue #2405 adds a **goal graph** on top of the flat
//! board: typed parent↔child edges ([`edges`]) and a decomposition driver
//! ([`decompose`]) that breaks one large goal into bounded sub-goals.

pub mod completion_gate;
pub mod labels;
pub mod live_signal;
pub mod live_signal_source;
pub mod no_progress_breaker;
pub mod no_progress_why;
mod operations;
pub mod outcome_verify;
pub mod progress_evidence;
pub mod progress_reviewer;
pub mod recipe_progress_checker;
mod types;

mod decompose;
mod edges;
mod prioritize;

// Re-export all public items so `crate::goal_curation::X` still works.
pub use operations::CarryoverVerification;
pub use operations::{
    BoardPlacement, DEFAULT_SEED_GOALS, DEFAULT_STEWARD_SCORE, active_goals_as_records,
    add_active_goal, add_backlog_item, archive_completed, board_snapshot_hash,
    clear_goal_assignment, default_seed_goals, load_goal_board, overwrite_memory_cache,
    persist_board, promote_to_active, read_latest_carryover, record_as_active_goal,
    resolve_seed_goals, rollup_parent_progress, save_goal_board, save_goal_board_with_removals,
    seed_board_from_seed_goals, seed_default_board, simard_state_root, update_goal_progress,
    update_goal_progress_with_evidence, verify_goal_carryover, write_goal_carryover,
};
pub(crate) use types::NO_PROGRESS_TRACKING_LABEL_PREFIX;
pub use types::{
    ActiveGoal, BacklogItem, CARRYOVER_CONCEPT, GoalBoard, GoalCarryoverRecord, GoalEdge,
    GoalEdgeType, GoalNode, GoalProgress, MAX_ACTIVE_GOALS, STANDING_MARKER_PREFIX, WipRef,
    description_marks_research, description_marks_standing,
};

pub use decompose::{
    ChildPlacement, DecomposeOutcome, GoalDecomposer, MAX_SUBGOALS, MIN_SUBGOALS,
    RecipeGoalDecomposer, SubGoalProposal, decompose_goal, parse_subgoals_json,
};
pub use edges::{children_of, edges_of_type, node_of, parse_goal_edge, write_edge, write_node};
pub use prioritize::{PrioritizationSignals, prioritize};

pub use completion_gate::{
    COMPLETION_VERIFICATION_METRIC, CompletionEvidence, CompletionEvidenceGate, CompletionVerdict,
    DependencyState, EvidenceSource, FALSE_COMPLETION_RATE_METRIC, GhCliEvidenceSource,
    MissingEvidence, VerificationOutcome, archive_completed_evidence_aware,
    archive_completed_with_evidence, classify_from_missing, classify_outcome,
    completion_evidence_enabled, error_class_from_missing, false_completion_rate,
    has_derivable_signal, is_self_affecting, record_completion_verification,
    record_false_completion_rate,
};

pub use no_progress_breaker::{
    NO_PROGRESS_BLOCKED_PREFIX, NO_PROGRESS_BLOCKED_SUFFIX, NO_PROGRESS_BREAKER_THRESHOLD,
    NoProgressResolution, NoProgressTracker, StuckGoalDisposition, humanize_block_reason,
    is_bare_no_progress_block, is_no_progress_marker, no_progress_blocked_reason,
    no_progress_blocked_reason_with_why, obsolescence_reason, resolution_for_why,
    resolve_no_progress, verify_stuck_goal,
};

pub use no_progress_why::{Evidence, NoProgressClass, NoProgressWhy, NoProgressWhyReasoner};

pub use live_signal::{LiveSignal, LiveSignalSource};
pub use live_signal_source::DaemonLiveSignals;
pub use outcome_verify::{
    GOAL_LIVE_OUTCOME_VERIFICATION_METRIC, OutcomeVerificationReport, outcome_verify_enabled,
    record_outcome_verification, verify_completion_candidates, verify_goal_outcome,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_adapter;
#[cfg(test)]
mod tests_carryover;
#[cfg(test)]
mod tests_labels;
#[cfg(test)]
mod tests_no_progress_breaker;
// Issue #16 (TDD): pure-policy tests for the agentic root-cause upgrade of the
// no-progress breaker (classification tokens, WHY/evidence value types, the
// WHY-aware block-reason renderer, and the class -> resolution map).
#[cfg(test)]
mod tests_no_progress_why;
// Issue #17 (TDD): pure primitives of the already-blocked re-investigation pass
// — the `is_bare_no_progress_block` deterministic rail and the
// `NoProgressTracker` persisted `reinvestigated` dedupe set (lifecycle + serde).
#[cfg(test)]
mod tests_no_progress_reinvestigation;
#[cfg(test)]
mod tests_operations;
#[cfg(test)]
mod tests_reverse_adapter;
#[cfg(test)]
mod tests_save_with_removals;

// Issue #2329 (SimPR4): repeated goal-board snapshot saves supersede via
// CallerKey dedup instead of accumulating live duplicates.
#[cfg(test)]
mod tests_snapshot_dedup;

// Issue #2405: goal decomposition + the typed goal-graph edge model. These
// tests pin the durable edge format, the parent-linkage data model,
// `decompose_goal`, and the parent-progress roll-up.
#[cfg(test)]
mod tests_decompose;
#[cfg(test)]
mod tests_edges;

// Issue #2695 follow-up: the goal prioritization pass differentiates
// undifferentiated (flat) priorities while preserving operator-set ones. These
// tests pin the `priority_explicit` provenance flag and the pure `prioritize`
// pass. The `prioritize` module is added by the implementation step.
#[cfg(test)]
mod tests_prioritize;
