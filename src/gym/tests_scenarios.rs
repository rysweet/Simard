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
// --- benchmark_scenarios ---

#[test]
fn benchmark_scenarios_returns_nine_scenarios() {
    assert_eq!(benchmark_scenarios().len(), 155);
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
    assert!(ids.contains(&"doc-generation-multi-process"));
    assert!(ids.contains(&"bug-fix-distributed"));
    assert!(ids.contains(&"dep-analysis-cargo-audit"));
    assert!(ids.contains(&"dep-analysis-module-coupling"));
    assert!(ids.contains(&"error-handling-unwrap-audit"));
    assert!(ids.contains(&"error-handling-propagation-chain"));
    assert!(ids.contains(&"a11y-aria-audit-local"));
    assert!(ids.contains(&"a11y-keyboard-nav-multiprocess-copilot"));
    assert!(ids.contains(&"a11y-color-contrast-distributed-terminal"));
    assert!(ids.contains(&"i18n-string-extraction-local"));
    assert!(ids.contains(&"i18n-locale-routing-multiprocess-rusty-clawd"));
    assert!(ids.contains(&"i18n-pluralization-rtl-distributed-copilot"));
    assert!(ids.contains(&"incident-response-postmortem-local"));
    assert!(ids.contains(&"incident-response-runbook-multiprocess-terminal"));
    assert!(ids.contains(&"incident-response-pager-rotation-distributed-copilot"));
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
    assert!(has_class(BenchmarkClass::BugFix));
    assert!(has_class(BenchmarkClass::Refactoring));
    assert!(has_class(BenchmarkClass::DependencyAnalysis));
    assert!(has_class(BenchmarkClass::ErrorHandling));
    assert!(has_class(BenchmarkClass::PerformanceAnalysis));
    assert!(has_class(BenchmarkClass::SecurityAudit));
    assert!(has_class(BenchmarkClass::ApiDesign));
    assert!(has_class(BenchmarkClass::CodeReview));
    assert!(has_class(BenchmarkClass::Debugging));
    assert!(has_class(BenchmarkClass::ConfigManagement));
    assert!(has_class(BenchmarkClass::ConcurrencyAnalysis));
    assert!(has_class(BenchmarkClass::MigrationPlanning));
    assert!(has_class(BenchmarkClass::ObservabilityInstrumentation));
    assert!(has_class(BenchmarkClass::DataModeling));
    assert!(has_class(BenchmarkClass::DataMigration));
    assert!(has_class(BenchmarkClass::CicdPipeline));
    assert!(has_class(BenchmarkClass::DependencyUpgrade));
    assert!(has_class(BenchmarkClass::ReleaseManagement));
    assert!(has_class(BenchmarkClass::AccessibilityReview));
    assert!(has_class(BenchmarkClass::InternationalizationReview));
    assert!(has_class(BenchmarkClass::IncidentResponse));
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
