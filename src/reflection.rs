use crate::base_types::BaseTypeId;
use crate::error::SimardResult;
use crate::identity::ManifestContract;
use crate::metadata::BackendDescriptor;
use crate::prompt_assets::PromptAssetId;
use crate::runtime::{RuntimeState, RuntimeTopology};
use crate::session::SessionPhase;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflectionSnapshot {
    pub identity_name: String,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_state: RuntimeState,
    pub session_phase: Option<SessionPhase>,
    pub prompt_assets: Vec<PromptAssetId>,
    pub manifest_contract: ManifestContract,
    pub evidence_records: usize,
    pub memory_records: usize,
    pub adapter_backend: BackendDescriptor,
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
