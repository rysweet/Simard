mod seed;
mod store;
mod types;

// Re-export all public items so `crate::goals::X` still works.
pub use seed::seed_default_goals;
pub use store::{FileBackedGoalStore, GoalStore, InMemoryGoalStore};
pub use types::{GoalRecord, GoalStatus, GoalUpdate, goal_slug};
