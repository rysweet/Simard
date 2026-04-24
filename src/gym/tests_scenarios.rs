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
    assert_eq!(benchmark_scenarios().len(), 151);
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

// -- BugFix checks --

#[test]
fn class_checks_bug_fix_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::BugFix,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "found unwrap bug in error handling path",
        "fix: replace expect with Result propagation for safety",
        "the panic was unsafe, now uses graceful error recovery",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "bug-defect-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "bug-fix-described" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "bug-safety-analyzed" && c.passed)
    );
}

#[test]
fn class_checks_bug_fix_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::BugFix,
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

#[test]
fn class_checks_release_management_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::ReleaseManagement,
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

// -- Wave 8: AccessibilityReview / InternationalizationReview / IncidentResponse --

#[test]
fn class_checks_accessibility_review_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::AccessibilityReview,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "audit aria roles, missing alt text, label association, and keyboard focus",
        "cite WCAG 2.1.1 and 4.1.2 success criterion at level AA",
        "remediation: add aria-label, fix focus order, replace contrast pair",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "a11y-issues-identified" && c.passed)
    );
    assert!(checks.iter().any(|c| c.id == "a11y-wcag-cited" && c.passed));
    assert!(
        checks
            .iter()
            .any(|c| c.id == "a11y-remediation-proposed" && c.passed)
    );
}

#[test]
fn class_checks_accessibility_review_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::AccessibilityReview,
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
fn class_checks_internationalization_review_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::InternationalizationReview,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "inventory hardcoded string literals and route to message catalog",
        "negotiate locale via Accept-Language with CLDR fallback (en-US, pt-BR)",
        "address plural categories, RTL/bidi mirroring, and date format via ICU MessageFormat",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "i18n-localizable-strings-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "i18n-locale-handling-described" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "i18n-pluralization-or-format-addressed" && c.passed)
    );
}

#[test]
fn class_checks_internationalization_review_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::InternationalizationReview,
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
fn class_checks_incident_response_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::IncidentResponse,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "reconstruct timeline: alert paged, mitigation started, resolved at",
        "blameless analysis: root cause distinct from trigger, latent contributing factor",
        "follow-up runbook with on-call escalation and prevention action item",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "incident-timeline-reconstructed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "incident-root-cause-or-contributing-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "incident-mitigation-or-followup-proposed" && c.passed)
    );
}

#[test]
fn class_checks_incident_response_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::IncidentResponse,
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
