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
fn class_checks_bug_fix_detects_panic_keyword() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::BugFix,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("found a panic in production code", "bland", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "bug-defect-identified")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_bug_fix_detects_convert_for_fix() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::BugFix,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("bland", "convert the call to use ? operator", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks.iter().find(|c| c.id == "bug-fix-described").unwrap();
    assert!(check.passed);
}

// -- Refactoring checks --

#[test]
fn class_checks_refactoring_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Refactoring,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "extract helper fn from long function",
        "refactored code preserves original behavior",
        "before and after code shown, equivalent output",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "refactor-change-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "refactor-behavior-preserved" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "refactor-code-shown" && c.passed)
    );
}

#[test]
fn class_checks_refactoring_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Refactoring,
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
fn class_checks_refactoring_detects_simplify_keyword() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Refactoring,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("simplified the match expression", "bland", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "refactor-change-identified")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_refactoring_detects_preserve_keyword() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Refactoring,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("bland", "bland", "behavior is preserved after change");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "refactor-behavior-preserved")
        .unwrap();
    assert!(check.passed);
}

// -- Coverage distribution checks --

#[test]
fn benchmark_scenarios_each_class_has_at_least_two() {
    let scenarios = benchmark_scenarios();
    let count = |class: BenchmarkClass| scenarios.iter().filter(|s| s.class == class).count();
    assert!(count(BenchmarkClass::RepoExploration) >= 2);
    assert!(count(BenchmarkClass::Documentation) >= 2);
    assert!(count(BenchmarkClass::SafeCodeChange) >= 2);
    assert!(count(BenchmarkClass::SessionQuality) >= 2);
    assert!(count(BenchmarkClass::TestWriting) >= 2);
    assert!(count(BenchmarkClass::BugFix) >= 2);
    assert!(count(BenchmarkClass::Refactoring) >= 2);
    assert!(count(BenchmarkClass::DependencyAnalysis) >= 2);
    assert!(count(BenchmarkClass::ErrorHandling) >= 2);
    assert!(count(BenchmarkClass::DataMigration) >= 2);
    assert!(count(BenchmarkClass::CicdPipeline) >= 2);
    assert!(count(BenchmarkClass::DependencyUpgrade) >= 2);
    assert!(count(BenchmarkClass::ReleaseManagement) >= 2);
    assert!(count(BenchmarkClass::AccessibilityReview) >= 2);
    assert!(count(BenchmarkClass::InternationalizationReview) >= 2);
    assert!(count(BenchmarkClass::IncidentResponse) >= 2);
}

#[test]
fn benchmark_scenarios_multi_process_coverage() {
    let multi = benchmark_scenarios()
        .iter()
        .filter(|s| s.topology == RuntimeTopology::MultiProcess)
        .count();
    assert!(
        multi >= 2,
        "need at least 2 MultiProcess scenarios, got {multi}"
    );
}

#[test]
fn benchmark_scenarios_copilot_sdk_coverage() {
    let copilot = benchmark_scenarios()
        .iter()
        .filter(|s| s.base_type == "copilot-sdk")
        .count();
    assert!(
        copilot >= 2,
        "need at least 2 copilot-sdk scenarios, got {copilot}"
    );
}

// -- DependencyAnalysis checks --

#[test]
fn class_checks_dependency_analysis_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::DependencyAnalysis,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "analyzed cargo.toml dependencies and crate versions",
        "import coupling between modules assessed",
        "suggest decoupling the most coupled module",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "dep-analysis-performed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "dep-coupling-assessed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "dep-recommendations-present" && c.passed)
    );
}

#[test]
fn class_checks_dependency_analysis_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::DependencyAnalysis,
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

// -- ErrorHandling checks --

#[test]
fn class_checks_error_handling_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::ErrorHandling,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "found unwrap calls that could panic in production",
        "classified as safe or risky based on context",
        "error propagation chain traced through diagnostic output",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "error-analysis-performed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "error-classification-present" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "error-propagation-traced" && c.passed)
    );
}

#[test]
fn class_checks_error_handling_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::ErrorHandling,
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

// -- Distributed topology coverage --
