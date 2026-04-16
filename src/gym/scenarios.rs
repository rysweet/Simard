use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::runtime::RuntimeTopology;

use super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};

const BENCHMARK_SCENARIOS: [BenchmarkScenario; 35] = [
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
    // --- Wave 3: PerformanceAnalysis scenarios ---
    BenchmarkScenario {
        id: "perf-quadratic-detection",
        title: "Identify quadratic or worse algorithms",
        description: "Identify O(n²) or worse algorithms in string/collection processing code. Scored on whether the analysis correctly identifies the complexity class and suggests an efficient alternative.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard codebase for functions that process collections or strings using nested iteration (e.g., nested loops over vectors, repeated string concatenation in a loop, or repeated contains/find calls inside a loop). For each finding: (1) identify the file and function, (2) state the current complexity class (e.g., O(n²), O(n·m)), (3) explain why it is suboptimal, (4) propose an efficient alternative with its complexity class. Produce a prioritized list of at least two findings.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "perf-caching-opportunity",
        title: "Identify caching opportunities for repeated computations",
        description: "Analyze code with repeated expensive computations and suggest appropriate caching strategies. Scored on identifying cache-worthy call sites and proposing a reasonable cache invalidation approach.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the Simard codebase for functions that perform repeated expensive computations (e.g., repeated file I/O, repeated serialization, repeated regex compilation, or functions called in a loop with identical arguments). For each opportunity: (1) identify the call site, (2) estimate the cost of repeated execution, (3) propose a caching strategy (e.g., lazy_static, HashMap memoization, once_cell), (4) describe the cache invalidation approach. Produce a structured report.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "perf-allocation-audit",
        title: "Audit unnecessary heap allocations",
        description: "Review code for unnecessary heap allocations (e.g., String where &str suffices, Vec clones). Scored on identifying allocation hot spots and suggesting zero-copy alternatives.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Review the Simard codebase for functions that perform unnecessary heap allocations. Look for: (1) String parameters where &str would suffice, (2) Vec::clone() where a slice reference would work, (3) .to_string() or .to_owned() in hot paths where borrowing is possible, (4) format!() for simple concatenation that could use push_str. For each finding, describe the allocation, explain why it is unnecessary, and propose a zero-copy or reduced-allocation alternative. Report at least three findings.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 3: SecurityAudit scenarios ---
    BenchmarkScenario {
        id: "security-unsafe-block-audit",
        title: "Audit unsafe blocks for justification and alternatives",
        description: "Systematically identify and classify `unsafe` blocks, assessing whether each is justified and whether safe alternatives exist. Scored on classification accuracy and risk assessment quality.",
        class: BenchmarkClass::SecurityAudit,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard src/ directory for all `unsafe` blocks and `unsafe fn` declarations. For each occurrence: (1) identify the file and line, (2) classify the reason for unsafety (FFI, raw pointer dereference, union access, mutable static, unchecked invariant), (3) assess whether a safe alternative exists (e.g., safe wrapper, standard library API), (4) rate the risk as low/medium/high based on potential for undefined behavior. If no unsafe blocks exist, report that finding and suggest where unsafe might be needed (e.g., FFI boundaries) if the project ever grows to require it.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "security-credential-patterns",
        title: "Review code for credential leakage patterns",
        description: "Review code for hardcoded secrets, credential leakage in logs, or insecure storage patterns. Scored on detection completeness and remediation suggestions.",
        class: BenchmarkClass::SecurityAudit,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard codebase for credential-related security patterns. Check for: (1) hardcoded strings resembling API keys, tokens, or passwords, (2) log statements that might print sensitive data (look for variables named key, token, secret, password near println!, log::, tracing::), (3) environment variable reads for secrets without validation, (4) file paths that suggest credential storage. For each finding, describe the risk and propose a remediation (e.g., use environment variables, redact in logs, use a secrets manager). Produce a structured security audit report.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "security-input-validation",
        title: "Analyze public API entry points for input validation",
        description: "Analyze public API entry points for missing input validation, buffer overflows, or injection vulnerabilities. Scored on coverage of attack surfaces and quality of suggested mitigations.",
        class: BenchmarkClass::SecurityAudit,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Identify public functions in the Simard codebase that accept string or numeric inputs from external sources (CLI arguments, file paths, configuration values). For each entry point: (1) describe what inputs it accepts, (2) assess whether inputs are validated before use (length checks, character validation, range checks), (3) identify potential injection risks (path traversal, command injection if inputs reach shell commands), (4) propose specific validation or sanitization to add. Produce a prioritized list of at least three entry points.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 3: ApiDesign scenarios ---
    BenchmarkScenario {
        id: "api-builder-pattern-review",
        title: "Evaluate builder pattern implementations",
        description: "Evaluate builder pattern implementations for ergonomics, type safety, and completeness. Scored on identifying missing validations, suggesting improvements, and assessing the builder's discoverability.",
        class: BenchmarkClass::ApiDesign,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Search the Simard codebase for builder pattern implementations (structs with methods returning &mut Self or Self, or structs named *Builder). For each builder found: (1) assess whether required fields are enforced at compile time vs runtime, (2) check if the build() method validates all invariants, (3) evaluate method naming consistency and discoverability, (4) suggest improvements such as typestate pattern for compile-time safety. If no builders exist, identify a struct that would benefit from a builder pattern and sketch the design.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "api-breaking-change-detection",
        title: "Identify breaking changes between API versions",
        description: "Given two versions of a public API, identify breaking changes in function signatures, trait bounds, or type definitions. Scored on completeness and accuracy of identified breaking changes.",
        class: BenchmarkClass::ApiDesign,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the public API surface of the Simard crate (all `pub fn`, `pub struct`, `pub enum`, `pub trait` items). Identify: (1) functions whose signatures would break callers if modified (parameter types, return types, generic bounds), (2) enums that are non-exhaustive vs exhaustive and the implications of adding variants, (3) trait definitions where adding a method would break existing implementors, (4) type aliases or re-exports that downstream code might depend on. Produce a breaking-change risk assessment for the top 5 most sensitive API items.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "api-trait-vs-generics",
        title: "Analyze trait object vs generic parameter usage",
        description: "Analyze trait object usage (dyn Trait) vs generic parameters to recommend the appropriate approach based on use case. Scored on correctness of trade-off analysis.",
        class: BenchmarkClass::ApiDesign,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Search the Simard codebase for uses of `dyn` trait objects and generic type parameters (`<T: Trait>`). For each usage: (1) identify whether dynamic dispatch (dyn) or static dispatch (generics) is used, (2) assess whether the choice is appropriate for the use case (consider: number of implementors, performance sensitivity, binary size impact, need for heterogeneous collections), (3) recommend switching if the alternative would be more appropriate, with justification. Produce a summary of dispatch strategy patterns across the codebase.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 3: TestWriting additional scenarios ---
    BenchmarkScenario {
        id: "test-integration-scaffold",
        title: "Design integration test for multi-module workflow",
        description: "Design an integration test that exercises a multi-module workflow end-to-end. Scored on whether the test covers the critical path, uses appropriate fixtures, and validates observable side effects.",
        class: BenchmarkClass::TestWriting,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Design an integration test for the Simard gym benchmark execution pipeline (scenario resolution → executor → reporting). The test should: (1) select a specific scenario by ID, (2) mock or stub the runtime execution layer, (3) verify that the executor produces a BenchmarkRunReport with correct fields, (4) verify that the reporting module can serialize the report to JSON. Write the complete test function with appropriate setup, execution, and assertions.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "test-property-based-design",
        title: "Design property-based test specifications",
        description: "Given a function with numeric or string inputs, design property-based test specifications. Scored on whether properties are meaningful invariants (not just restating the implementation).",
        class: BenchmarkClass::TestWriting,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Choose a pure function in the Simard codebase that transforms strings or numbers (e.g., slug generation, ID formatting, score calculation). Design at least three property-based test specifications: (1) an idempotency or roundtrip property, (2) a structural invariant (e.g., output length bounds, character set constraints), (3) a relationship property between related inputs (e.g., ordering preservation). For each property, describe it precisely enough that a proptest or quickcheck harness could implement it.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 3: Refactoring additional scenarios ---
    BenchmarkScenario {
        id: "refactor-inline-abstraction",
        title: "Identify and inline unnecessary abstraction",
        description: "Identify an unnecessary abstraction layer (e.g., a trait with a single implementation, a wrapper adding no value) and propose inlining. Scored on whether the simplification reduces indirection without losing extensibility.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Search the Simard codebase for abstraction layers that may be unnecessary: (1) traits with exactly one implementation, (2) wrapper structs that simply delegate all methods to an inner type, (3) intermediate modules that re-export without adding logic. For each candidate: describe the abstraction, assess whether the indirection provides current or future value, and if not, propose a concrete inlining plan with before/after code sketches. Justify whether extensibility would be lost.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "refactor-consolidate-duplicates",
        title: "Consolidate duplicated logic across modules",
        description: "Identify duplicated logic across modules and propose a shared utility. Scored on whether the identified duplication is genuine, the proposed consolidation is correct, and the shared code is appropriately placed.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard src/ directory for duplicated code patterns across different modules. Look for: (1) identical or near-identical function bodies in different files, (2) repeated error-handling boilerplate, (3) duplicated string formatting or serialization logic. For each finding: identify the duplicated locations, describe the shared logic, propose a shared utility function or module, and specify where the shared code should live (e.g., a utils module or existing common module). Show the consolidated code and how call sites would change.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 3: SafeCodeChange additional scenario ---
    BenchmarkScenario {
        id: "safe-change-add-enum-variant",
        title: "Safely add a new enum variant",
        description: "Safely add a new variant to an existing enum, updating all match arms across the codebase. Scored on whether all match expressions are found and updated, the change compiles, and no fallthrough behavior is introduced.",
        class: BenchmarkClass::SafeCodeChange,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Plan the addition of a new variant to an existing enum in the Simard codebase (e.g., a new BenchmarkClass or RuntimeTopology variant). Describe: (1) the enum to modify and the new variant name, (2) every match expression across the codebase that handles this enum (list file and line for each), (3) what the new arm should do in each match, (4) any Display, Serialize, or other trait implementations that need updating. Verify the plan would result in a compiling codebase with no unhandled match arms.",
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
        BenchmarkClass::PerformanceAnalysis => {
            let complexity_mentioned = combined.contains("o(n")
                || combined.contains("complexity")
                || combined.contains("quadratic")
                || combined.contains("linear")
                || combined.contains("big-o");
            let optimization_suggested = combined.contains("optimi")
                || combined.contains("cache")
                || combined.contains("memoiz")
                || combined.contains("allocat")
                || combined.contains("zero-copy");
            let bottleneck_identified = combined.contains("bottleneck")
                || combined.contains("hot path")
                || combined.contains("hot spot")
                || combined.contains("expensive")
                || combined.contains("repeated");
            vec![
                BenchmarkCheckResult {
                    id: "perf-complexity-analyzed".to_string(),
                    passed: complexity_mentioned,
                    detail: format!(
                        "execution output {} complexity analysis",
                        if complexity_mentioned {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "perf-optimization-suggested".to_string(),
                    passed: optimization_suggested,
                    detail: format!(
                        "execution output {} optimization suggestions",
                        if optimization_suggested {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "perf-bottleneck-identified".to_string(),
                    passed: bottleneck_identified,
                    detail: format!(
                        "execution output {} bottleneck identification",
                        if bottleneck_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::SecurityAudit => {
            let vulnerability_found = combined.contains("unsafe")
                || combined.contains("vulnerab")
                || combined.contains("cve")
                || combined.contains("credential")
                || combined.contains("secret")
                || combined.contains("injection");
            let risk_assessed = combined.contains("risk")
                || combined.contains("severity")
                || combined.contains("low")
                || combined.contains("medium")
                || combined.contains("high")
                || combined.contains("critical");
            let remediation_proposed = combined.contains("remediat")
                || combined.contains("mitigat")
                || combined.contains("fix")
                || combined.contains("sanitiz")
                || combined.contains("validat");
            vec![
                BenchmarkCheckResult {
                    id: "security-vulnerability-found".to_string(),
                    passed: vulnerability_found,
                    detail: format!(
                        "execution output {} vulnerability identification",
                        if vulnerability_found {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "security-risk-assessed".to_string(),
                    passed: risk_assessed,
                    detail: format!(
                        "execution output {} risk assessment",
                        if risk_assessed { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "security-remediation-proposed".to_string(),
                    passed: remediation_proposed,
                    detail: format!(
                        "execution output {} remediation proposal",
                        if remediation_proposed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ApiDesign => {
            let api_surface_analyzed = combined.contains("pub fn")
                || combined.contains("pub struct")
                || combined.contains("pub trait")
                || combined.contains("public api")
                || combined.contains("api surface");
            let design_quality_assessed = combined.contains("ergonomic")
                || combined.contains("discoverab")
                || combined.contains("builder")
                || combined.contains("breaking change")
                || combined.contains("type safe");
            let recommendation_present = combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("improv")
                || combined.contains("should")
                || combined.contains("consider");
            vec![
                BenchmarkCheckResult {
                    id: "api-surface-analyzed".to_string(),
                    passed: api_surface_analyzed,
                    detail: format!(
                        "execution output {} API surface analysis",
                        if api_surface_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "api-design-quality-assessed".to_string(),
                    passed: design_quality_assessed,
                    detail: format!(
                        "execution output {} design quality assessment",
                        if design_quality_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "api-recommendation-present".to_string(),
                    passed: recommendation_present,
                    detail: format!(
                        "execution output {} design recommendations",
                        if recommendation_present {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_scenarios_not_empty() {
        let scenarios = benchmark_scenarios();
        assert!(!scenarios.is_empty());
    }

    #[test]
    fn benchmark_scenarios_ids_are_unique() {
        let scenarios = benchmark_scenarios();
        let ids: Vec<_> = scenarios.iter().map(|s| s.id).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "scenario IDs must be unique");
    }

    #[test]
    fn resolve_known_scenario() {
        let scenario = resolve_benchmark_scenario("repo-exploration-local").unwrap();
        assert_eq!(scenario.id, "repo-exploration-local");
        assert_eq!(scenario.class, BenchmarkClass::RepoExploration);
    }

    #[test]
    fn resolve_unknown_scenario_errors() {
        let result = resolve_benchmark_scenario("nonexistent-scenario");
        assert!(result.is_err());
    }

    #[test]
    fn all_scenarios_have_nonempty_fields() {
        for scenario in benchmark_scenarios() {
            assert!(!scenario.id.is_empty(), "id must be non-empty");
            assert!(
                !scenario.title.is_empty(),
                "title must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.description.is_empty(),
                "description must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.identity.is_empty(),
                "identity must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.base_type.is_empty(),
                "base_type must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.objective.is_empty(),
                "objective must be non-empty for {}",
                scenario.id
            );
        }
    }

    #[test]
    fn benchmark_class_display_roundtrip() {
        let classes = [
            (BenchmarkClass::RepoExploration, "repo-exploration"),
            (BenchmarkClass::Documentation, "documentation"),
            (BenchmarkClass::SafeCodeChange, "safe-code-change"),
            (BenchmarkClass::SessionQuality, "session-quality"),
            (BenchmarkClass::TestWriting, "test-writing"),
            (BenchmarkClass::BugFix, "bug-fix"),
            (BenchmarkClass::Refactoring, "refactoring"),
            (BenchmarkClass::DependencyAnalysis, "dependency-analysis"),
            (BenchmarkClass::ErrorHandling, "error-handling"),
            (BenchmarkClass::PerformanceAnalysis, "performance-analysis"),
            (BenchmarkClass::SecurityAudit, "security-audit"),
            (BenchmarkClass::ApiDesign, "api-design"),
        ];
        for (class, label) in classes {
            assert_eq!(class.to_string(), label);
        }
    }

    // --- resolve_benchmark_scenario: all scenarios resolve ---

    #[test]
    fn all_scenarios_resolve_by_id() {
        for scenario in benchmark_scenarios() {
            let resolved = resolve_benchmark_scenario(scenario.id).unwrap();
            assert_eq!(resolved.id, scenario.id);
            assert_eq!(resolved.class, scenario.class);
        }
    }

    // --- scenario consistency: identities match expected patterns ---

    #[test]
    fn all_scenarios_use_valid_identity() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario.identity == "simard-gym" || scenario.identity == "simard-engineer",
                "unexpected identity '{}' in scenario '{}'",
                scenario.identity,
                scenario.id
            );
        }
    }

    #[test]
    fn all_scenarios_use_valid_base_type() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario.base_type == "local-harness" || scenario.base_type == "terminal-shell",
                "unexpected base_type '{}' in scenario '{}'",
                scenario.base_type,
                scenario.id
            );
        }
    }

    // --- scenario ID format conventions ---

    #[test]
    fn all_scenario_ids_are_lowercase_kebab_case() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "scenario id '{}' must be lowercase kebab-case",
                scenario.id
            );
        }
    }

    #[test]
    fn all_scenarios_have_reasonable_evidence_requirements() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario.expected_min_runtime_evidence <= 20,
                "scenario '{}' requires too many evidence records: {}",
                scenario.id,
                scenario.expected_min_runtime_evidence
            );
        }
    }

    // --- BenchmarkClass: all 12 classes covered by at least one scenario ---

    #[test]
    fn every_benchmark_class_has_at_least_one_scenario() {
        let all_classes = [
            BenchmarkClass::RepoExploration,
            BenchmarkClass::Documentation,
            BenchmarkClass::SafeCodeChange,
            BenchmarkClass::SessionQuality,
            BenchmarkClass::TestWriting,
            BenchmarkClass::BugFix,
            BenchmarkClass::Refactoring,
            BenchmarkClass::DependencyAnalysis,
            BenchmarkClass::ErrorHandling,
            BenchmarkClass::PerformanceAnalysis,
            BenchmarkClass::SecurityAudit,
            BenchmarkClass::ApiDesign,
        ];
        let scenarios = benchmark_scenarios();
        for class in all_classes {
            assert!(
                scenarios.iter().any(|s| s.class == class),
                "no scenario covers class '{}'",
                class
            );
        }
    }
}
