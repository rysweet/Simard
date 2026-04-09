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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::InMemoryEvidenceStore;
    use crate::memory::InMemoryMemoryStore;
    use crate::prompt_assets::InMemoryPromptAssetStore;
    use crate::session::UuidSessionIdGenerator;

    // -- with_runtime_services_and_program --

    #[test]
    fn with_runtime_services_and_program_constructs_all_ports() {
        let prompt_store: Arc<dyn PromptAssetStore> =
            Arc::new(InMemoryPromptAssetStore::new(vec![]));
        let memory_store: Arc<dyn MemoryStore> =
            Arc::new(InMemoryMemoryStore::try_default().unwrap());
        let evidence_store: Arc<dyn EvidenceStore> =
            Arc::new(InMemoryEvidenceStore::try_default().unwrap());
        let goal_store: Arc<dyn GoalStore> = Arc::new(InMemoryGoalStore::try_default().unwrap());
        let base_types = BaseTypeRegistry::default();
        let topology_driver: Arc<dyn RuntimeTopologyDriver> =
            Arc::new(InProcessTopologyDriver::try_default().unwrap());
        let transport: Arc<dyn RuntimeMailboxTransport> =
            Arc::new(InMemoryMailboxTransport::try_default().unwrap());
        let supervisor: Arc<dyn RuntimeSupervisor> =
            Arc::new(InProcessSupervisor::try_default().unwrap());
        let agent_program: Arc<dyn AgentProgram> =
            Arc::new(ObjectiveRelayProgram::try_default().unwrap());
        let handoff_store: Arc<dyn RuntimeHandoffStore> =
            Arc::new(InMemoryHandoffStore::try_default().unwrap());
        let session_ids: Arc<dyn SessionIdGenerator> = Arc::new(UuidSessionIdGenerator);

        let ports = RuntimePorts::with_runtime_services_and_program(
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
        );

        // Verify ports are accessible (the struct has fields we can read).
        assert!(ports.base_types.registered_ids().is_empty());
    }

    // -- new constructor --

    #[test]
    fn new_constructor_creates_runtime_with_defaults() {
        let prompt_store: Arc<dyn PromptAssetStore> =
            Arc::new(InMemoryPromptAssetStore::new(vec![]));
        let memory_store: Arc<dyn MemoryStore> =
            Arc::new(InMemoryMemoryStore::try_default().unwrap());
        let evidence_store: Arc<dyn EvidenceStore> =
            Arc::new(InMemoryEvidenceStore::try_default().unwrap());
        let base_types = BaseTypeRegistry::default();
        let session_ids: Arc<dyn SessionIdGenerator> = Arc::new(UuidSessionIdGenerator);

        let result = RuntimePorts::new(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            session_ids,
        );
        assert!(result.is_ok());
    }

    // -- with_session_ids --

    #[test]
    fn with_session_ids_delegates_to_new() {
        let prompt_store: Arc<dyn PromptAssetStore> =
            Arc::new(InMemoryPromptAssetStore::new(vec![]));
        let memory_store: Arc<dyn MemoryStore> =
            Arc::new(InMemoryMemoryStore::try_default().unwrap());
        let evidence_store: Arc<dyn EvidenceStore> =
            Arc::new(InMemoryEvidenceStore::try_default().unwrap());
        let base_types = BaseTypeRegistry::default();
        let session_ids: Arc<dyn SessionIdGenerator> = Arc::new(UuidSessionIdGenerator);

        let result = RuntimePorts::with_session_ids(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            session_ids,
        );
        assert!(result.is_ok());
    }
}
