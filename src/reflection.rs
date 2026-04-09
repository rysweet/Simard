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
    pub active_goal_count: usize,
    pub active_goals: Vec<String>,
    pub proposed_goal_count: usize,
    pub proposed_goals: Vec<String>,
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
    pub goal_backend: BackendDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflectionReport {
    pub summary: String,
    pub snapshot: ReflectionSnapshot,
}

pub trait ReflectiveRuntime {
    fn snapshot(&self) -> SimardResult<ReflectionSnapshot>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Freshness, Provenance};

    fn test_backend() -> BackendDescriptor {
        BackendDescriptor::new(
            "test-backend",
            Provenance::builtin("test"),
            Freshness::now().unwrap(),
        )
    }

    fn test_snapshot() -> ReflectionSnapshot {
        ReflectionSnapshot {
            identity_name: "test-id".to_string(),
            identity_components: vec!["c1".to_string()],
            selected_base_type: BaseTypeId::new("bt"),
            topology: RuntimeTopology::SingleProcess,
            runtime_state: RuntimeState::Active,
            runtime_node: RuntimeNodeId::new("node-1"),
            mailbox_address: RuntimeAddress::new("addr-1"),
            session_phase: Some(SessionPhase::Execution),
            prompt_assets: vec![],
            manifest_contract: ManifestContract::new(
                "module::entry",
                "a -> b",
                vec!["layer:core".to_string()],
                Provenance::builtin("test"),
                Freshness::now().unwrap(),
            )
            .unwrap(),
            evidence_records: 5,
            memory_records: 10,
            active_goal_count: 2,
            active_goals: vec!["g1".to_string()],
            proposed_goal_count: 1,
            proposed_goals: vec!["g2".to_string()],
            agent_program_backend: test_backend(),
            handoff_backend: test_backend(),
            adapter_backend: test_backend(),
            adapter_capabilities: vec!["cap1".to_string()],
            adapter_supported_topologies: vec!["single".to_string()],
            topology_backend: test_backend(),
            transport_backend: test_backend(),
            supervisor_backend: test_backend(),
            memory_backend: test_backend(),
            evidence_backend: test_backend(),
            goal_backend: test_backend(),
        }
    }

    #[test]
    fn reflection_snapshot_construction() {
        let snap = test_snapshot();
        assert_eq!(snap.identity_name, "test-id");
        assert_eq!(snap.evidence_records, 5);
        assert_eq!(snap.memory_records, 10);
        assert_eq!(snap.active_goal_count, 2);
    }

    #[test]
    fn reflection_report_construction() {
        let report = ReflectionReport {
            summary: "all good".to_string(),
            snapshot: test_snapshot(),
        };
        assert_eq!(report.summary, "all good");
        assert_eq!(report.snapshot.identity_name, "test-id");
    }

    #[test]
    fn reflection_snapshot_equality() {
        let a = test_snapshot();
        let b = a.clone();
        assert_eq!(a, b);
    }
}
