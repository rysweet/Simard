use crate::base_types::{BaseTypeId, BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;
use crate::goals::{GoalRecord, GoalUpdate};
use crate::identity::OperatingMode;
use crate::memory::CognitiveMemoryType;
use crate::metadata::BackendDescriptor;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProgramContext {
    pub session_id: SessionId,
    pub identity_name: String,
    pub mode: OperatingMode,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
    pub objective: String,
    pub active_goals: Vec<GoalRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProgramMemoryRecord {
    pub key_suffix: String,
    pub memory_type: CognitiveMemoryType,
    pub value: String,
}

pub trait AgentProgram: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput>;

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String>;

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String>;

    fn additional_memory_records(
        &self,
        _context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<AgentProgramMemoryRecord>> {
        Ok(Vec::new())
    }

    fn goal_updates(
        &self,
        _context: &AgentProgramContext,
        _outcome: &BaseTypeOutcome,
    ) -> SimardResult<Vec<GoalUpdate>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_program::test_support::test_context;

    #[test]
    fn agent_program_context_equality() {
        let ctx1 = test_context("objective-a");
        let ctx2 = test_context("objective-a");
        assert_eq!(ctx1, ctx2);
    }

    #[test]
    fn agent_program_context_inequality() {
        let ctx1 = test_context("objective-a");
        let ctx2 = test_context("objective-b");
        assert_ne!(ctx1, ctx2);
    }

    #[test]
    fn agent_program_memory_record_fields() {
        let record = AgentProgramMemoryRecord {
            key_suffix: "test-key".to_string(),
            memory_type: CognitiveMemoryType::Semantic,
            value: "test-value".to_string(),
        };
        assert_eq!(record.key_suffix, "test-key");
        assert_eq!(record.memory_type, CognitiveMemoryType::Semantic);
        assert_eq!(record.value, "test-value");
    }
}
