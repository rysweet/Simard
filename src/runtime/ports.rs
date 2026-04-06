use std::sync::Arc;

use crate::agent_program::{AgentProgram, ObjectiveRelayProgram};
use crate::error::{SimardError, SimardResult};
use crate::evidence::EvidenceStore;
use crate::goals::{GoalStore, InMemoryGoalStore};
use crate::handoff::{InMemoryHandoffStore, RuntimeHandoffStore};
use crate::memory::MemoryStore;
use crate::prompt_assets::PromptAssetStore;
use crate::session::SessionIdGenerator;

use super::traits::{
    InMemoryMailboxTransport, InProcessSupervisor, InProcessTopologyDriver,
    RuntimeMailboxTransport, RuntimeSupervisor, RuntimeTopologyDriver,
};
use super::types::BaseTypeRegistry;

pub struct RuntimePorts {
    pub(super) prompt_store: Arc<dyn PromptAssetStore>,
    pub(super) memory_store: Arc<dyn MemoryStore>,
    pub(super) evidence_store: Arc<dyn EvidenceStore>,
    pub(super) goal_store: Arc<dyn GoalStore>,
    pub(super) base_types: BaseTypeRegistry,
    pub(super) topology_driver: Arc<dyn RuntimeTopologyDriver>,
    pub(super) transport: Arc<dyn RuntimeMailboxTransport>,
    pub(super) supervisor: Arc<dyn RuntimeSupervisor>,
    pub(super) agent_program: Arc<dyn AgentProgram>,
    pub(super) handoff_store: Arc<dyn RuntimeHandoffStore>,
    pub(super) session_ids: Arc<dyn SessionIdGenerator>,
}

impl RuntimePorts {
    pub fn new(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        base_types: BaseTypeRegistry,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> SimardResult<Self> {
        Self::with_runtime_services(
            prompt_store,
            memory_store,
            evidence_store,
            Arc::new(InMemoryGoalStore::try_default().map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "goal_store".into(),
                    reason: e.to_string(),
                }
            })?),
            base_types,
            Arc::new(InProcessTopologyDriver::try_default().map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "topology_driver".into(),
                    reason: e.to_string(),
                }
            })?),
            Arc::new(InMemoryMailboxTransport::try_default().map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "transport".into(),
                    reason: e.to_string(),
                }
            })?),
            Arc::new(InProcessSupervisor::try_default().map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "supervisor".into(),
                    reason: e.to_string(),
                }
            })?),
            session_ids,
        )
    }

    pub fn with_session_ids(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        base_types: BaseTypeRegistry,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> SimardResult<Self> {
        Self::new(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            session_ids,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "runtime assembly requires explicit injected ports for topology neutrality"
    )]
    pub fn with_runtime_services(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        goal_store: Arc<dyn GoalStore>,
        base_types: BaseTypeRegistry,
        topology_driver: Arc<dyn RuntimeTopologyDriver>,
        transport: Arc<dyn RuntimeMailboxTransport>,
        supervisor: Arc<dyn RuntimeSupervisor>,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> SimardResult<Self> {
        Ok(Self::with_runtime_services_and_program(
            prompt_store,
            memory_store,
            evidence_store,
            goal_store,
            base_types,
            topology_driver,
            transport,
            supervisor,
            Arc::new(ObjectiveRelayProgram::try_default().map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "agent_program".into(),
                    reason: e.to_string(),
                }
            })?),
            Arc::new(InMemoryHandoffStore::try_default().map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "handoff_store".into(),
                    reason: e.to_string(),
                }
            })?),
            session_ids,
        ))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "runtime assembly requires explicit injected ports for topology-neutral execution"
    )]
    pub fn with_runtime_services_and_program(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        goal_store: Arc<dyn GoalStore>,
        base_types: BaseTypeRegistry,
        topology_driver: Arc<dyn RuntimeTopologyDriver>,
        transport: Arc<dyn RuntimeMailboxTransport>,
        supervisor: Arc<dyn RuntimeSupervisor>,
        agent_program: Arc<dyn AgentProgram>,
        handoff_store: Arc<dyn RuntimeHandoffStore>,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> Self {
        Self {
            prompt_store,
            memory_store,
            evidence_store,
            goal_store,
            base_types,
            topology_driver,
            transport,
            supervisor,
            agent_program,
            handoff_store,
            session_ids,
        }
    }
}
