//! Top-5 goal board with active goals and backlog curation.
//!
//! `GoalBoard` maintains a strict maximum of 5 active goals. Promotion from
//! backlog to active enforces the cap, and progress updates track completion.

mod operations;
pub mod progress_evidence;
pub mod progress_reviewer;
pub mod recipe_progress_checker;
mod types;

// Re-export all public items so `crate::goal_curation::X` still works.
pub use operations::{
    DEFAULT_SEED_GOALS, DEFAULT_STEWARD_SCORE, active_goals_as_records, add_active_goal,
    add_backlog_item, archive_completed, clear_goal_assignment, enqueue_stewardship_issue,
    load_goal_board, persist_board, promote_to_active, save_goal_board,
    save_goal_board_with_removals, seed_default_board, simard_state_root, update_goal_progress,
    update_goal_progress_with_evidence,
};
pub use types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS, WipRef};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_adapter;
#[cfg(test)]
mod tests_operations;
#[cfg(test)]
mod tests_save_with_removals;
