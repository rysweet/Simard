use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use crate::base_types::{BaseTypeAdapter, BaseTypeId, BaseTypeRequest};
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore};
use crate::identity::IdentityManifest;
use crate::memory::{MemoryRecord, MemoryScope, MemoryStore};
use crate::prompt_assets::{PromptAssetRef, PromptAssetStore};
use crate::reflection::{ReflectionReport, ReflectionSnapshot, ReflectiveRuntime};
use crate::session::{SessionIdGenerator, SessionPhase, SessionRecord, UuidSessionIdGenerator};

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
                | (Self::Ready, Self::Active)
                | (Self::Active, Self::Reflecting)
                | (Self::Reflecting, Self::Persisting)
                | (Self::Persisting, Self::Ready)
                | (Self::Ready, Self::Stopping)
                | (Self::Failed, Self::Stopping)
                | (Self::Stopping, Self::Stopped)
                | (_, Self::Failed)
        )
    }
}

#[derive(Default)]
pub struct BaseTypeRegistry {
    adapters: BTreeMap<BaseTypeId, Arc<dyn BaseTypeAdapter>>,
}

impl BaseTypeRegistry {
    pub fn register<A>(&mut self, adapter: A)
    where
        A: BaseTypeAdapter + 'static,
    {
        self.adapters
            .insert(adapter.descriptor().id.clone(), Arc::new(adapter));
    }

    pub fn get(&self, id: &BaseTypeId) -> Option<Arc<dyn BaseTypeAdapter>> {
        self.adapters.get(id).map(Arc::clone)
    }
}

pub struct RuntimePorts {
    prompt_store: Arc<dyn PromptAssetStore>,
    memory_store: Arc<dyn MemoryStore>,
    evidence_store: Arc<dyn EvidenceStore>,
    base_types: BaseTypeRegistry,
    session_ids: Arc<dyn SessionIdGenerator>,
}

impl RuntimePorts {
    pub fn new(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        base_types: BaseTypeRegistry,
    ) -> Self {
        Self::with_session_ids(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        )
    }

    pub fn with_session_ids(
        prompt_store: Arc<dyn PromptAssetStore>,
        memory_store: Arc<dyn MemoryStore>,
        evidence_store: Arc<dyn EvidenceStore>,
        base_types: BaseTypeRegistry,
        session_ids: Arc<dyn SessionIdGenerator>,
    ) -> Self {
        Self {
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
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

pub struct LocalRuntime {
    ports: RuntimePorts,
    request: RuntimeRequest,
    state: RuntimeState,
    adapter: Arc<dyn BaseTypeAdapter>,
    prompt_assets: Vec<PromptAssetRef>,
    last_session: Option<SessionRecord>,
}

impl LocalRuntime {
    pub fn compose(ports: RuntimePorts, request: RuntimeRequest) -> SimardResult<Self> {
        if !request
            .manifest
            .supports_base_type(&request.selected_base_type)
        {
            return Err(SimardError::UnsupportedBaseType {
                identity: request.manifest.name.clone(),
                base_type: request.selected_base_type.to_string(),
            });
        }

        let adapter = ports
            .base_types
            .get(&request.selected_base_type)
            .ok_or_else(|| SimardError::AdapterNotRegistered {
                base_type: request.selected_base_type.to_string(),
            })?;

        let descriptor = adapter.descriptor();
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

        Ok(Self {
            ports,
            request,
            state: RuntimeState::Initializing,
            adapter,
            prompt_assets: Vec::new(),
            last_session: None,
        })
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn start(&mut self) -> SimardResult<()> {
        self.request
            .manifest
            .prompt_assets
            .iter()
            .try_for_each(|reference| self.ports.prompt_store.load(reference).map(|_| ()))?;
        self.prompt_assets = self.request.manifest.prompt_assets.clone();

        self.transition(RuntimeState::Ready)
    }

    pub fn stop(&mut self) -> SimardResult<()> {
        self.transition(RuntimeState::Stopping)?;
        self.transition(RuntimeState::Stopped)
    }

    pub fn run(&mut self, objective: impl Into<String>) -> SimardResult<SessionOutcome> {
        let result = (|| {
            self.transition(RuntimeState::Active)?;

            let mut session = SessionRecord::new(
                self.request.manifest.default_mode,
                objective.into(),
                self.request.selected_base_type.clone(),
                self.ports.session_ids.as_ref(),
            );

            session.advance(SessionPhase::Preparation)?;
            let scratch_key = format!("{}-scratch", session.id);
            self.ports.memory_store.put(MemoryRecord {
                key: scratch_key.clone(),
                scope: MemoryScope::SessionScratch,
                value: format!("objective={}", session.objective),
                session_id: session.id.clone(),
                recorded_in: SessionPhase::Preparation,
            })?;
            session.attach_memory(scratch_key);

            session.advance(SessionPhase::Planning)?;
            let outcome = self.adapter.invoke(BaseTypeRequest {
                session_id: session.id.clone(),
                objective: session.objective.clone(),
                mode: session.mode,
                topology: self.request.topology,
                prompt_assets: self.prompt_assets.clone(),
            })?;

            session.advance(SessionPhase::Execution)?;
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

            self.transition(RuntimeState::Reflecting)?;
            session.advance(SessionPhase::Reflection)?;
            let reflection = ReflectionReport {
                summary: format!(
                    "Session '{}' completed through '{}' on '{}'.",
                    session.objective, self.request.selected_base_type, self.request.topology
                ),
                snapshot: self.snapshot_for(Some(&session))?,
            };

            self.transition(RuntimeState::Persisting)?;
            session.advance(SessionPhase::Persistence)?;
            let summary_key = format!("{}-summary", session.id);
            self.ports.memory_store.put(MemoryRecord {
                key: summary_key.clone(),
                scope: self.request.manifest.memory_policy.summary_scope,
                value: format!("{} | {}", outcome.plan, outcome.execution_summary),
                session_id: session.id.clone(),
                recorded_in: SessionPhase::Persistence,
            })?;
            session.attach_memory(summary_key);

            session.advance(SessionPhase::Complete)?;
            self.transition(RuntimeState::Ready)?;
            self.last_session = Some(session.clone());

            Ok(SessionOutcome {
                session,
                plan: outcome.plan,
                execution_summary: outcome.execution_summary,
                reflection,
            })
        })();

        if result.is_err() {
            self.state = RuntimeState::Failed;
        }

        result
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

    fn snapshot_for(&self, session: Option<&SessionRecord>) -> SimardResult<ReflectionSnapshot> {
        let evidence_records = match session {
            Some(active_session) => self
                .ports
                .evidence_store
                .list_for_session(&active_session.id)?
                .len(),
            None => 0,
        };
        let memory_records = match session {
            Some(active_session) => self
                .ports
                .memory_store
                .list_for_session(&active_session.id)?
                .len(),
            None => 0,
        };

        Ok(ReflectionSnapshot {
            identity_name: self.request.manifest.name.clone(),
            selected_base_type: self.request.selected_base_type.clone(),
            topology: self.request.topology,
            runtime_state: self.state,
            session_phase: session.map(|active_session| active_session.phase),
            prompt_assets: self
                .prompt_assets
                .iter()
                .map(|asset| asset.id.clone())
                .collect(),
            manifest_contract: self.request.manifest.contract.clone(),
            manifest_provenance: self.request.manifest.provenance.clone(),
            manifest_freshness: self.request.manifest.freshness,
            evidence_records,
            memory_records,
            memory_backend: self.ports.memory_store.descriptor(),
            evidence_backend: self.ports.evidence_store.descriptor(),
        })
    }
}

impl ReflectiveRuntime for LocalRuntime {
    fn snapshot(&self) -> SimardResult<ReflectionSnapshot> {
        self.snapshot_for(self.last_session.as_ref())
    }
}
