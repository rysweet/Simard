use crate::base_types::{BaseTypeId, BaseTypeOutcome};
use crate::identity::OperatingMode;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

use super::types::AgentProgramContext;

pub(crate) fn test_context(objective: &str) -> AgentProgramContext {
    AgentProgramContext {
        session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap(),
        identity_name: "test-identity".to_string(),
        mode: OperatingMode::Engineer,
        selected_base_type: BaseTypeId::new("local-harness"),
        topology: RuntimeTopology::SingleProcess,
        runtime_node: RuntimeNodeId::local(),
        mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        objective: objective.to_string(),
        active_goals: vec![],
    }
}

pub(crate) fn test_outcome() -> BaseTypeOutcome {
    BaseTypeOutcome {
        plan: "test plan".to_string(),
        execution_summary: "executed successfully".to_string(),
        evidence: vec!["evidence-1".to_string()],
    }
}
