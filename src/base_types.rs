use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::metadata::{BackendDescriptor, Freshness, Provenance};
use crate::prompt_assets::PromptAssetRef;
use crate::runtime::RuntimeTopology;
use crate::session::SessionId;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
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
pub struct BaseTypeRequest {
    pub session_id: SessionId,
    pub objective: String,
    pub mode: OperatingMode,
    pub topology: RuntimeTopology,
    pub prompt_assets: Vec<PromptAssetRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeOutcome {
    pub plan: String,
    pub execution_summary: String,
    pub evidence: Vec<String>,
}

pub trait BaseTypeAdapter: Send + Sync {
    fn descriptor(&self) -> &BaseTypeDescriptor;

    fn invoke(&self, request: BaseTypeRequest) -> SimardResult<BaseTypeOutcome>;
}

#[derive(Debug)]
pub struct LocalProcessHarnessAdapter {
    descriptor: BaseTypeDescriptor,
}

impl LocalProcessHarnessAdapter {
    pub fn new(
        id: impl Into<String>,
        capabilities: impl IntoIterator<Item = BaseTypeCapability>,
        supported_topologies: impl IntoIterator<Item = RuntimeTopology>,
    ) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        let backend = BackendDescriptor::new(
            id.to_string(),
            Provenance::injected(format!("base-type-registry:{}", id)),
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
        Self::new(
            id,
            [
                BaseTypeCapability::PromptAssets,
                BaseTypeCapability::SessionLifecycle,
                BaseTypeCapability::Memory,
                BaseTypeCapability::Evidence,
                BaseTypeCapability::Reflection,
            ],
            [RuntimeTopology::SingleProcess],
        )
    }
}

impl BaseTypeAdapter for LocalProcessHarnessAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn invoke(&self, request: BaseTypeRequest) -> SimardResult<BaseTypeOutcome> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        let prompt_ids = request
            .prompt_assets
            .iter()
            .map(|asset| asset.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        Ok(BaseTypeOutcome {
            plan: format!(
                "Run {:?} session '{}' with prompt assets [{}].",
                request.mode, request.objective, prompt_ids
            ),
            execution_summary: format!(
                "Local single-process harness executed '{}' via '{}'.",
                request.objective, self.descriptor.id
            ),
            evidence: vec![
                format!("selected-base-type={}", self.descriptor.id),
                format!("prompt-assets=[{}]", prompt_ids),
            ],
        })
    }
}
