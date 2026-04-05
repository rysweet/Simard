//! Goal assignment and progress tracking via the cognitive memory bridge.
//!
//! Supervisors assign goals to subordinates by writing semantic facts into
//! the hive (shared cognitive memory). Subordinates read their assigned
//! goals and report progress back through the same channel. This avoids
//! raw IPC and ensures all inter-agent communication is auditable through
//! the memory system.
//!
//! Fact conventions:
//! - Goal facts:     concept = "goal-assignment", tag = "sub:<sub_id>"
//! - Progress facts: concept = "goal-progress",   tag = "sub:<sub_id>"

mod operations;
mod types;

#[cfg(test)]
mod tests;

// Re-export all public items so `crate::agent_goal_assignment::X` still works.
pub use operations::{assign_goal, poll_progress, read_assigned_goal, report_progress};
pub use types::SubordinateProgress;

/// The concept used for goal assignment facts in the hive.
const GOAL_CONCEPT: &str = "goal-assignment";

/// The concept used for progress report facts in the hive.
const PROGRESS_CONCEPT: &str = "goal-progress";

/// Confidence value for goal and progress facts.
/// Set high because these are authoritative supervisor directives.
const DIRECTIVE_CONFIDENCE: f64 = 0.95;

/// Tag prefix for subordinate-scoped facts.
fn sub_tag(sub_id: &str) -> String {
    format!("sub:{sub_id}")
}

/// The source_id used for goal assignment facts.
fn goal_source_id(sub_id: &str) -> String {
    format!("supervisor:goal:{sub_id}")
}

/// The source_id used for progress report facts.
fn progress_source_id(sub_id: &str) -> String {
    format!("subordinate:progress:{sub_id}")
}
