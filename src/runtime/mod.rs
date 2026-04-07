mod ports;
mod session;
mod traits;
mod types;

#[cfg(test)]
mod tests_mod;

pub use ports::*;
pub use traits::*;
pub use types::*;

use std::sync::Arc;
use std::time::Instant;

use crate::base_types::BaseTypeFactory;
use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::prompt_assets::PromptAssetRef;
use crate::session::SessionRecord;

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
    /// Optional cognitive memory bridge for consolidation lifecycle hooks.
    cognitive_bridge: Option<CognitiveMemoryBridge>,
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
            cognitive_bridge: None,
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

    /// Attach a cognitive memory bridge for session lifecycle consolidation.
    pub fn set_cognitive_bridge(&mut self, bridge: CognitiveMemoryBridge) {
        self.cognitive_bridge = Some(bridge);
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
