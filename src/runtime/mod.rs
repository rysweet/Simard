mod session;
mod traits;
mod types;

pub use traits::*;
pub use types::*;

use std::sync::Arc;
use std::time::Instant;

use crate::agent_program::{AgentProgram, ObjectiveRelayProgram};
use crate::base_types::BaseTypeFactory;
use crate::error::{SimardError, SimardResult};
use crate::evidence::EvidenceStore;
use crate::goals::{GoalStore, InMemoryGoalStore};
use crate::handoff::{InMemoryHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore};
use crate::memory::MemoryStore;
use crate::prompt_assets::{PromptAssetRef, PromptAssetStore};
use crate::session::{SessionIdGenerator, SessionRecord};

pub struct RuntimePorts {
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
}

impl RuntimePorts {
    pub fn new(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        base_types: BaseTypeRegistry,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> Self {
        Self::with_runtime_services(
            prompt_store,
            memory_store,
            evidence_store,
            Arc::new(
                InMemoryGoalStore::try_default().expect("default goal store should initialize"),
            ),
            base_types,
            Arc::new(
                InProcessTopologyDriver::try_default()
                    .expect("in-process topology driver should initialize"),
            ),
            Arc::new(
                InMemoryMailboxTransport::try_default()
                    .expect("in-memory transport should initialize"),
            ),
            Arc::new(
                InProcessSupervisor::try_default()
                    .expect("in-process supervisor should initialize"),
            ),
            session_ids,
        )
    }

    pub fn with_session_ids(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        base_types: BaseTypeRegistry,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> Self {
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
    ) -> Self {
        Self::with_runtime_services_and_program(
            prompt_store,
            memory_store,
            evidence_store,
            goal_store,
            base_types,
            topology_driver,
            transport,
            supervisor,
            Arc::new(
                ObjectiveRelayProgram::try_default()
                    .expect("default agent program should initialize"),
            ),
            Arc::new(
                InMemoryHandoffStore::try_default()
                    .expect("default handoff store should initialize"),
            ),
            session_ids,
        )
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

pub struct RuntimeKernel {
    ports: RuntimePorts,
    request: RuntimeRequest,
    state: RuntimeState,
    factory: Arc<dyn BaseTypeFactory>,
    prompt_assets: Vec<PromptAssetRef>,
    last_session: Option<SessionRecord>,
    runtime_node: RuntimeNodeId,
    mailbox_address: RuntimeAddress,
    start_time: Instant,
    subordinates: Vec<crate::runtime_ipc::IpcSubprocessHandle>,
}

pub type LocalRuntime = RuntimeKernel;

impl RuntimeKernel {
    pub fn compose(ports: RuntimePorts, request: RuntimeRequest) -> SimardResult<Self> {
        request.manifest.memory_policy.validate()?;

        if !ports.topology_driver.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedRuntimeTopology {
                topology: request.topology,
                driver: ports.topology_driver.descriptor().identity,
            });
        }

        if !request
            .manifest
            .supports_base_type(&request.selected_base_type)
        {
            return Err(SimardError::UnsupportedBaseType {
                identity: request.manifest.name.clone(),
                base_type: request.selected_base_type.to_string(),
            });
        }

        let factory = ports
            .base_types
            .get(&request.selected_base_type)
            .ok_or_else(|| SimardError::AdapterNotRegistered {
                base_type: request.selected_base_type.to_string(),
            })?;

        let descriptor = factory.descriptor();
        for capability in &request.manifest.required_capabilities {
            if !descriptor.capabilities.contains(capability) {
                return Err(SimardError::MissingCapability {
                    base_type: descriptor.id.to_string(),
                    capability: *capability,
                });
            }
        }

        if !descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        let runtime_node = ports.topology_driver.local_node()?;
        let mailbox_address = ports.transport.mailbox_for(&runtime_node)?;

        Ok(Self {
            ports,
            request,
            state: RuntimeState::Initializing,
            factory,
            prompt_assets: Vec::new(),
            last_session: None,
            runtime_node,
            mailbox_address,
            start_time: Instant::now(),
            subordinates: Vec::new(),
        })
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn reflector(&self) -> crate::runtime_reflection::LocalReflector {
        let base_types = self
            .ports
            .base_types
            .registered_ids()
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        let memory_backends = vec![self.ports.memory_store.descriptor().identity.clone()];
        let identities = self.request.manifest.components.clone();
        let mut r = crate::runtime_reflection::LocalReflector::new(
            self.request.topology,
            self.start_time,
            base_types,
            memory_backends,
            identities,
        );
        if let Some(session) = &self.last_session {
            r.set_session_phase(session.phase);
        }
        r
    }

    pub fn compose_from_handoff(
        ports: RuntimePorts,
        request: RuntimeRequest,
        snapshot: RuntimeHandoffSnapshot,
    ) -> SimardResult<Self> {
        if snapshot.identity_name != request.manifest.name {
            return Err(SimardError::InvalidHandoffSnapshot {
                field: "identity_name".to_string(),
                reason: format!(
                    "snapshot identity '{}' does not match request identity '{}'",
                    snapshot.identity_name, request.manifest.name
                ),
            });
        }
        if snapshot.selected_base_type != request.selected_base_type {
            return Err(SimardError::InvalidHandoffSnapshot {
                field: "selected_base_type".to_string(),
                reason: format!(
                    "snapshot base type '{}' does not match request base type '{}'",
                    snapshot.selected_base_type, request.selected_base_type
                ),
            });
        }

        let mut sanitized_snapshot = snapshot;
        sanitized_snapshot.session = sanitized_snapshot
            .session
            .as_ref()
            .map(SessionRecord::redacted_for_handoff);

        let mut runtime = Self::compose(ports, request)?;
        for record in &sanitized_snapshot.memory_records {
            runtime.ports.memory_store.put(record.clone())?;
        }
        for record in &sanitized_snapshot.evidence_records {
            runtime.ports.evidence_store.record(record.clone())?;
        }
        runtime.last_session = sanitized_snapshot.session.clone();
        runtime.ports.handoff_store.save(sanitized_snapshot)?;
        Ok(runtime)
    }

    pub fn start(&mut self) -> SimardResult<()> {
        self.ensure_available("start")?;
        self.request
            .manifest
            .prompt_assets
            .iter()
            .try_for_each(|reference| self.ports.prompt_store.load(reference).map(|_| ()))?;
        self.prompt_assets = self.request.manifest.prompt_assets.clone();

        self.transition(RuntimeState::Ready)
    }

    pub fn stop(&mut self) -> SimardResult<()> {
        if matches!(self.state, RuntimeState::Stopped | RuntimeState::Stopping) {
            return Err(SimardError::RuntimeStopped {
                action: "stop".to_string(),
            });
        }

        self.transition(RuntimeState::Stopping)?;
        self.transition(RuntimeState::Stopped)
    }

    /// Spawn a subordinate subprocess with IPC transport.
    ///
    /// Only available when the runtime topology is `MultiProcess`.
    #[cfg(unix)]
    pub fn spawn_subordinate(
        &mut self,
        binary_path: &std::path::Path,
        identity_name: &str,
        socket_path: &std::path::Path,
    ) -> SimardResult<u32> {
        if self.request.topology != RuntimeTopology::MultiProcess {
            return Err(SimardError::UnsupportedRuntimeTopology {
                topology: self.request.topology,
                driver: "spawn_subordinate requires MultiProcess".to_string(),
            });
        }
        let handle = crate::runtime_ipc::spawn_subprocess(binary_path, identity_name, socket_path)?;
        let pid = handle.pid();
        self.subordinates.push(handle);
        Ok(pid)
    }

    /// Gracefully shut down all tracked subordinate subprocesses.
    pub fn shutdown_all(&mut self) -> SimardResult<()> {
        let mut last_err = None;
        for handle in self.subordinates.drain(..) {
            if let Err(e) = crate::runtime_ipc::shutdown_subprocess(handle) {
                last_err = Some(e);
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Number of active subordinate subprocesses.
    pub fn subordinate_count(&self) -> usize {
        self.subordinates.len()
    }

    pub fn run(&mut self, objective: impl Into<String>) -> SimardResult<SessionOutcome> {
        self.ensure_available("run")?;
        let objective = objective.into();

        let result = self.execute_session(objective);

        if result.is_err() && !matches!(self.state, RuntimeState::Stopped | RuntimeState::Stopping)
        {
            self.mark_last_session_failed();
            let _ = self.transition(RuntimeState::Failed);
        }

        result
    }

    pub fn export_handoff(&self) -> SimardResult<RuntimeHandoffSnapshot> {
        let memory_records = match self.last_session.as_ref() {
            Some(session) => self.ports.memory_store.list_for_session(&session.id)?,
            None => Vec::new(),
        };
        let evidence_records = match self.last_session.as_ref() {
            Some(session) => self.ports.evidence_store.list_for_session(&session.id)?,
            None => Vec::new(),
        };

        let snapshot = RuntimeHandoffSnapshot {
            exported_state: self.state,
            identity_name: self.request.manifest.name.clone(),
            selected_base_type: self.request.selected_base_type.clone(),
            topology: self.request.topology,
            source_runtime_node: self.runtime_node.clone(),
            source_mailbox_address: self.mailbox_address.clone(),
            session: self
                .last_session
                .as_ref()
                .map(SessionRecord::redacted_for_handoff),
            memory_records,
            evidence_records,
            copilot_submit_audit: None,
        };
        self.ports.handoff_store.save(snapshot.clone())?;
        Ok(snapshot)
    }

    fn ensure_available(&self, action: &str) -> SimardResult<()> {
        match self.state {
            RuntimeState::Stopped | RuntimeState::Stopping => Err(SimardError::RuntimeStopped {
                action: action.to_string(),
            }),
            RuntimeState::Failed => Err(SimardError::RuntimeFailed {
                action: action.to_string(),
            }),
            _ => Ok(()),
        }
    }

    fn transition(&mut self, next: RuntimeState) -> SimardResult<()> {
        if !self.state.can_transition_to(next) {
            return Err(SimardError::InvalidRuntimeTransition {
                from: self.state,
                to: next,
            });
        }

        self.state = next;
        Ok(())
    }

    fn remember_session(&mut self, session: &SessionRecord) {
        self.last_session = Some(session.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeId;
    use crate::evidence::InMemoryEvidenceStore;
    use crate::identity::{ManifestContract, MemoryPolicy, OperatingMode};
    use crate::memory::InMemoryMemoryStore;
    use crate::metadata::Provenance;
    use crate::prompt_assets::{InMemoryPromptAssetStore, PromptAsset};
    use crate::session::{SessionId, SessionIdGenerator, SessionPhase};
    use crate::test_support::TestAdapter;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestSessionIds(AtomicU64);

    impl TestSessionIds {
        fn new() -> Self {
            Self(AtomicU64::new(1))
        }
    }

    impl SessionIdGenerator for TestSessionIds {
        fn next_id(&self) -> SessionId {
            let n = self.0.fetch_add(1, Ordering::Relaxed);
            SessionId::parse(format!("session-00000000-0000-0000-0000-{n:012}")).unwrap()
        }
    }

    fn test_contract() -> ManifestContract {
        ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            crate::metadata::Freshness::now().unwrap(),
        )
        .unwrap()
    }

    fn test_manifest() -> crate::identity::IdentityManifest {
        crate::identity::IdentityManifest::new(
            "test-identity",
            "0.1.0",
            vec![crate::prompt_assets::PromptAssetRef::new(
                "test-system",
                "test.md",
            )],
            vec![BaseTypeId::new("local-harness")],
            crate::base_types::capability_set([
                crate::base_types::BaseTypeCapability::PromptAssets,
                crate::base_types::BaseTypeCapability::SessionLifecycle,
                crate::base_types::BaseTypeCapability::Memory,
                crate::base_types::BaseTypeCapability::Evidence,
                crate::base_types::BaseTypeCapability::Reflection,
            ]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
    }

    fn test_ports() -> RuntimePorts {
        let prompt_store = Arc::new(InMemoryPromptAssetStore::new([PromptAsset::new(
            "test-system",
            "test.md",
            "You are a test system.",
        )]));
        let memory_store = Arc::new(InMemoryMemoryStore::try_default().unwrap());
        let evidence_store = Arc::new(InMemoryEvidenceStore::try_default().unwrap());
        let mut registry = BaseTypeRegistry::default();
        registry.register(TestAdapter::single_process("local-harness").unwrap());
        RuntimePorts::new(
            prompt_store,
            memory_store,
            evidence_store,
            registry,
            Arc::new(TestSessionIds::new()),
        )
    }

    fn test_request() -> RuntimeRequest {
        RuntimeRequest::new(
            test_manifest(),
            BaseTypeId::new("local-harness"),
            RuntimeTopology::SingleProcess,
        )
    }

    // --- Kernel compose tests ---

    #[test]
    fn compose_initializes_in_initializing_state() {
        let kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        assert_eq!(kernel.state(), RuntimeState::Initializing);
    }

    #[test]
    fn compose_rejects_unregistered_base_type() {
        let mut request = test_request();
        request.selected_base_type = BaseTypeId::new("unknown-adapter");
        request
            .manifest
            .supported_base_types
            .push(BaseTypeId::new("unknown-adapter"));
        let result = RuntimeKernel::compose(test_ports(), request);
        assert!(matches!(
            result,
            Err(SimardError::AdapterNotRegistered { .. })
        ));
    }

    #[test]
    fn compose_rejects_unsupported_base_type_for_identity() {
        let mut request = test_request();
        request.selected_base_type = BaseTypeId::new("not-in-manifest");
        let result = RuntimeKernel::compose(test_ports(), request);
        assert!(matches!(
            result,
            Err(SimardError::UnsupportedBaseType { .. })
        ));
    }

    // --- Lifecycle tests ---

    #[test]
    fn start_transitions_to_ready() {
        let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        kernel.start().unwrap();
        assert_eq!(kernel.state(), RuntimeState::Ready);
    }

    #[test]
    fn stop_transitions_to_stopped() {
        let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        kernel.start().unwrap();
        kernel.stop().unwrap();
        assert_eq!(kernel.state(), RuntimeState::Stopped);
    }

    #[test]
    fn stop_on_stopped_runtime_returns_error() {
        let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        kernel.start().unwrap();
        kernel.stop().unwrap();
        let err = kernel.stop().unwrap_err();
        assert!(matches!(err, SimardError::RuntimeStopped { .. }));
    }

    #[test]
    fn run_on_stopped_runtime_returns_error() {
        let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        kernel.start().unwrap();
        kernel.stop().unwrap();
        let err = kernel.run("test").unwrap_err();
        assert!(matches!(err, SimardError::RuntimeStopped { .. }));
    }

    // --- Session orchestration integration test ---

    #[test]
    fn full_session_lifecycle_produces_outcome_and_returns_to_ready() {
        let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        kernel.start().unwrap();
        let outcome = kernel.run("Implement feature X").unwrap();
        assert_eq!(kernel.state(), RuntimeState::Ready);
        assert!(!outcome.plan.is_empty());
        assert!(!outcome.execution_summary.is_empty());
        assert!(!outcome.reflection.summary.is_empty());
        assert_eq!(outcome.session.phase, SessionPhase::Complete);
    }

    #[test]
    fn multiple_sessions_each_return_to_ready() {
        let mut kernel = RuntimeKernel::compose(test_ports(), test_request()).unwrap();
        kernel.start().unwrap();
        kernel.run("First objective").unwrap();
        assert_eq!(kernel.state(), RuntimeState::Ready);
        kernel.run("Second objective").unwrap();
        assert_eq!(kernel.state(), RuntimeState::Ready);
    }
}
