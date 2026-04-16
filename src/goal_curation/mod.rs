//! Top-5 goal board with active goals and backlog curation.
//!
//! `GoalBoard` maintains a strict maximum of 5 active goals. Promotion from
//! backlog to active enforces the cap, and progress updates track completion.

mod operations;
mod types;

// Re-export all public items so `crate::goal_curation::X` still works.
pub use operations::{
    DEFAULT_SEED_GOALS, add_active_goal, add_backlog_item, archive_completed, load_goal_board,
    persist_board, promote_to_active, save_goal_board, seed_default_board,
    surface_developer_discoveries, update_goal_progress,
};
pub use types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};

#[cfg(test)]
mod tests;
