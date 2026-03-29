use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use crate::agent_program::{AgentProgram, AgentProgramContext, ObjectiveRelayProgram};
use crate::base_types::{BaseTypeFactory, BaseTypeId, BaseTypeOutcome, BaseTypeSessionRequest};
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore};
use crate::handoff::{InMemoryHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore};
use crate::identity::IdentityManifest;
use crate::memory::{MemoryRecord, MemoryScope, MemoryStore};
use crate::metadata::{BackendDescriptor, Freshness, FreshnessState};
use crate::prompt_assets::{PromptAssetRef, PromptAssetStore};
use crate::reflection::{ReflectionReport, ReflectionSnapshot, ReflectiveRuntime};
use crate::sanitization::objective_metadata;
use crate::session::{SessionIdGenerator, SessionPhase, SessionRecord};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum RuntimeTopology {
    SingleProcess,
    MultiProcess,
    Distributed,
}

impl Display for RuntimeTopology {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::SingleProcess => "single-process",
            Self::MultiProcess => "multi-process",
            Self::Distributed => "distributed",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum RuntimeState {
    Initializing,
    Ready,
    Active,
    Reflecting,
    Persisting,
    Failed,
    Stopping,
    Stopped,
}

impl Display for RuntimeState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Reflecting => "reflecting",
            Self::Persisting => "persisting",
            Self::Failed => "failed",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        };
        f.write_str(label)
    }
}

impl RuntimeState {
    pub fn can_transition_to(self, next: RuntimeState) -> bool {
        matches!(
            (self, next),
            (Self::Initializing, Self::Ready)
                | (Self::Initializing, Self::Stopping)
                | (Self::Ready, Self::Active)
                | (Self::Ready, Self::Stopping)
                | (Self::Active, Self::Reflecting)
                | (Self::Active, Self::Stopping)
                | (Self::Reflecting, Self::Persisting)
                | (Self::Reflecting, Self::Stopping)
                | (Self::Persisting, Self::Ready)
                | (Self::Persisting, Self::Stopping)
                | (Self::Failed, Self::Stopping)
                | (Self::Stopping, Self::Stopped)
                | (Self::Initializing, Self::Failed)
                | (Self::Ready, Self::Failed)
                | (Self::Active, Self::Failed)
                | (Self::Reflecting, Self::Failed)
                | (Self::Persisting, Self::Failed)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RuntimeNodeId(String);

impl RuntimeNodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn local() -> Self {
        Self::new("node-local")
    }
}

impl Display for RuntimeNodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RuntimeAddress(String);

impl RuntimeAddress {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn local(node: &RuntimeNodeId) -> Self {
        Self::new(format!("inmemory://{node}"))
    }
}

impl Display for RuntimeAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Default)]
pub struct BaseTypeRegistry {
    factories: BTreeMap<BaseTypeId, Arc<dyn BaseTypeFactory>>,
}

impl BaseTypeRegistry {
    pub fn register<F>(&mut self, factory: F)
    where
        F: BaseTypeFactory + 'static,
    {
        self.factories
            .insert(factory.descriptor().id.clone(), Arc::new(factory));
    }

    pub fn get(&self, id: &BaseTypeId) -> Option<Arc<dyn BaseTypeFactory>> {
        self.factories.get(id).map(Arc::clone)
    }
}

pub trait RuntimeTopologyDriver: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn supports_topology(&self, topology: RuntimeTopology) -> bool;

    fn local_node(&self) -> SimardResult<RuntimeNodeId>;
}

pub trait RuntimeMailboxTransport: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn mailbox_for(&self, node: &RuntimeNodeId) -> SimardResult<RuntimeAddress>;
}

pub trait RuntimeSupervisor: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;
}

#[derive(Debug)]
pub struct InProcessTopologyDriver {
    descriptor: BackendDescriptor,
}

impl InProcessTopologyDriver {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "topology::in-process",
                "runtime-port:topology-driver",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeTopologyDriver for InProcessTopologyDriver {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn supports_topology(&self, topology: RuntimeTopology) -> bool {
        matches!(topology, RuntimeTopology::SingleProcess)
    }

    fn local_node(&self) -> SimardResult<RuntimeNodeId> {
        Ok(RuntimeNodeId::local())
    }
}

#[derive(Debug)]
pub struct InMemoryMailboxTransport {
    descriptor: BackendDescriptor,
}

impl InMemoryMailboxTransport {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "transport::in-memory-mailbox",
                "runtime-port:mailbox-transport",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeMailboxTransport for InMemoryMailboxTransport {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn mailbox_for(&self, node: &RuntimeNodeId) -> SimardResult<RuntimeAddress> {
        Ok(RuntimeAddress::local(node))
    }
}

#[derive(Debug)]
pub struct LoopbackMeshTopologyDriver {
    descriptor: BackendDescriptor,
}

impl LoopbackMeshTopologyDriver {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "topology::loopback-mesh",
                "runtime-port:topology-driver",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeTopologyDriver for LoopbackMeshTopologyDriver {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn supports_topology(&self, topology: RuntimeTopology) -> bool {
        matches!(
            topology,
            RuntimeTopology::MultiProcess | RuntimeTopology::Distributed
        )
    }

    fn local_node(&self) -> SimardResult<RuntimeNodeId> {
        Ok(RuntimeNodeId::new("node-loopback-mesh"))
    }
}

#[derive(Debug)]
pub struct LoopbackMailboxTransport {
    descriptor: BackendDescriptor,
}

impl LoopbackMailboxTransport {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "transport::loopback-mailbox",
                "runtime-port:mailbox-transport",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeMailboxTransport for LoopbackMailboxTransport {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn mailbox_for(&self, node: &RuntimeNodeId) -> SimardResult<RuntimeAddress> {
        Ok(RuntimeAddress::new(format!("loopback://{node}")))
    }
}

#[derive(Debug)]
pub struct InProcessSupervisor {
    descriptor: BackendDescriptor,
}

impl InProcessSupervisor {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "supervisor::in-process",
                "runtime-port:supervisor",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeSupervisor for InProcessSupervisor {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }
}

#[derive(Debug)]
pub struct CoordinatedSupervisor {
    descriptor: BackendDescriptor,
}

impl CoordinatedSupervisor {
    pub fn try_default() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "supervisor::coordinated",
                "runtime-port:supervisor",
                Freshness::now()?,
            ),
        })
    }
}

impl RuntimeSupervisor for CoordinatedSupervisor {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }
}

pub struct RuntimePorts {
    prompt_store: Arc<dyn PromptAssetStore>,
    memory_store: Arc<dyn MemoryStore>,
    evidence_store: Arc<dyn EvidenceStore>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeRequest {
    pub manifest: IdentityManifest,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
}

impl RuntimeRequest {
    pub fn new(
        manifest: IdentityManifest,
        selected_base_type: BaseTypeId,
        topology: RuntimeTopology,
    ) -> Self {
        Self {
            manifest,
            selected_base_type,
            topology,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionOutcome {
    pub session: SessionRecord,
    pub plan: String,
    pub execution_summary: String,
    pub reflection: ReflectionReport,
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
        })
    }

    pub fn state(&self) -> RuntimeState {
        self.state
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

        let mut runtime = Self::compose(ports, request)?;
        for record in &snapshot.memory_records {
            runtime.ports.memory_store.put(record.clone())?;
        }
        for record in &snapshot.evidence_records {
            runtime.ports.evidence_store.record(record.clone())?;
        }
        runtime.last_session = snapshot.session.clone();
        runtime.ports.handoff_store.save(snapshot)?;
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
            session: self.last_session.clone(),
            memory_records,
            evidence_records,
        };
        self.ports.handoff_store.save(snapshot.clone())?;
        Ok(snapshot)
    }

    fn execute_session(&mut self, objective: String) -> SimardResult<SessionOutcome> {
        self.transition(RuntimeState::Active)?;

        let mut session = self.new_session(objective);
        self.persist_session_scratch(&mut session)?;
        let outcome = self.run_selected_base_type_session(&mut session)?;
        self.record_execution_evidence(&mut session, &outcome)?;
        let reflection = self.build_reflection(&mut session, &outcome)?;
        self.persist_session_summary(&mut session, &outcome)?;
        self.complete_session(session, outcome, reflection)
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

    fn new_session(&mut self, objective: String) -> SessionRecord {
        let session = SessionRecord::new(
            self.request.manifest.default_mode,
            objective,
            self.request.selected_base_type.clone(),
            self.ports.session_ids.as_ref(),
        );
        self.remember_session(&session);
        session
    }

    fn persist_session_scratch(&mut self, session: &mut SessionRecord) -> SimardResult<()> {
        session.advance(SessionPhase::Preparation)?;
        self.remember_session(session);

        let scratch_key = format!("{}-scratch", session.id);
        self.ports.memory_store.put(MemoryRecord {
            key: scratch_key.clone(),
            scope: MemoryScope::SessionScratch,
            value: objective_metadata(&session.objective),
            session_id: session.id.clone(),
            recorded_in: SessionPhase::Preparation,
        })?;
        session.attach_memory(scratch_key);
        self.remember_session(session);

        Ok(())
    }

    fn run_selected_base_type_session(
        &mut self,
        session: &mut SessionRecord,
    ) -> SimardResult<BaseTypeOutcome> {
        session.advance(SessionPhase::Planning)?;
        self.remember_session(session);

        let context = self.agent_program_context(session);

        let mut base_type_session = self.factory.open_session(BaseTypeSessionRequest {
            session_id: session.id.clone(),
            mode: session.mode,
            topology: self.request.topology,
            prompt_assets: self.prompt_assets.clone(),
            runtime_node: self.runtime_node.clone(),
            mailbox_address: self.mailbox_address.clone(),
        })?;
        base_type_session.open()?;
        let outcome = base_type_session.run_turn(self.ports.agent_program.plan_turn(&context)?)?;
        base_type_session.close()?;
        Ok(outcome)
    }

    fn record_execution_evidence(
        &mut self,
        session: &mut SessionRecord,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<()> {
        session.advance(SessionPhase::Execution)?;
        self.remember_session(session);

        for (index, detail) in outcome.evidence.iter().enumerate() {
            let evidence_id = format!("{}-evidence-{}", session.id, index + 1);
            self.ports.evidence_store.record(EvidenceRecord {
                id: evidence_id.clone(),
                session_id: session.id.clone(),
                phase: SessionPhase::Execution,
                detail: detail.clone(),
                source: EvidenceSource::BaseType(self.request.selected_base_type.clone()),
            })?;
            session.attach_evidence(evidence_id);
        }
        self.remember_session(session);

        Ok(())
    }

    fn build_reflection(
        &mut self,
        session: &mut SessionRecord,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<ReflectionReport> {
        self.transition(RuntimeState::Reflecting)?;
        session.advance(SessionPhase::Reflection)?;
        self.remember_session(session);

        Ok(ReflectionReport {
            summary: self
                .ports
                .agent_program
                .reflection_summary(&self.agent_program_context(session), outcome)?,
            snapshot: self.snapshot_for(Some(session))?,
        })
    }

    fn persist_session_summary(
        &mut self,
        session: &mut SessionRecord,
        outcome: &BaseTypeOutcome,
    ) -> SimardResult<()> {
        self.transition(RuntimeState::Persisting)?;
        session.advance(SessionPhase::Persistence)?;
        self.remember_session(session);

        let summary_key = format!("{}-summary", session.id);
        self.ports.memory_store.put(MemoryRecord {
            key: summary_key.clone(),
            scope: self.request.manifest.memory_policy.summary_scope,
            value: self
                .ports
                .agent_program
                .persistence_summary(&self.agent_program_context(session), outcome)?,
            session_id: session.id.clone(),
            recorded_in: SessionPhase::Persistence,
        })?;
        session.attach_memory(summary_key);
        self.remember_session(session);

        Ok(())
    }

    fn complete_session(
        &mut self,
        mut session: SessionRecord,
        outcome: BaseTypeOutcome,
        reflection: ReflectionReport,
    ) -> SimardResult<SessionOutcome> {
        session.advance(SessionPhase::Complete)?;
        self.remember_session(&session);
        self.transition(RuntimeState::Ready)?;

        Ok(SessionOutcome {
            session,
            plan: outcome.plan,
            execution_summary: outcome.execution_summary,
            reflection,
        })
    }

    fn mark_last_session_failed(&mut self) {
        if let Some(session) = self.last_session.as_mut()
            && session.phase != SessionPhase::Failed
        {
            session.phase = SessionPhase::Failed;
        }
    }

    fn agent_program_context(&self, session: &SessionRecord) -> AgentProgramContext {
        AgentProgramContext {
            session_id: session.id.clone(),
            identity_name: self.request.manifest.name.clone(),
            mode: session.mode,
            selected_base_type: self.request.selected_base_type.clone(),
            topology: self.request.topology,
            runtime_node: self.runtime_node.clone(),
            mailbox_address: self.mailbox_address.clone(),
            objective: session.objective.clone(),
        }
    }

    fn snapshot_for(&self, session: Option<&SessionRecord>) -> SimardResult<ReflectionSnapshot> {
        let evidence_records = match session {
            Some(active_session) => self
                .ports
                .evidence_store
                .count_for_session(&active_session.id)?,
            None => 0,
        };
        let memory_records = match session {
            Some(active_session) => self
                .ports
                .memory_store
                .count_for_session(&active_session.id)?,
            None => 0,
        };
        let manifest_freshness = match self.state {
            RuntimeState::Stopped | RuntimeState::Failed => {
                Freshness::observed(FreshnessState::Stale)?
            }
            _ => Freshness::observed(FreshnessState::Current)?,
        };

        Ok(ReflectionSnapshot {
            identity_name: self.request.manifest.name.clone(),
            selected_base_type: self.request.selected_base_type.clone(),
            topology: self.request.topology,
            runtime_state: self.state,
            runtime_node: self.runtime_node.clone(),
            mailbox_address: self.mailbox_address.clone(),
            session_phase: session.map(|active_session| active_session.phase),
            prompt_assets: self
                .prompt_assets
                .iter()
                .map(|asset| asset.id.clone())
                .collect(),
            manifest_contract: self
                .request
                .manifest
                .contract
                .with_freshness(manifest_freshness),
            evidence_records,
            memory_records,
            agent_program_backend: self.ports.agent_program.descriptor(),
            handoff_backend: self.ports.handoff_store.descriptor(),
            adapter_backend: self.factory.descriptor().backend.clone(),
            topology_backend: self.ports.topology_driver.descriptor(),
            transport_backend: self.ports.transport.descriptor(),
            supervisor_backend: self.ports.supervisor.descriptor(),
            memory_backend: self.ports.memory_store.descriptor(),
            evidence_backend: self.ports.evidence_store.descriptor(),
        })
    }
}

impl ReflectiveRuntime for RuntimeKernel {
    fn snapshot(&self) -> SimardResult<ReflectionSnapshot> {
        self.snapshot_for(self.last_session.as_ref())
    }
}
