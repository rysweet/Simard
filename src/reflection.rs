use crate::base_types::BaseTypeId;
use crate::error::SimardResult;
use crate::identity::ManifestContract;
use crate::metadata::BackendDescriptor;
use crate::prompt_assets::PromptAssetId;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::session::SessionPhase;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflectionSnapshot {
    pub identity_name: String,
    pub identity_components: Vec<String>,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_state: RuntimeState,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
    pub session_phase: Option<SessionPhase>,
    pub prompt_assets: Vec<PromptAssetId>,
    pub manifest_contract: ManifestContract,
    pub evidence_records: usize,
    pub memory_records: usize,
    pub agent_program_backend: BackendDescriptor,
    pub handoff_backend: BackendDescriptor,
    pub adapter_backend: BackendDescriptor,
    pub adapter_capabilities: Vec<String>,
    pub adapter_supported_topologies: Vec<String>,
    pub topology_backend: BackendDescriptor,
    pub transport_backend: BackendDescriptor,
    pub supervisor_backend: BackendDescriptor,
    pub memory_backend: BackendDescriptor,
    pub evidence_backend: BackendDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflectionReport {
    pub summary: String,
    pub snapshot: ReflectionSnapshot,
}

pub trait ReflectiveRuntime {
    fn snapshot(&self) -> SimardResult<ReflectionSnapshot>;
}
