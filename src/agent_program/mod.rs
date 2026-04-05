mod goal_curator;
mod improvement_curator;
mod meeting_facilitator;
mod objective_relay;
mod parsing;
mod types;

#[cfg(test)]
mod test_support;

// Re-export all public items so `crate::agent_program::X` still works.
pub use goal_curator::GoalCuratorProgram;
pub use improvement_curator::ImprovementCuratorProgram;
pub use meeting_facilitator::MeetingFacilitatorProgram;
pub use objective_relay::ObjectiveRelayProgram;
pub use types::{AgentProgram, AgentProgramContext, AgentProgramMemoryRecord};
