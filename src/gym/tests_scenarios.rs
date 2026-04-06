use super::scenarios::*;
use super::types::{BenchmarkClass, BenchmarkScenario};
use crate::base_types::BaseTypeId;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::identity::ManifestContract;
use crate::identity::OperatingMode;
use crate::memory::{MemoryRecord, MemoryScope};
use crate::metadata::{BackendDescriptor, Freshness, Provenance};
use crate::reflection::{ReflectionReport, ReflectionSnapshot};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology, SessionOutcome};
use crate::session::{SessionId, SessionPhase, SessionRecord};
// --- benchmark_scenarios ---

#[test]
fn benchmark_scenarios_returns_nine_scenarios() {
    assert_eq!(benchmark_scenarios().len(), 9);
}

#[test]
fn benchmark_scenarios_all_have_unique_ids() {
    let scenarios = benchmark_scenarios();
    let mut ids: Vec<&str> = scenarios.iter().map(|s| s.id).collect();
    let original_len = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), original_len, "scenario ids must be unique");
}

#[test]
fn benchmark_scenarios_all_have_nonempty_fields() {
    for scenario in benchmark_scenarios() {
        assert!(!scenario.id.is_empty());
        assert!(!scenario.title.is_empty());
        assert!(!scenario.description.is_empty());
        assert!(!scenario.identity.is_empty());
        assert!(!scenario.base_type.is_empty());
        assert!(!scenario.objective.is_empty());
        assert!(scenario.expected_min_runtime_evidence > 0);
    }
}

#[test]
fn benchmark_scenarios_contains_known_ids() {
    let ids: Vec<&str> = benchmark_scenarios().iter().map(|s| s.id).collect();
    assert!(ids.contains(&"repo-exploration-local"));
    assert!(ids.contains(&"docs-refresh-copilot"));
    assert!(ids.contains(&"safe-code-change-rusty-clawd"));
    assert!(ids.contains(&"composite-session-review"));
    assert!(ids.contains(&"interactive-terminal-driving"));
}

#[test]
fn benchmark_scenarios_covers_all_classes() {
    let scenarios = benchmark_scenarios();
    let has_class = |class: BenchmarkClass| scenarios.iter().any(|s| s.class == class);
    assert!(has_class(BenchmarkClass::RepoExploration));
    assert!(has_class(BenchmarkClass::Documentation));
    assert!(has_class(BenchmarkClass::SafeCodeChange));
    assert!(has_class(BenchmarkClass::SessionQuality));
    assert!(has_class(BenchmarkClass::TestWriting));
}

// --- resolve_benchmark_scenario ---

#[test]
fn resolve_known_scenario() {
    let result = resolve_benchmark_scenario("repo-exploration-local");
    assert!(result.is_ok());
    let scenario = result.unwrap();
    assert_eq!(scenario.id, "repo-exploration-local");
    assert_eq!(scenario.class, BenchmarkClass::RepoExploration);
}

#[test]
fn resolve_unknown_scenario_returns_error() {
    let result = resolve_benchmark_scenario("nonexistent-scenario");
    assert!(result.is_err());
}

#[test]
fn resolve_each_known_scenario() {
    for scenario in benchmark_scenarios() {
        let resolved = resolve_benchmark_scenario(scenario.id).unwrap();
        assert_eq!(resolved, *scenario);
    }
}

// --- class_specific_checks helpers ---

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

fn dummy_outcome(
    plan: &str,
    execution_summary: &str,
    reflection_summary: &str,
) -> SessionOutcome {
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

// -- RepoExploration checks --

#[test]
fn class_checks_repo_exploration_passes_with_keywords() {
    let scenario = repo_exploration_scenario();
    let outcome = dummy_outcome(
        "inspect src/ directory structure",
        "found Cargo.toml dependencies and module layout",
        "identified main.rs entry point",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "repo-structure-discovered" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "repo-dependencies-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "repo-entry-points-found" && c.passed)
    );
}

#[test]
fn class_checks_repo_exploration_fails_without_keywords() {
    let scenario = repo_exploration_scenario();
    let outcome = dummy_outcome("nothing useful", "no content", "empty reflection");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

// -- Documentation checks --

#[test]
fn class_checks_documentation_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Documentation,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "generate /// doc comment for function",
        "produced rustdoc with parameter descriptions",
        "documentation covers return type",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "doc-comment-syntax-valid" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "doc-params-return-covered" && c.passed)
    );
}

#[test]
fn class_checks_documentation_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Documentation,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "no content here", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

// -- SafeCodeChange checks --

#[test]
fn class_checks_safe_code_change_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::SafeCodeChange,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "add derive attribute to struct",
        "cargo check compilation succeeded with no errors",
        "change described in diff",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "code-change-compilation-checked" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "code-change-described" && c.passed)
    );
}

#[test]
fn class_checks_safe_code_change_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::SafeCodeChange,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

// -- TestWriting checks --

#[test]
fn class_checks_test_writing_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::TestWriting,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "write #[test] function to call target",
        "unit test with assert_eq validates input/output",
        "test covers basic case and result verification",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "test-structure-valid" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "test-has-assertions" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "test-covers-basic-case" && c.passed)
    );
}

#[test]
fn class_checks_test_writing_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::TestWriting,
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

// -- SessionQuality checks --

#[test]
fn class_checks_session_quality_passes_with_nonempty_summary_and_enough_memory() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::SessionQuality,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("plan", "non-empty execution summary", "reflection");
    let exported = dummy_handoff(3);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 1);
    assert!(checks[0].passed);
    assert_eq!(checks[0].id, "session-quality-summary-adequate");
    assert!(checks[0].detail.contains("3 memory records"));
    assert!(checks[0].detail.contains("non-empty"));
}

#[test]
fn class_checks_session_quality_fails_with_empty_summary() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::SessionQuality,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("plan", "   ", "reflection");
    let exported = dummy_handoff(5);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 1);
    assert!(!checks[0].passed);
    assert!(checks[0].detail.contains("empty"));
}

#[test]
fn class_checks_session_quality_fails_with_insufficient_memory() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::SessionQuality,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("plan", "has content", "reflection");
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 1);
    assert!(!checks[0].passed);
}

// -- Edge cases for keyword detection --

#[test]
fn class_checks_repo_exploration_detects_module_keyword() {
    let scenario = repo_exploration_scenario();
    let outcome = dummy_outcome("found module layout", "bland", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "repo-structure-discovered")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_repo_exploration_detects_crate_for_deps() {
    let scenario = repo_exploration_scenario();
    let outcome = dummy_outcome("bland", "uses crate dependencies", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "repo-dependencies-identified")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_repo_exploration_detects_lib_rs_for_entry_points() {
    let scenario = repo_exploration_scenario();
    let outcome = dummy_outcome("bland", "bland", "found lib.rs");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "repo-entry-points-found")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_documentation_detects_doc_comment_keyword() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::Documentation,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("doc comment style", "bland", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "doc-comment-syntax-valid")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_safe_code_change_detects_cargo_build() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::SafeCodeChange,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("bland", "ran cargo build successfully", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "code-change-compilation-checked")
        .unwrap();
    assert!(check.passed);
}

#[test]
fn class_checks_test_writing_detects_expect_for_assertions() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::TestWriting,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("bland", "test uses expect to verify", "bland");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    let check = checks
        .iter()
        .find(|c| c.id == "test-has-assertions")
        .unwrap();
    assert!(check.passed);
}
