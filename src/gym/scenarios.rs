use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::runtime::RuntimeTopology;

use super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};

const BENCHMARK_SCENARIOS: [BenchmarkScenario; 21] = [
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
    // --- Additional scenarios for broader coverage ---
    BenchmarkScenario {
        id: "test-writing-edge-cases",
        title: "Write edge-case tests for boundary conditions",
        description: "Exercise writing tests that cover boundary conditions and edge cases for a function with numeric or string inputs. Scored on whether tests cover empty input, maximum values, and off-by-one conditions.",
        class: BenchmarkClass::TestWriting,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Write edge-case unit tests for the function `pub fn render_benchmark_count(count: usize, label: &str) -> String` in src/gym/reporting.rs. Tests should cover: (1) count of zero, (2) count of 1 (singular vs plural handling), (3) a large count value, (4) an empty label string. Each test should be a valid #[test] function with clear assertions.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "bug-fix-error-propagation",
        title: "Identify and describe an error propagation fix",
        description: "Exercise identifying code that uses .expect() or .unwrap() where Result propagation with ? would be safer. Scored on whether the defect is correctly identified, the fix is described precisely, and safety implications are noted.",
        class: BenchmarkClass::BugFix,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard codebase for a production (non-test) function that uses .expect() or .unwrap() on a Result or Option where the calling function already returns Result. Identify: (1) the exact file and line, (2) why the panic is unsafe in that context, (3) the precise replacement using ? or .ok_or_else(), (4) any signature changes needed. Produce a structured fix description.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "bug-fix-off-by-one",
        title: "Identify potential off-by-one or boundary bug",
        description: "Exercise identifying code with potential off-by-one errors, incorrect boundary checks, or fence-post problems. Scored on whether the analysis is specific, the risk is correctly assessed, and a concrete fix is proposed.",
        class: BenchmarkClass::BugFix,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the Simard codebase for a function that performs index arithmetic, slice operations, or loop bounds checking. Identify: (1) a specific location where boundary handling could be incorrect, (2) the exact inputs that would trigger the issue, (3) a concrete fix with before/after code. If no real bug exists, describe the defensive check that would guard against one.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "refactor-extract-function",
        title: "Identify and plan a function extraction refactor",
        description: "Exercise identifying a code block that should be extracted into a named function for clarity and reuse. Scored on whether the extracted function has a clear responsibility, appropriate parameters, and preserves existing behavior.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Find a function in the Simard codebase longer than 30 lines that contains a logically distinct block of code suitable for extraction. Describe: (1) the source file and function name, (2) the lines to extract, (3) the new function signature (name, parameters, return type), (4) how the original function would call the extracted function. Verify the refactor preserves behavior.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "refactor-simplify-match",
        title: "Simplify a complex match expression",
        description: "Exercise simplifying a match expression by combining arms, using wildcard patterns, or converting to if-let chains. Scored on whether the simplification is correct, reduces line count, and preserves all behavior.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-engineer",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Find a match expression in the Simard codebase with 4 or more arms where some arms share identical or nearly identical bodies. Describe: (1) the exact location, (2) which arms can be combined, (3) the simplified match expression, (4) any edge cases that must be preserved. Show both the original and simplified code.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "repo-exploration-multi-process",
        title: "Repository exploration under multi-process topology",
        description: "Exercise the same repo-exploration task as the local variant but under the multi-process topology, validating that the loopback mesh transport and coordinated supervisor correctly propagate exploration results.",
        class: BenchmarkClass::RepoExploration,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Analyze the repository at /home/azureuser/src/Simard under multi-process topology. Identify: (1) the top-level project structure and key directories, (2) all Cargo.toml dependencies and their purposes, (3) the main entry point(s), (4) at least five public modules and their responsibilities. Produce a structured summary covering all four areas.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "doc-generation-multi-process",
        title: "Documentation generation on multi-process topology",
        description: "Exercise documentation generation through the loopback multi-process topology to validate that doc tasks work correctly across process boundaries.",
        class: BenchmarkClass::Documentation,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Read the function `pub fn run_benchmark_suite()` in src/gym/mod.rs. Generate a comprehensive Rust doc comment (/// style) that: (1) describes the function's purpose and behavior, (2) documents the `suite_id` and `output_root` parameters, (3) explains the return type `SimardResult<BenchmarkSuiteReport>`, (4) notes error conditions. Output the doc comment text.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "bug-fix-distributed",
        title: "Bug fix scenario on distributed topology",
        description: "Exercise a bug-fix objective on the distributed topology to validate that diagnostic reasoning works across distributed process boundaries.",
        class: BenchmarkClass::BugFix,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::Distributed,
        objective: "Identify a potential panic site in the Simard codebase where an `.unwrap()` is called on a `Result` or `Option` that could reasonably fail at runtime. Describe: (1) the file and line, (2) the conditions under which it would panic, (3) the appropriate error handling replacement (e.g., `?` operator, `unwrap_or_default`, `match`). Do not modify code, only analyze and recommend.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "dep-analysis-cargo-audit",
        title: "Dependency audit of Cargo.toml",
        description: "Analyze the project's Cargo.toml to identify dependency health: version currency, potential conflicts, unused dependencies, and feature flag usage.",
        class: BenchmarkClass::DependencyAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the Cargo.toml at the repository root. Report: (1) total number of direct dependencies, (2) any dependencies using path-based or git-based sources instead of crates.io, (3) dependencies with wildcard (*) version specs, (4) optional dependencies and their associated feature flags, (5) any dev-dependencies that could be moved to regular dependencies or vice versa. Produce a structured audit report.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "dep-analysis-module-coupling",
        title: "Module coupling analysis",
        description: "Analyze inter-module dependencies within the Simard codebase to identify tightly coupled modules and suggest decoupling opportunities.",
        class: BenchmarkClass::DependencyAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the `use crate::` import statements across all modules in src/. Report: (1) which module is imported by the most other modules, (2) any circular dependency chains, (3) modules that import more than 5 sibling modules, (4) suggested decoupling strategies for the most coupled module. Produce a dependency matrix summary.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "error-handling-unwrap-audit",
        title: "Unwrap usage audit",
        description: "Systematically identify and classify .unwrap() calls across the codebase, distinguishing safe usage (e.g., after .is_some() check) from risky runtime panics.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard src/ directory for all `.unwrap()` calls. For each occurrence, classify as: (A) safe — preceded by a guard or in test code, (B) risky — could panic in production. Report: (1) total unwrap count, (2) count of safe vs risky, (3) the top 3 riskiest unwrap sites with file, line, and justification. Produce a prioritized remediation list.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "error-handling-propagation-chain",
        title: "Error propagation chain analysis",
        description: "Trace error propagation paths through the SimardError type to verify that errors surface meaningful context to the operator.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the SimardError enum in src/error.rs. Trace how errors propagate from: (1) runtime failures through to CLI output, (2) benchmark execution through to gym reports. For each path, assess whether the error message preserved enough context for an operator to diagnose the issue without reading source code. Report any error variants that lose context during propagation.",
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
        BenchmarkClass::BugFix => {
            let defect_identified = combined.contains("bug")
                || combined.contains("defect")
                || combined.contains("issue")
                || combined.contains("unwrap")
                || combined.contains("expect")
                || combined.contains("panic");
            let fix_described = combined.contains("fix")
                || combined.contains("replac")
                || combined.contains("propagat")
                || combined.contains("convert")
                || combined.contains("refactor");
            let safety_analysis = combined.contains("safe")
                || combined.contains("error handling")
                || combined.contains("result")
                || combined.contains("graceful")
                || combined.contains("recover");
            vec![
                BenchmarkCheckResult {
                    id: "bug-defect-identified".to_string(),
                    passed: defect_identified,
                    detail: format!(
                        "execution output {} defect identification",
                        if defect_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "bug-fix-described".to_string(),
                    passed: fix_described,
                    detail: format!(
                        "execution output {} fix description",
                        if fix_described { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "bug-safety-analyzed".to_string(),
                    passed: safety_analysis,
                    detail: format!(
                        "execution output {} safety analysis",
                        if safety_analysis { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::Refactoring => {
            let change_identified = combined.contains("extract")
                || combined.contains("simplif")
                || combined.contains("refactor")
                || combined.contains("renam")
                || combined.contains("restructur");
            let behavior_preserved = combined.contains("preserv")
                || combined.contains("behavior")
                || combined.contains("equivalent")
                || combined.contains("same result")
                || combined.contains("no change in");
            let code_shown = combined.contains("fn ")
                || combined.contains("before")
                || combined.contains("after")
                || combined.contains("original")
                || combined.contains("simplified");
            vec![
                BenchmarkCheckResult {
                    id: "refactor-change-identified".to_string(),
                    passed: change_identified,
                    detail: format!(
                        "execution output {} refactoring identification",
                        if change_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "refactor-behavior-preserved".to_string(),
                    passed: behavior_preserved,
                    detail: format!(
                        "execution output {} behavior preservation evidence",
                        if behavior_preserved {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "refactor-code-shown".to_string(),
                    passed: code_shown,
                    detail: format!(
                        "execution output {} code examples",
                        if code_shown { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::DependencyAnalysis => {
            let deps_analyzed = combined.contains("cargo.toml")
                || combined.contains("dependenc")
                || combined.contains("crate")
                || combined.contains("version");
            let coupling_assessed = combined.contains("import")
                || combined.contains("coupling")
                || combined.contains("module")
                || combined.contains("use crate");
            let recommendations_present = combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("should")
                || combined.contains("consider")
                || combined.contains("audit");
            vec![
                BenchmarkCheckResult {
                    id: "dep-analysis-performed".to_string(),
                    passed: deps_analyzed,
                    detail: format!(
                        "execution output {} dependency analysis",
                        if deps_analyzed { "contains" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-coupling-assessed".to_string(),
                    passed: coupling_assessed,
                    detail: format!(
                        "execution output {} coupling assessment",
                        if coupling_assessed {
                            "contains"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-recommendations-present".to_string(),
                    passed: recommendations_present,
                    detail: format!(
                        "execution output {} actionable recommendations",
                        if recommendations_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ErrorHandling => {
            let error_analysis = combined.contains("unwrap")
                || combined.contains("error")
                || combined.contains("panic")
                || combined.contains("result");
            let classification_present = combined.contains("safe")
                || combined.contains("risky")
                || combined.contains("classif")
                || combined.contains("categor");
            let propagation_traced = combined.contains("propagat")
                || combined.contains("chain")
                || combined.contains("context")
                || combined.contains("diagnostic");
            vec![
                BenchmarkCheckResult {
                    id: "error-analysis-performed".to_string(),
                    passed: error_analysis,
                    detail: format!(
                        "execution output {} error analysis",
                        if error_analysis { "contains" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "error-classification-present".to_string(),
                    passed: classification_present,
                    detail: format!(
                        "execution output {} error classification",
                        if classification_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "error-propagation-traced".to_string(),
                    passed: propagation_traced,
                    detail: format!(
                        "execution output {} propagation tracing",
                        if propagation_traced {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
    }
}
