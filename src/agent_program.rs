use crate::base_types::{BaseTypeId, BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::SimardResult;
use crate::identity::OperatingMode;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::sanitization::objective_metadata;
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
}

#[derive(Debug)]
pub struct ObjectiveRelayProgram {
    descriptor: BackendDescriptor,
}

impl ObjectiveRelayProgram {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "agent-program::objective-relay",
                "runtime-port:agent-program",
                Freshness::now()?,
            ),
        })
    }
}

impl AgentProgram for ObjectiveRelayProgram {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn plan_turn(&self, context: &AgentProgramContext) -> SimardResult<BaseTypeTurnInput> {
        Ok(BaseTypeTurnInput {
            objective: context.objective.clone(),
        })
    }

    fn reflection_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        let objective_summary = objective_metadata(&context.objective);
        Ok(format!(
            "Agent program '{}' completed '{}' through '{}' on '{}' from '{}' with {}.",
            self.descriptor.identity,
            context.mode,
            context.selected_base_type,
            context.topology,
            context.runtime_node,
            objective_summary,
        ) + &format!(" Outcome summary: {}.", outcome.execution_summary))
    }

    fn persistence_summary(
        &self,
        context: &AgentProgramContext,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<String> {
        Ok(format!(
            "{} | {} | {}",
            objective_metadata(&context.objective),
            outcome.plan,
            outcome.execution_summary,
        ))
    }
}
