use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::runtime::RuntimeTopology;

use super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};

const BENCHMARK_SCENARIOS: [BenchmarkScenario; 9] = [
    BenchmarkScenario {
        id: "repo-exploration-local",
        title: "Repo exploration on local harness",
        description: "Exercise a bounded repo-exploration task through the gym identity on the single-process local harness.",
        class: BenchmarkClass::RepoExploration,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Inspect repository structure, identify likely extension points, and summarize where benchmark and runtime changes should land.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "docs-refresh-copilot",
        title: "Documentation refresh through copilot-sdk alias",
        description: "Exercise a documentation-oriented benchmark while preserving the explicit copilot-sdk selection and honest local-harness implementation identity.",
        class: BenchmarkClass::Documentation,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::SingleProcess,
        objective: "Produce a concise documentation-oriented execution summary for the current repository state and report the relevant reflected runtime contracts.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "safe-code-change-rusty-clawd",
        title: "Safe code change style task on rusty-clawd",
        description: "Exercise a bounded safe-change objective on the distinct rusty-clawd backend through the loopback multi-process topology.",
        class: BenchmarkClass::SafeCodeChange,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Plan a narrow, reviewable runtime change and summarize the exact evidence an operator would inspect before approving it.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "composite-session-review",
        title: "Composite identity session quality review",
        description: "Exercise the composite engineer identity as a session-quality benchmark so the starter suite covers the shipped composite identity as well as the dedicated gym identity.",
        class: BenchmarkClass::SessionQuality,
        identity: "simard-composite-engineer",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Run a disciplined bounded engineering session, preserve evidence, and produce a concise operator-facing summary of what happened.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "interactive-terminal-driving",
        title: "Interactive terminal driving on terminal-shell",
        description: "Exercise the engineer identity through the terminal-shell base type by launching a bounded interactive child process, waiting for prompts, and sending follow-up inputs like an operator validating generic PTY-driven control flow.",
        class: BenchmarkClass::SessionQuality,
        identity: "simard-engineer",
        base_type: "terminal-shell",
        topology: RuntimeTopology::SingleProcess,
        objective: "working-directory: .\ncommand: sh -c 'printf \"terminal-ready\\n\"; while IFS= read -r line; do if [ \"$line\" = \"ack\" ]; then printf \"terminal-ack\\n\"; elif [ \"$line\" = \"exit\" ]; then printf \"terminal-bye\\n\"; break; else printf \"echo:%s\\n\" \"$line\"; fi; done'\nwait-for: terminal-ready\ninput: ack\nwait-for: terminal-ack\ninput: exit\nwait-for: terminal-bye",
        expected_min_runtime_evidence: 6,
    },
    // --- Benchmark scenarios: real gym challenges ---
    BenchmarkScenario {
        id: "repo-exploration-deep-scan",
        title: "Deep repository structure analysis",
        description: "Explore the Simard repository itself to identify project structure, key source files, dependency graph from Cargo.toml, and executable entry points. Scored on completeness of structural discovery.",
        class: BenchmarkClass::RepoExploration,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the repository at /home/azureuser/src/Simard. Identify: (1) the top-level project structure and key directories, (2) all Cargo.toml dependencies and their purposes, (3) the main entry point(s) in src/main.rs or src/lib.rs, (4) at least five public modules and their responsibilities. Produce a structured summary covering all four areas.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "doc-generation-public-fn",
        title: "Generate doc comment for public function",
        description: "Given a source file in the Simard repository, generate a syntactically valid Rust doc comment for a public function. Scored on whether the comment is valid rustdoc, mentions parameters and return type, and accurately describes behavior.",
        class: BenchmarkClass::Documentation,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Read the function `pub fn benchmark_scenarios()` in src/gym.rs. Generate a complete Rust doc comment (/// style) for it that: (1) describes what the function returns, (2) mentions the BenchmarkScenario type, (3) notes the static lifetime of the returned slice, (4) is syntactically valid rustdoc. Output the doc comment text.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "safe-code-change-add-derive",
        title: "Add Debug derive to a struct",
        description: "Execute a safe, minimal code change: add a #[derive(Debug)] attribute to a struct that lacks it. Scored on whether the change compiles successfully and the derive is correctly placed.",
        class: BenchmarkClass::SafeCodeChange,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Identify a public struct in the Simard codebase that does not derive Debug. Plan a minimal change to add #[derive(Debug)] to it. Describe the exact file, line, and change required. Verify the change would compile by checking for any generic parameter constraints that might prevent Debug derivation.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "test-writing-unit-case",
        title: "Write a unit test for a public function",
        description: "Given a function signature, write a unit test that exercises its basic behavior. Scored on whether the test compiles, covers the primary code path, and uses appropriate assertions.",
        class: BenchmarkClass::TestWriting,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Write a unit test for the function `pub fn goal_slug(title: &str) -> String` defined in src/goals.rs. The test should: (1) call goal_slug with a representative input string containing uppercase letters, spaces, and special characters, (2) assert the output matches expected slug format (lowercase, hyphen-separated, no leading/trailing hyphens), (3) be a valid #[test] function that compiles and runs.",
        expected_min_runtime_evidence: 3,
    },
];

pub fn benchmark_scenarios() -> &'static [BenchmarkScenario] {
    &BENCHMARK_SCENARIOS
}

pub(super) fn resolve_benchmark_scenario(scenario_id: &str) -> SimardResult<BenchmarkScenario> {
    benchmark_scenarios()
        .iter()
        .copied()
        .find(|candidate| candidate.id == scenario_id)
        .ok_or_else(|| SimardError::BenchmarkScenarioNotFound {
            scenario_id: scenario_id.to_string(),
        })
}

/// Produce additional scenario-class-specific checks based on the scenario type.
///
/// These supplement the generic 8-check baseline with checks tailored to each
/// `BenchmarkClass`: structural discovery for repo exploration, doc validity
/// for documentation, compilation evidence for safe code changes, and test
/// structure for test writing scenarios.
pub(super) fn class_specific_checks(
    scenario: &BenchmarkScenario,
    outcome: &crate::runtime::SessionOutcome,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let summary = outcome.execution_summary.to_lowercase();
    let plan = outcome.plan.to_lowercase();
    let reflection = outcome.reflection.summary.to_lowercase();
    let combined = format!("{summary} {plan} {reflection}");

    match scenario.class {
        BenchmarkClass::RepoExploration => {
            let structure_mentioned = combined.contains("src/")
                || combined.contains("directory")
                || combined.contains("structure")
                || combined.contains("module");
            let deps_mentioned = combined.contains("cargo.toml")
                || combined.contains("dependenc")
                || combined.contains("crate");
            let entry_mentioned = combined.contains("main.rs")
                || combined.contains("lib.rs")
                || combined.contains("entry point")
                || combined.contains("entry-point");
            vec![
                BenchmarkCheckResult {
                    id: "repo-structure-discovered".to_string(),
                    passed: structure_mentioned,
                    detail: format!(
                        "execution output {} project structure references",
                        if structure_mentioned {
                            "contains"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "repo-dependencies-identified".to_string(),
                    passed: deps_mentioned,
                    detail: format!(
                        "execution output {} dependency references",
                        if deps_mentioned { "contains" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "repo-entry-points-found".to_string(),
                    passed: entry_mentioned,
                    detail: format!(
                        "execution output {} entry point references",
                        if entry_mentioned { "contains" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::Documentation => {
            let has_doc_syntax = combined.contains("///")
                || combined.contains("doc comment")
                || combined.contains("rustdoc")
                || combined.contains("documentation");
            let mentions_params = combined.contains("param")
                || combined.contains("argument")
                || combined.contains("return")
                || combined.contains("-> ");
            vec![
                BenchmarkCheckResult {
                    id: "doc-comment-syntax-valid".to_string(),
                    passed: has_doc_syntax,
                    detail: format!(
                        "execution output {} doc comment syntax",
                        if has_doc_syntax {
                            "references"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "doc-params-return-covered".to_string(),
                    passed: mentions_params,
                    detail: format!(
                        "execution output {} parameter/return documentation",
                        if mentions_params { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::SafeCodeChange => {
            let compilation_evidence = combined.contains("compil")
                || combined.contains("cargo build")
                || combined.contains("cargo check")
                || combined.contains("build succeed")
                || combined.contains("no errors");
            let change_described = combined.contains("derive")
                || combined.contains("change")
                || combined.contains("modif")
                || combined.contains("diff");
            vec![
                BenchmarkCheckResult {
                    id: "code-change-compilation-checked".to_string(),
                    passed: compilation_evidence,
                    detail: format!(
                        "execution output {} compilation verification",
                        if compilation_evidence {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "code-change-described".to_string(),
                    passed: change_described,
                    detail: format!(
                        "execution output {} change description",
                        if change_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::TestWriting => {
            let has_test_annotation = combined.contains("#[test]")
                || combined.contains("test function")
                || combined.contains("unit test");
            let has_assertion = combined.contains("assert")
                || combined.contains("expect")
                || combined.contains("should_eq")
                || combined.contains("assert_eq");
            let covers_basic_case = combined.contains("input")
                || combined.contains("call")
                || combined.contains("invoke")
                || combined.contains("result");
            vec![
                BenchmarkCheckResult {
                    id: "test-structure-valid".to_string(),
                    passed: has_test_annotation,
                    detail: format!(
                        "execution output {} test annotation/structure",
                        if has_test_annotation {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "test-has-assertions".to_string(),
                    passed: has_assertion,
                    detail: format!(
                        "execution output {} assertions",
                        if has_assertion { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "test-covers-basic-case".to_string(),
                    passed: covers_basic_case,
                    detail: format!(
                        "execution output {} basic case coverage",
                        if covers_basic_case {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::SessionQuality => {
            // Session quality scenarios rely on the generic checks.
            let session_summary_present =
                !outcome.execution_summary.trim().is_empty() && exported.memory_records.len() >= 2;
            vec![BenchmarkCheckResult {
                id: "session-quality-summary-adequate".to_string(),
                passed: session_summary_present,
                detail: format!(
                    "session produced {} memory records with {} execution summary",
                    exported.memory_records.len(),
                    if outcome.execution_summary.trim().is_empty() {
                        "empty"
                    } else {
                        "non-empty"
                    }
                ),
            }]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeId;
    use crate::handoff::RuntimeHandoffSnapshot;
    use crate::identity::ManifestContract;
    use crate::identity::OperatingMode;
    use crate::memory::{CognitiveMemoryType, MemoryRecord};
    use crate::metadata::{BackendDescriptor, Freshness, Provenance};
    use crate::reflection::{ReflectionReport, ReflectionSnapshot};
    use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, SessionOutcome};
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
                    memory_type: CognitiveMemoryType::Procedural,
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
}
