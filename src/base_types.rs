use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::prompt_assets::PromptAssetRef;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::sanitization::objective_metadata;
use crate::session::SessionId;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct BaseTypeId(String);

impl BaseTypeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for BaseTypeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Display for BaseTypeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum BaseTypeCapability {
    PromptAssets,
    SessionLifecycle,
    Memory,
    Evidence,
    Reflection,
}

impl Display for BaseTypeCapability {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::PromptAssets => "prompt-assets",
            Self::SessionLifecycle => "session-lifecycle",
            Self::Memory => "memory",
            Self::Evidence => "evidence",
            Self::Reflection => "reflection",
        };
        f.write_str(label)
    }
}

pub fn capability_set(
    capabilities: impl IntoIterator<Item = BaseTypeCapability>,
) -> BTreeSet<BaseTypeCapability> {
    capabilities.into_iter().collect()
}

fn standard_session_capabilities() -> BTreeSet<BaseTypeCapability> {
    capability_set([
        BaseTypeCapability::PromptAssets,
        BaseTypeCapability::SessionLifecycle,
        BaseTypeCapability::Memory,
        BaseTypeCapability::Evidence,
        BaseTypeCapability::Reflection,
    ])
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeDescriptor {
    pub id: BaseTypeId,
    pub backend: BackendDescriptor,
    pub capabilities: BTreeSet<BaseTypeCapability>,
    pub supported_topologies: BTreeSet<RuntimeTopology>,
}

impl BaseTypeDescriptor {
    pub fn supports_topology(&self, topology: RuntimeTopology) -> bool {
        self.supported_topologies.contains(&topology)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeSessionRequest {
    pub session_id: SessionId,
    pub mode: OperatingMode,
    pub topology: RuntimeTopology,
    pub prompt_assets: Vec<PromptAssetRef>,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeTurnInput {
    pub objective: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeOutcome {
    pub plan: String,
    pub execution_summary: String,
    pub evidence: Vec<String>,
}

pub trait BaseTypeSession: Send {
    fn descriptor(&self) -> &BaseTypeDescriptor;

    fn open(&mut self) -> SimardResult<()>;

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome>;

    fn close(&mut self) -> SimardResult<()>;
}

pub trait BaseTypeFactory: Send + Sync {
    fn descriptor(&self) -> &BaseTypeDescriptor;

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>>;
}

#[derive(Debug)]
pub struct LocalProcessHarnessAdapter {
    descriptor: BaseTypeDescriptor,
}

impl LocalProcessHarnessAdapter {
    pub fn new(
        id: impl Into<String>,
        implementation_identity: impl Into<String>,
        capabilities: impl IntoIterator<Item = BaseTypeCapability>,
        supported_topologies: impl IntoIterator<Item = RuntimeTopology>,
    ) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        let implementation_identity = implementation_identity.into();
        let backend = BackendDescriptor::for_runtime_type::<Self>(
            implementation_identity.clone(),
            format!("registered-base-type:{id}::implementation:{implementation_identity}"),
            Freshness::now()?,
        );
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend,
                capabilities: capability_set(capabilities),
                supported_topologies: supported_topologies.into_iter().collect(),
            },
        })
    }

    pub fn single_process(id: impl Into<String>) -> SimardResult<Self> {
        let id = id.into();
        Self::single_process_alias(id.clone(), id)
    }

    pub fn single_process_alias(
        id: impl Into<String>,
        implementation_identity: impl Into<String>,
    ) -> SimardResult<Self> {
        Self::new(
            id,
            implementation_identity,
            standard_session_capabilities(),
            [RuntimeTopology::SingleProcess],
        )
    }
}

impl BaseTypeFactory for LocalProcessHarnessAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        Ok(Box::new(LocalProcessHarnessSession {
            descriptor: self.descriptor.clone(),
            request,
            is_open: false,
            is_closed: false,
        }))
    }
}

#[derive(Debug)]
pub struct RustyClawdAdapter {
    descriptor: BaseTypeDescriptor,
}

impl RustyClawdAdapter {
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "rusty-clawd::session-backend",
                    "registered-base-type:rusty-clawd",
                    Freshness::now()?,
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [
                    RuntimeTopology::SingleProcess,
                    RuntimeTopology::MultiProcess,
                ]
                .into_iter()
                .collect(),
            },
        })
    }
}

impl BaseTypeFactory for RustyClawdAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        Ok(Box::new(RustyClawdSession {
            descriptor: self.descriptor.clone(),
            request,
            is_open: false,
            is_closed: false,
        }))
    }
}

#[derive(Debug)]
struct RustyClawdSession {
    descriptor: BaseTypeDescriptor,
    request: BaseTypeSessionRequest,
    is_open: bool,
    is_closed: bool,
}

impl RustyClawdSession {
    fn ensure_can(&self, action: &str) -> SimardResult<()> {
        if self.is_closed {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: action.to_string(),
                reason: "session is already closed".to_string(),
            });
        }

        Ok(())
    }
}

impl BaseTypeSession for RustyClawdSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        self.ensure_can("open")?;
        if self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "open".to_string(),
                reason: "session is already open".to_string(),
            });
        }
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        self.ensure_can("run_turn")?;
        if !self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "run_turn".to_string(),
                reason: "session must be opened before turns can run".to_string(),
            });
        }

        let prompt_ids = self
            .request
            .prompt_assets
            .iter()
            .map(|asset| asset.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let objective_summary = objective_metadata(&input.objective);

        Ok(BaseTypeOutcome {
            plan: format!(
                "Launch RustyClawd backend '{}' for '{}' on '{}' and bind prompt assets [{}] with {}.",
                self.descriptor.backend.identity,
                self.request.mode,
                self.request.topology,
                prompt_ids,
                objective_summary
            ),
            execution_summary: format!(
                "RustyClawd session backend executed {} via selected base type '{}' on implementation '{}' from node '{}' at '{}'.",
                objective_summary,
                self.descriptor.id,
                self.descriptor.backend.identity,
                self.request.runtime_node,
                self.request.mailbox_address,
            ),
            evidence: vec![
                format!("selected-base-type={}", self.descriptor.id),
                format!(
                    "backend-implementation={}",
                    self.descriptor.backend.identity
                ),
                format!("prompt-assets=[{}]", prompt_ids),
                format!("runtime-node={}", self.request.runtime_node),
                format!("mailbox-address={}", self.request.mailbox_address),
            ],
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        self.ensure_can("close")?;
        if !self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "close".to_string(),
                reason: "session was never opened".to_string(),
            });
        }
        self.is_closed = true;
        Ok(())
    }
}

#[derive(Debug)]
struct LocalProcessHarnessSession {
    descriptor: BaseTypeDescriptor,
    request: BaseTypeSessionRequest,
    is_open: bool,
    is_closed: bool,
}

impl LocalProcessHarnessSession {
    fn ensure_can(&self, action: &str) -> SimardResult<()> {
        if self.is_closed {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: action.to_string(),
                reason: "session is already closed".to_string(),
            });
        }

        Ok(())
    }
}

impl BaseTypeSession for LocalProcessHarnessSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        self.ensure_can("open")?;
        if self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "open".to_string(),
                reason: "session is already open".to_string(),
            });
        }
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        self.ensure_can("run_turn")?;
        if !self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "run_turn".to_string(),
                reason: "session must be opened before turns can run".to_string(),
            });
        }

        let prompt_ids = self
            .request
            .prompt_assets
            .iter()
            .map(|asset| asset.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let objective_summary = objective_metadata(&input.objective);
        let implementation_identity = &self.descriptor.backend.identity;

        Ok(BaseTypeOutcome {
            plan: format!(
                "Open '{}' session on '{}' via {} and run prompt assets [{}].",
                self.request.mode, self.request.topology, objective_summary, prompt_ids
            ),
            execution_summary: format!(
                "Local single-process harness session executed {} via selected base type '{}' on implementation '{}' from node '{}' at '{}'.",
                objective_summary,
                self.descriptor.id,
                implementation_identity,
                self.request.runtime_node,
                self.request.mailbox_address,
            ),
            evidence: vec![
                format!("selected-base-type={}", self.descriptor.id),
                format!("prompt-assets=[{}]", prompt_ids),
                format!("runtime-node={}", self.request.runtime_node),
                format!("mailbox-address={}", self.request.mailbox_address),
            ],
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        self.ensure_can("close")?;
        if !self.is_open {
            return Err(SimardError::InvalidBaseTypeSessionState {
                base_type: self.descriptor.id.to_string(),
                action: "close".to_string(),
                reason: "session was never opened".to_string(),
            });
        }
        self.is_closed = true;
        Ok(())
    }
}
