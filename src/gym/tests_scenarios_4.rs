use super::scenarios::*;
use super::types::{BenchmarkClass, BenchmarkScenario};
use crate::base_types::BaseTypeId;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::identity::ManifestContract;
use crate::identity::OperatingMode;
use crate::memory::{MemoryRecord, MemoryScope};
use crate::metadata::{BackendDescriptor, Freshness, Provenance};
use crate::reflection::{ReflectionReport, ReflectionSnapshot};
use crate::runtime::{
    RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology, SessionOutcome,
};
use crate::session::{SessionId, SessionPhase, SessionRecord};
fn dummy_backend() -> BackendDescriptor {
    BackendDescriptor {
        identity: "test-backend".to_string(),
        provenance: Provenance::new("test-src", "test::loc"),
        freshness: Freshness::now().unwrap(),
    }
}

fn dummy_contract() -> ManifestContract {
    ManifestContract {
        entrypoint: "test::entry".to_string(),
        composition: "a -> b".to_string(),
        precedence: vec!["tag:value".to_string()],
        provenance: Provenance::new("test-src", "test::loc"),
        freshness: Freshness::now().unwrap(),
    }
}

fn dummy_snapshot() -> ReflectionSnapshot {
    let backend = dummy_backend();
    ReflectionSnapshot {
        identity_name: "test".to_string(),
        identity_components: vec![],
        selected_base_type: BaseTypeId::new("test"),
        topology: RuntimeTopology::SingleProcess,
        runtime_state: RuntimeState::Ready,
        runtime_node: RuntimeNodeId::local(),
        mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session_phase: Some(SessionPhase::Complete),
        prompt_assets: vec![],
        manifest_contract: dummy_contract(),
        evidence_records: 0,
        memory_records: 0,
        active_goal_count: 0,
        active_goals: vec![],
        proposed_goal_count: 0,
        proposed_goals: vec![],
        agent_program_backend: backend.clone(),
        handoff_backend: backend.clone(),
        adapter_backend: backend.clone(),
        adapter_capabilities: vec![],
        adapter_supported_topologies: vec![],
        topology_backend: backend.clone(),
        transport_backend: backend.clone(),
        supervisor_backend: backend.clone(),
        memory_backend: backend.clone(),
        evidence_backend: backend.clone(),
        goal_backend: backend,
    }
}

fn dummy_outcome(plan: &str, execution_summary: &str, reflection_summary: &str) -> SessionOutcome {
    SessionOutcome {
        session: SessionRecord {
            id: SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap(),
            mode: OperatingMode::Gym,
            objective: "test".to_string(),
            phase: SessionPhase::Complete,
            selected_base_type: BaseTypeId::new("test"),
            evidence_ids: vec![],
            memory_keys: vec![],
        },
        plan: plan.to_string(),
        execution_summary: execution_summary.to_string(),
        reflection: ReflectionReport {
            summary: reflection_summary.to_string(),
            snapshot: dummy_snapshot(),
        },
    }
}

fn dummy_handoff(memory_count: usize) -> RuntimeHandoffSnapshot {
    let session_id = SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap();
    RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Stopped,
        identity_name: "test".to_string(),
        selected_base_type: BaseTypeId::new("test"),
        topology: RuntimeTopology::SingleProcess,
        source_runtime_node: RuntimeNodeId::local(),
        source_mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session: None,
        memory_records: (0..memory_count)
            .map(|i| MemoryRecord {
                key: format!("key-{i}"),
                scope: MemoryScope::Benchmark,
                value: format!("value-{i}"),
                session_id: session_id.clone(),
                recorded_in: SessionPhase::Complete,
                created_at: None,
            })
            .collect(),
        evidence_records: vec![],
        copilot_submit_audit: None,
    }
}

fn repo_exploration_scenario() -> BenchmarkScenario {
    BenchmarkScenario {
        id: "test-repo-exp",
        title: "Test Repo Exploration",
        description: "test",
        class: BenchmarkClass::RepoExploration,
        identity: "test",
        base_type: "test",
        topology: RuntimeTopology::SingleProcess,
        objective: "test",
        expected_min_runtime_evidence: 1,
    }
}

#[test]
fn benchmark_scenarios_distributed_topology_coverage() {
    let distributed = benchmark_scenarios()
        .iter()
        .filter(|s| s.topology == RuntimeTopology::Distributed)
        .count();
    assert!(
        distributed >= 1,
        "need at least 1 Distributed scenario, got {distributed}"
    );
}

// -- Wave 7: DataMigration / CicdPipeline / DependencyUpgrade / ReleaseManagement --

#[test]
fn class_checks_data_migration_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::DataMigration,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "schema delta adds optional field with serde default",
        "backward compatibility preserved during phased rollout",
        "rollback path documented if migration must be reverted",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "data-migration-schema-delta-described" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "data-migration-compatibility-addressed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "data-migration-rollout-or-rollback-planned" && c.passed)
    );
}

#[test]
fn class_checks_data_migration_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::DataMigration,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

#[test]
fn class_checks_cicd_pipeline_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::CicdPipeline,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "drafted github actions workflow .yml with build job and steps",
        "trigger on pull_request and push, pin uses: actions/checkout@v4",
        "runs cargo check and cargo test with cache and matrix",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "cicd-workflow-structure-described" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "cicd-trigger-or-pin-addressed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "cicd-verification-or-remediation-present" && c.passed)
    );
}

#[test]
fn class_checks_cicd_pipeline_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::CicdPipeline,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

#[test]
fn class_checks_dependency_upgrade_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::DependencyUpgrade,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "plan major version bump of crate in cargo.toml",
        "changelog lists breaking api change at call site",
        "verify with cargo check and cargo test, staged rollout with rollback",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "dep-upgrade-target-named" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "dep-upgrade-breakage-analyzed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "dep-upgrade-verification-plan-present" && c.passed)
    );
}

#[test]
fn class_checks_dependency_upgrade_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::DependencyUpgrade,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

#[test]
fn class_checks_release_management_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::ReleaseManagement,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "semver minor version bump in cargo.toml",
        "changelog grouped by added/changed/fixed for release notes",
        "git tag and publish, with rollback path",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "release-version-bump-planned" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "release-changelog-authored" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "release-tag-or-cutover-addressed" && c.passed)
    );
}
