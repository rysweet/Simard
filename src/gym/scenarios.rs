use crate::error::{SimardError, SimardResult};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::runtime::RuntimeTopology;

use super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};

const BENCHMARK_SCENARIOS: [BenchmarkScenario; 100] = [
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
    // --- Wave 4: CodeReview scenarios ---
    BenchmarkScenario {
        id: "code-review-public-api-surface",
        title: "Review public API surface for consistency",
        description: "Perform a code review of the public API surface in a Rust module, checking for naming consistency, documentation coverage, and type safety patterns.",
        class: BenchmarkClass::CodeReview,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Review the public API surface of src/gym/types.rs. Evaluate: (1) naming consistency across public structs and enums (snake_case fields, PascalCase types), (2) documentation coverage — which public items lack doc comments, (3) derive macro consistency — whether all serializable types derive the same set of traits, (4) type safety — whether any fields use String where an enum or newtype would be more appropriate. Produce a structured review with findings and severity ratings.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "code-review-error-propagation",
        title: "Review error propagation patterns in module",
        description: "Audit a module for correct error propagation using Result types, ensuring no silent error swallowing and consistent use of the ? operator.",
        class: BenchmarkClass::CodeReview,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Review error propagation in src/gym/executor.rs. Check: (1) all fallible operations return Result, (2) unwrap/expect calls are justified with comments or used only in test code, (3) error context is preserved when converting between error types, (4) the ? operator is used consistently instead of manual match-on-Result patterns. List each finding with file location and recommended fix.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "code-review-test-quality",
        title: "Review test quality and coverage gaps",
        description: "Evaluate the quality of existing tests in a module, checking for assertion quality, edge case coverage, and test isolation.",
        class: BenchmarkClass::CodeReview,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Review the test quality in src/gym/tests_scenarios.rs. Evaluate: (1) whether tests use specific assertions (assert_eq!) rather than boolean checks (assert!), (2) whether edge cases are covered (empty input, boundary values), (3) whether tests are isolated and do not depend on ordering, (4) whether test names clearly describe what is being tested. Produce a quality report with improvement suggestions.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 4: Debugging scenarios ---
    BenchmarkScenario {
        id: "debugging-trace-error-origin",
        title: "Trace an error to its origin across modules",
        description: "Given an error type, trace its construction sites and propagation path through the codebase to identify where and why it originates.",
        class: BenchmarkClass::Debugging,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Trace the SimardError::BenchmarkScenarioNotFound variant through the Simard codebase. Identify: (1) where this error variant is defined, (2) every location where it is constructed (list file and line), (3) how it propagates through the call stack (which functions return it via ?), (4) where it is ultimately handled or displayed. Produce a propagation diagram showing the full error flow.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "debugging-type-mismatch",
        title: "Diagnose a hypothetical type mismatch scenario",
        description: "Analyze a function signature to identify potential type mismatch issues that could arise from callers passing incorrect argument types.",
        class: BenchmarkClass::Debugging,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the function `resolve_benchmark_scenario(scenario_id: &str)` in src/gym/scenarios.rs. Determine: (1) all call sites that invoke this function, (2) what types the callers pass (literal, String::as_str, format! result, etc.), (3) whether any caller could accidentally pass an owned String where &str is expected and what the compiler behavior would be, (4) whether the error message provides enough context to debug a wrong-id issue. Suggest diagnostic improvements.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "debugging-runtime-state-inspection",
        title: "Inspect runtime state transitions for correctness",
        description: "Examine the state machine transitions in the runtime lifecycle to verify all state transitions are valid and no illegal transitions are possible.",
        class: BenchmarkClass::Debugging,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Inspect the RuntimeState enum and its transitions in the Simard codebase. Determine: (1) all valid states and their meaning, (2) what triggers each state transition, (3) whether any code path could produce an invalid state transition (e.g., going from Stopped back to Running), (4) whether state transitions are logged or observable for debugging. Produce a state transition table with validity annotations.",
        expected_min_runtime_evidence: 4,
    },
    // --- Wave 4: ConfigManagement scenarios ---
    BenchmarkScenario {
        id: "config-management-cargo-feature-audit",
        title: "Audit Cargo feature flags for correctness",
        description: "Analyze the Cargo.toml feature flags to ensure they are correctly defined, documented, and do not create conflicting configurations.",
        class: BenchmarkClass::ConfigManagement,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Audit the Cargo.toml file(s) in the Simard repository. Determine: (1) all defined feature flags and their purpose, (2) which features enable optional dependencies, (3) whether any features conflict or create impossible configurations, (4) whether the default feature set is appropriate for common use cases. Produce a feature matrix showing which capabilities each feature enables.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "config-management-env-var-inventory",
        title: "Inventory environment variable usage",
        description: "Scan the codebase for environment variable reads and produce a complete inventory of expected configuration, defaults, and validation.",
        class: BenchmarkClass::ConfigManagement,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard codebase for all environment variable access (env!, option_env!, std::env::var, std::env::var_os). For each variable found: (1) name, (2) where it is read (file and line), (3) whether a default is provided if missing, (4) whether the value is validated after reading. Produce a configuration inventory table and flag any variables that lack defaults or validation.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "config-management-topology-matrix",
        title: "Validate topology configuration combinations",
        description: "Analyze the RuntimeTopology enum and its configuration paths to verify all topology variants produce valid, functional configurations.",
        class: BenchmarkClass::ConfigManagement,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the RuntimeTopology enum and how each variant configures the runtime in src/gym/mod.rs (runtime_ports_for_topology function). For each topology: (1) list which backends are selected (transport, mesh, supervisor), (2) verify the configuration is internally consistent (e.g., MultiProcess uses loopback transport), (3) identify any topology that lacks test coverage, (4) determine whether adding a new topology variant would require changes beyond this function. Produce a topology configuration matrix.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 4: expand thin classes ---
    BenchmarkScenario {
        id: "session-quality-memory-export",
        title: "Session quality: memory export completeness",
        description: "Verify that a session exports complete and well-structured memory records that preserve the session context for future reference.",
        class: BenchmarkClass::SessionQuality,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Run a bounded engineering session and evaluate the quality of exported memory records. Check: (1) memory records have meaningful keys (not auto-generated UUIDs), (2) memory scopes are correctly assigned (session vs global), (3) at least one memory record captures the session objective, (4) exported records are sufficient to reconstruct what happened in the session. Produce a memory quality assessment.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "error-handling-custom-error-design",
        title: "Evaluate custom error type design",
        description: "Assess the design quality of a custom error enum, checking for informative variants, Display implementations, and From conversions.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Evaluate the SimardError enum design in the Simard codebase. Assess: (1) whether each variant carries enough context for diagnostic messages, (2) whether the Display implementation produces actionable error messages, (3) whether From conversions exist for common upstream error types (io::Error, serde_json::Error, etc.), (4) whether error variants follow Rust naming conventions. Produce a design quality report with specific improvement suggestions.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "dependency-analysis-version-audit",
        title: "Audit dependency versions for staleness",
        description: "Analyze Cargo.toml dependencies to identify outdated, yanked, or unnecessarily pinned versions and recommend updates.",
        class: BenchmarkClass::DependencyAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Audit the dependencies in Cargo.toml for the Simard project. For each dependency: (1) note the current version constraint, (2) identify whether the constraint is too tight (exact pin) or too loose (wildcard), (3) check whether the dependency is used in the codebase (search for use/extern crate statements), (4) flag any dev-dependencies that appear in normal dependencies or vice versa. Produce a dependency health report with update recommendations.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 5: ConcurrencyAnalysis scenarios ---
    BenchmarkScenario {
        id: "concurrency-race-condition-audit",
        title: "Audit codebase for potential race conditions",
        description: "Analyze shared mutable state and concurrent access patterns to identify potential race conditions in the runtime.",
        class: BenchmarkClass::ConcurrencyAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard codebase for shared mutable state accessed from multiple threads or async tasks. Identify: (1) all uses of Arc<Mutex<_>>, Arc<RwLock<_>>, and atomics, (2) whether any shared state is accessed without proper synchronization, (3) potential TOCTOU (time-of-check-time-of-use) patterns where a value is read and then acted upon without holding a lock, (4) whether any channel-based patterns could deadlock if the receiver is dropped. Produce a race condition risk assessment.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "concurrency-deadlock-analysis",
        title: "Analyze lock ordering for deadlock potential",
        description: "Examine lock acquisition patterns across the codebase to identify potential deadlock scenarios from inconsistent lock ordering.",
        class: BenchmarkClass::ConcurrencyAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze all mutex and rwlock usage in the Simard codebase for deadlock potential. Determine: (1) all lock types and where they are defined, (2) whether any code path acquires multiple locks and in what order, (3) whether async code holds locks across .await points (which can cause deadlocks with non-async-aware mutexes), (4) whether any lock guard is held longer than necessary. Produce a lock ordering analysis with deadlock risk ratings.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "concurrency-async-safety-review",
        title: "Review async task safety and cancellation handling",
        description: "Assess async task spawning patterns for proper cancellation handling, resource cleanup, and structured concurrency.",
        class: BenchmarkClass::ConcurrencyAnalysis,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Review all async task spawning (tokio::spawn, tokio::task::spawn_blocking) in the Simard codebase. Assess: (1) whether spawned tasks have proper error handling for JoinError, (2) whether task cancellation is handled gracefully (e.g., select! branches with cleanup), (3) whether any spawned tasks hold resources that would leak on cancellation, (4) whether structured concurrency patterns (JoinSet, TaskTracker) are used where appropriate. Produce a task safety assessment.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 5: MigrationPlanning scenarios ---
    BenchmarkScenario {
        id: "migration-schema-evolution-plan",
        title: "Plan schema evolution for runtime state",
        description: "Design a migration strategy for evolving the runtime state serialization format while maintaining backward compatibility.",
        class: BenchmarkClass::MigrationPlanning,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the serialized state structures in the Simard codebase (RuntimeHandoffSnapshot, session state, memory records). Plan: (1) which fields are serialized and their current format, (2) what would break if a field were added, removed, or renamed, (3) whether serde attributes (default, skip_serializing_if, rename) are used to maintain compatibility, (4) a migration strategy for adding a new required field to RuntimeHandoffSnapshot without breaking existing serialized data. Produce a schema evolution plan.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "migration-api-versioning-strategy",
        title: "Design API versioning strategy for public interfaces",
        description: "Evaluate the public API surface and design a versioning strategy that supports backward-compatible evolution.",
        class: BenchmarkClass::MigrationPlanning,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the public API surface of the Simard codebase (pub functions, pub structs, pub traits). Plan: (1) which APIs are stable vs experimental, (2) which API changes would be breaking (removing fields, changing types, removing functions), (3) whether the current API uses any deprecation markers (#[deprecated]), (4) a versioning strategy for introducing a v2 of a core trait while maintaining the v1 interface. Produce an API migration roadmap.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "migration-dependency-upgrade-plan",
        title: "Plan major dependency upgrade migration",
        description: "Analyze the impact of upgrading a major dependency and produce a step-by-step migration plan.",
        class: BenchmarkClass::MigrationPlanning,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the Cargo.toml dependencies and select the most impactful major dependency (e.g., tokio, serde, or another core crate). Plan: (1) which modules directly depend on this crate, (2) which API changes a major version bump would introduce, (3) whether any transitive dependencies would conflict, (4) a step-by-step migration plan with intermediate checkpoints where the build can be verified. Produce a dependency upgrade migration plan.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 5: ObservabilityInstrumentation scenarios ---
    BenchmarkScenario {
        id: "observability-logging-audit",
        title: "Audit logging coverage and consistency",
        description: "Analyze logging statements across the codebase for coverage gaps, inconsistent log levels, and missing context.",
        class: BenchmarkClass::ObservabilityInstrumentation,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Scan the Simard codebase for all logging statements (log::info!, log::warn!, log::error!, tracing::info!, etc.). Assess: (1) whether error paths consistently log before returning errors, (2) whether log levels are used appropriately (debug for verbose, info for milestones, warn for recoverable issues, error for failures), (3) whether log messages include sufficient context (identifiers, state values), (4) which modules lack any logging. Produce a logging coverage report with specific recommendations.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "observability-tracing-instrumentation",
        title: "Design tracing instrumentation for request flows",
        description: "Design a tracing strategy to instrument key request flows with spans, events, and context propagation.",
        class: BenchmarkClass::ObservabilityInstrumentation,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the key execution flows in the Simard codebase (session lifecycle, message handling, state transitions). Design: (1) where tracing spans should be placed to capture the full request lifecycle, (2) which fields should be recorded on each span (session_id, phase, objective), (3) whether existing instrumentation (if any) follows OpenTelemetry conventions, (4) how span context should propagate across async task boundaries. Produce a tracing instrumentation plan with specific code locations.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "observability-metrics-design",
        title: "Design metrics collection for runtime health",
        description: "Design a metrics collection strategy covering runtime health indicators, throughput, latency, and error rates.",
        class: BenchmarkClass::ObservabilityInstrumentation,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the Simard runtime for key health indicators that should be measured. Design: (1) which counters to track (sessions started, messages processed, errors encountered), (2) which histograms to track (session duration, message latency, state transition time), (3) which gauges to track (active sessions, queue depth, memory usage), (4) labeling strategy for dimensional metrics (by topology, by phase, by error type). Produce a metrics specification with metric names, types, and collection points.",
        expected_min_runtime_evidence: 3,
    },
    // --- Wave 5: DataModeling scenarios ---
    BenchmarkScenario {
        id: "data-modeling-entity-relationship-map",
        title: "Map entity relationships across domain types",
        description: "Analyze domain types to produce an entity-relationship map showing ownership, references, and lifecycle dependencies.",
        class: BenchmarkClass::DataModeling,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the core domain types in the Simard codebase (session, runtime, memory, evidence, handoff structures). Map: (1) which types contain or reference other types (ownership vs borrowing), (2) lifecycle dependencies (which entities must exist before others can be created), (3) whether any circular references exist between types, (4) which types serve as aggregate roots vs value objects. Produce an entity-relationship diagram description with cardinality annotations.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "data-modeling-serialization-consistency",
        title: "Audit serialization format consistency across types",
        description: "Check that serialization conventions (field naming, optional handling, enum representation) are consistent across all serializable types.",
        class: BenchmarkClass::DataModeling,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Audit all types that derive Serialize or Deserialize in the Simard codebase. Check: (1) whether serde rename conventions are consistent (kebab-case vs snake_case vs camelCase), (2) whether Option fields consistently use skip_serializing_if or not, (3) whether enum serialization is consistent (externally tagged vs internally tagged vs untagged), (4) whether any types use custom serializers that could produce surprising output. Produce a serialization consistency report.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "data-modeling-type-safety-assessment",
        title: "Assess type safety of domain model boundaries",
        description: "Evaluate whether the type system effectively prevents invalid states and enforces domain invariants at compile time.",
        class: BenchmarkClass::DataModeling,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the domain model types in the Simard codebase for type safety. Assess: (1) whether enums are used to represent finite state sets instead of stringly-typed fields, (2) whether newtypes or wrapper types prevent mixing up identifiers (session ID vs suite ID), (3) whether builder patterns or constructor functions enforce required fields, (4) whether any pub fields allow construction of invalid states. Produce a type safety assessment with specific improvement recommendations.",
        expected_min_runtime_evidence: 3,
    },
    // --- wave 6: topology and base_type diversity ---
    BenchmarkScenario {
        id: "repo-exploration-distributed-copilot",
        title: "Repo exploration on distributed copilot-sdk",
        description: "Exercise a repo-exploration task through the distributed topology with the copilot-sdk base type to cover an under-represented topology–base_type pair.",
        class: BenchmarkClass::RepoExploration,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Inspect repository structure using the distributed topology. Identify top-level modules, summarize their responsibilities, and verify that distributed transport is selected in the runtime report.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "bug-fix-multiprocess-terminal-shell",
        title: "Bug-fix scenario on multi-process terminal-shell",
        description: "Exercise a bug-fix benchmark through the multi-process topology with the terminal-shell base type, covering a combination absent from earlier waves.",
        class: BenchmarkClass::BugFix,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::MultiProcess,
        objective: "Identify a plausible bug surface in the codebase, propose a minimal fix, and produce evidence that the loopback multi-process transport was active during the session.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "test-writing-distributed-rusty-clawd",
        title: "Test writing on distributed rusty-clawd",
        description: "Exercise a test-writing benchmark on the distributed topology with the rusty-clawd base type.",
        class: BenchmarkClass::TestWriting,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::Distributed,
        objective: "Analyze existing test coverage in the gym module and propose concrete new test cases. Verify that the distributed runtime backend is reflected in the runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "security-audit-multiprocess-copilot",
        title: "Security audit on multi-process copilot-sdk",
        description: "Exercise a security-audit benchmark through the multi-process topology with the copilot-sdk base type.",
        class: BenchmarkClass::SecurityAudit,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Audit the runtime and session modules for security-relevant patterns: (1) untrusted input handling, (2) error messages that leak internals, (3) unsafe blocks, (4) privilege boundaries. Confirm multi-process transport is active in the runtime report.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "refactoring-distributed-terminal-shell",
        title: "Refactoring scenario on distributed terminal-shell",
        description: "Exercise a refactoring benchmark through the distributed topology with the terminal-shell base type.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::Distributed,
        objective: "Identify a refactoring opportunity in the gym module (extract helper, reduce duplication, or simplify a match arm). Propose the change and verify that distributed transport appears in the runtime evidence.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "error-handling-multiprocess-rusty-clawd",
        title: "Error handling on multi-process rusty-clawd",
        description: "Exercise an error-handling benchmark through the multi-process topology with the rusty-clawd base type.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Trace the error propagation path from a runtime subsystem through the gym executor. Verify: (1) errors are not silently swallowed, (2) context is preserved across module boundaries, (3) the multi-process transport is reflected in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "config-management-distributed-copilot",
        title: "Config management on distributed copilot-sdk",
        description: "Exercise a config-management benchmark through the distributed topology with the copilot-sdk base type.",
        class: BenchmarkClass::ConfigManagement,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Audit configuration surfaces (environment variables, feature flags, topology parameters) and verify they are documented and validated. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "code-review-multiprocess-terminal-shell",
        title: "Code review on multi-process terminal-shell",
        description: "Exercise a code-review benchmark through the multi-process topology with the terminal-shell base type.",
        class: BenchmarkClass::CodeReview,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::MultiProcess,
        objective: "Perform a structured code review of the gym executor module. Evaluate: (1) function length and complexity, (2) error handling consistency, (3) naming conventions, (4) separation of concerns. Verify multi-process transport is active in the runtime report.",
        expected_min_runtime_evidence: 4,
    },
    // --- Wave 7: new BenchmarkClass variants (DataMigration, CicdPipeline,
    // DependencyUpgrade, ReleaseManagement). Each class has at least one
    // non-default topology and at least one non-local-harness base_type.
    BenchmarkScenario {
        id: "data-migration-schema-version-bump",
        title: "Data migration schema version bump",
        description: "Plan a backward-compatible schema/data migration between two versions of a persisted record. Scored on whether the analysis names old and new fields, addresses backfill, and identifies a rollback path.",
        class: BenchmarkClass::DataMigration,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Inspect a serializable struct in src/session or src/memory. Propose a schema migration that adds a new optional field while preserving deserialization of existing records. Describe: (1) the old vs. new schema, (2) how existing data is read (default values, serde defaults), (3) a backfill or lazy upgrade strategy, (4) a rollback path if the migration must be reverted.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "data-migration-multiprocess-rusty-clawd",
        title: "Data migration on multi-process rusty-clawd",
        description: "Exercise a data-migration benchmark through the multi-process topology with the rusty-clawd base type so distributed migration steps and runtime evidence are exercised together.",
        class: BenchmarkClass::DataMigration,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Plan a data migration that must run across multiple processes (e.g., session records persisted by one node and consumed by another). Identify: (1) the schema delta, (2) ordering constraints between writers and readers, (3) compatibility windows during rollout, (4) confirm the multi-process transport is reflected in the runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "data-migration-distributed-copilot",
        title: "Data migration on distributed copilot-sdk",
        description: "Exercise a data-migration benchmark through the distributed topology with the copilot-sdk base type, focusing on coordination of schema upgrades across distributed nodes.",
        class: BenchmarkClass::DataMigration,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Design a phased migration of a stored record format across distributed nodes. Address: (1) versioned read paths, (2) feature-flagged write paths, (3) compatibility window strategy, (4) deprecation timeline. Verify the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "cicd-pipeline-workflow-author",
        title: "Author a GitHub Actions workflow",
        description: "Author a minimal but correct GitHub Actions workflow file for a Rust crate. Scored on whether the workflow names jobs, pins actions to versions, runs cargo fmt/check/test, and uses a sensible matrix or trigger.",
        class: BenchmarkClass::CicdPipeline,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Draft a .github/workflows/ci.yml that: (1) triggers on push and pull_request, (2) defines at least one job named build with steps for cargo fmt --check, cargo check --lib, and cargo test --lib, (3) pins actions/checkout and dtolnay/rust-toolchain to specific versions, (4) caches the cargo registry. Describe each section and why it is structured that way.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "cicd-pipeline-multiprocess-copilot",
        title: "CI/CD pipeline review on multi-process copilot-sdk",
        description: "Review and improve a CI/CD pipeline through the multi-process topology with the copilot-sdk base type so workflow analysis happens alongside cross-process runtime evidence.",
        class: BenchmarkClass::CicdPipeline,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Inspect the existing GitHub Actions workflows in .github/workflows. Identify: (1) jobs that lack timeouts or concurrency limits, (2) action versions that are unpinned (uses floating tags), (3) opportunities for caching or matrix testing, (4) steps that could be parallelized. Confirm the multi-process transport appears in the runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "cicd-pipeline-debug-failure-rusty-clawd",
        title: "Debug a failing CI/CD pipeline on rusty-clawd",
        description: "Diagnose a failing CI workflow run through the multi-process topology with the rusty-clawd base type. Scored on root-cause identification and remediation.",
        class: BenchmarkClass::CicdPipeline,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Given a hypothetical failing workflow run, walk through the diagnostic steps: (1) inspect the failed job and its step logs, (2) classify the failure (flaky test, dependency drift, environment issue, code regression), (3) propose a fix and a re-run strategy, (4) suggest a guard (timeout, retry, cache key change) that prevents recurrence. Confirm the multi-process transport is active in the runtime report.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "dependency-upgrade-major-bump",
        title: "Major-version dependency upgrade analysis",
        description: "Plan a major-version upgrade of a Cargo dependency. Scored on whether the analysis surfaces breaking-change call sites, identifies an upgrade order, and proposes a verification strategy.",
        class: BenchmarkClass::DependencyUpgrade,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Pick a non-trivial dependency from Cargo.toml. Plan a major-version upgrade by: (1) listing changelog/breaking-change categories from the new release, (2) enumerating call sites in the Simard source that would need updating, (3) sequencing the upgrade across dependent crates, (4) describing the cargo check / cargo test verification gates. Output a concrete step-by-step plan.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "dependency-upgrade-multiprocess-copilot",
        title: "Dependency upgrade on multi-process copilot-sdk",
        description: "Exercise a dependency-upgrade benchmark through the multi-process topology with the copilot-sdk base type so upgrade impact is analyzed alongside cross-process runtime behavior.",
        class: BenchmarkClass::DependencyUpgrade,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Analyze how a hypothetical major bump of a transport-related dependency would ripple through the multi-process runtime. Identify: (1) trait or API surface changes, (2) impacted modules under src/runtime, (3) compatibility risk for in-flight sessions, (4) a staged rollout strategy. Confirm the multi-process transport is reflected in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "dependency-upgrade-distributed-rusty-clawd",
        title: "Dependency upgrade on distributed rusty-clawd",
        description: "Exercise a dependency-upgrade benchmark through the distributed topology with the rusty-clawd base type, focusing on coordinated rollout across distributed nodes.",
        class: BenchmarkClass::DependencyUpgrade,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::Distributed,
        objective: "Plan a coordinated dependency upgrade across distributed runtime nodes. Address: (1) wire-format compatibility during partial rollout, (2) feature-flag gating, (3) regression testing matrix, (4) rollback procedure. Verify the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "release-management-changelog-and-tag",
        title: "Release management: changelog, version bump, tag",
        description: "Author a release flow: changelog entry, version bump, and git tag. Scored on whether the plan covers semantic-versioning impact, generates a coherent changelog grouped by type, and proposes a tag/release notes flow.",
        class: BenchmarkClass::ReleaseManagement,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Plan a release for the Simard crate covering: (1) the next semver level (patch/minor/major) and why, (2) Cargo.toml version bump location and downstream crate updates, (3) a CHANGELOG section grouped by Added/Changed/Fixed/Deprecated, (4) the git tag and GitHub release notes template. Output the concrete steps an operator would run.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "release-management-multiprocess-copilot",
        title: "Release management on multi-process copilot-sdk",
        description: "Exercise a release-management benchmark through the multi-process topology with the copilot-sdk base type so release coordination is exercised alongside cross-process runtime evidence.",
        class: BenchmarkClass::ReleaseManagement,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Coordinate a release that touches both the runtime and the operator-facing CLI. Cover: (1) version bump propagation across workspace crates, (2) changelog entries split by audience (operators, integrators), (3) tagging and release-asset publication, (4) post-release verification (smoke run, gym suite). Confirm the multi-process transport is reflected in the runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "release-management-distributed-terminal-shell",
        title: "Release management on distributed terminal-shell",
        description: "Exercise a release-management benchmark through the distributed topology with the terminal-shell base type, focusing on cutover sequencing across distributed nodes.",
        class: BenchmarkClass::ReleaseManagement,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::Distributed,
        objective: "Plan the cutover of a release across distributed runtime nodes. Address: (1) tag and artifact distribution, (2) phased node rollout order, (3) compatibility window with the previous version, (4) rollback and post-release monitoring. Verify the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    // --- Wave 8: AccessibilityReview / InternationalizationReview / IncidentResponse ---
    BenchmarkScenario {
        id: "a11y-aria-audit-local",
        title: "Accessibility review: ARIA and semantic markup audit",
        description: "Audit a sample UI surface for ARIA role correctness, alt text, label association, and screen reader friendliness on the single-process local harness.",
        class: BenchmarkClass::AccessibilityReview,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Audit a UI surface for accessibility. Cover: (1) ARIA roles, states, and properties used (or missing), (2) image alt text and form label associations, (3) screen reader landmark structure, (4) cite at least one specific WCAG 2.1 success criterion (e.g., 1.1.1, 4.1.2). Propose concrete remediations.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "a11y-keyboard-nav-multiprocess-copilot",
        title: "Accessibility review: keyboard navigation on multi-process copilot-sdk",
        description: "Review keyboard navigation, focus order, and visible focus indicators through the multi-process topology with the copilot-sdk base type.",
        class: BenchmarkClass::AccessibilityReview,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Review keyboard navigation across an interactive surface. Cover: (1) tab/shift-tab focus order, (2) visible focus indicator and contrast against adjacent colors, (3) keyboard traps and skip-to-content affordances, (4) WCAG success criteria 2.1.1, 2.4.3, and 2.4.7. Propose remediations and confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "a11y-color-contrast-distributed-terminal",
        title: "Accessibility review: color contrast on distributed terminal-shell",
        description: "Audit color contrast ratios and non-color affordances across themes through the distributed topology with the terminal-shell base type.",
        class: BenchmarkClass::AccessibilityReview,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::Distributed,
        objective: "Audit color usage for accessibility. Cover: (1) computed contrast ratios for foreground/background pairs against WCAG AA (4.5:1 normal, 3:1 large) and AAA targets, (2) non-color affordances for status (icons, text), (3) high-contrast/dark theme parity, (4) cite WCAG 1.4.3 and 1.4.11. Propose remediations and confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "i18n-string-extraction-local",
        title: "Internationalization review: hardcoded string extraction",
        description: "Audit a module for hardcoded user-facing strings and propose an extraction-to-message-catalog plan on the single-process local harness.",
        class: BenchmarkClass::InternationalizationReview,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Find user-facing hardcoded strings and design a localization plan. Cover: (1) inventory of hardcoded message literals and their call sites, (2) proposed message catalog format (e.g., ICU MessageFormat, gettext .po, fluent .ftl), (3) translation key naming convention and fallback locale strategy, (4) at least one example before/after for a non-trivial message.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "i18n-locale-routing-multiprocess-rusty-clawd",
        title: "Internationalization review: locale routing on multi-process rusty-clawd",
        description: "Design locale negotiation, fallback, and per-request locale routing through the multi-process topology with the rusty-clawd base type.",
        class: BenchmarkClass::InternationalizationReview,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Design locale routing for a multi-process service. Cover: (1) locale negotiation from Accept-Language and explicit user preference, (2) language tag normalization (BCP 47, e.g., en-US, pt-BR) and CLDR-backed fallback chain, (3) per-request locale propagation across processes, (4) cache key partitioning by locale. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "i18n-pluralization-rtl-distributed-copilot",
        title: "Internationalization review: pluralization and RTL on distributed copilot-sdk",
        description: "Address plural rules, bidirectional (RTL) layout, and locale-aware number/date/currency formatting through the distributed topology with the copilot-sdk base type.",
        class: BenchmarkClass::InternationalizationReview,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Address advanced i18n concerns. Cover: (1) CLDR plural categories (zero/one/two/few/many/other) and how messages express them, (2) RTL/bidi layout mirroring for languages like Arabic and Hebrew, (3) locale-aware date format, number format, and currency format, (4) at least one concrete example per concern. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "incident-response-postmortem-local",
        title: "Incident response: blameless postmortem authoring",
        description: "Author a blameless postmortem for a simulated production incident on the single-process local harness.",
        class: BenchmarkClass::IncidentResponse,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Author a blameless postmortem. Cover: (1) reconstructed incident timeline (alert paged, mitigation started, resolved), (2) root cause and contributing factors distinguished from triggers, (3) customer impact and severity, (4) prioritized follow-up action items with owners. Avoid blame; focus on systemic prevention.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "incident-response-runbook-multiprocess-terminal",
        title: "Incident response: runbook authoring on multi-process terminal-shell",
        description: "Draft an operational runbook for a recurring failure mode through the multi-process topology with the terminal-shell base type.",
        class: BenchmarkClass::IncidentResponse,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::MultiProcess,
        objective: "Draft a runbook for an on-call responder. Cover: (1) detection signals and alert query, (2) step-by-step triage and mitigation commands, (3) escalation path and communication template, (4) verification and rollback steps. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "incident-response-pager-rotation-distributed-copilot",
        title: "Incident response: pager rotation and follow-up on distributed copilot-sdk",
        description: "Plan an on-call pager rotation, incident command structure, and follow-up tracking through the distributed topology with the copilot-sdk base type.",
        class: BenchmarkClass::IncidentResponse,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Plan distributed incident response. Cover: (1) on-call pager rotation and handoff procedure, (2) incident commander/scribe/communications role assignments, (3) cross-region escalation and runbook distribution, (4) postmortem follow-up tracking and prevention metrics. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    // --- Wave 9: PerformanceAnalysis / Refactoring / DataModeling / DataMigration topology diversity ---
    BenchmarkScenario {
        id: "perf-hotpath-profiling-multiprocess-copilot",
        title: "Hot-path profiling on multi-process copilot-sdk",
        description: "Profile a request-handling hot path to identify CPU-bound bottlenecks through the multi-process topology with the copilot-sdk base type.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Profile the request-handling path in the Simard runtime. Cover: (1) identify the top three CPU-consuming functions on the critical path using flame-graph reasoning, (2) measure or estimate the percentage of wall-clock time each consumes, (3) propose targeted optimizations (algorithmic, data-structure, or batching changes) for the top bottleneck, (4) describe how to validate the improvement with before/after benchmarks. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "perf-serialization-overhead-distributed-terminal",
        title: "Serialization overhead analysis on distributed terminal-shell",
        description: "Analyze serialization and deserialization overhead in cross-boundary data exchange through the distributed topology with the terminal-shell base type.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::Distributed,
        objective: "Analyze serialization overhead in the Simard codebase. Cover: (1) identify all serde round-trip points where data crosses module or process boundaries, (2) estimate payload sizes for representative objects (BenchmarkRunReport, RuntimeHandoffSnapshot), (3) compare current format (JSON) cost against alternatives (bincode, MessagePack) in terms of size and speed, (4) propose a migration path for the highest-cost serialization site. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "perf-lock-contention-multiprocess-rusty-clawd",
        title: "Lock contention analysis on multi-process rusty-clawd",
        description: "Identify lock contention and synchronization bottlenecks in shared-state access through the multi-process topology with the rusty-clawd base type.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Analyze synchronization primitives in the Simard codebase. Cover: (1) inventory all Mutex, RwLock, and Arc usages and their protected data, (2) identify potential contention points where multiple threads or tasks compete for the same lock, (3) assess whether any locks are held across await points or I/O operations, (4) propose lock-free or reduced-contention alternatives (e.g., sharding, per-thread state, channels). Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "perf-startup-latency-distributed-copilot",
        title: "Startup latency breakdown on distributed copilot-sdk",
        description: "Break down application startup latency into initialization phases and identify optimization targets through the distributed topology with the copilot-sdk base type.",
        class: BenchmarkClass::PerformanceAnalysis,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Analyze startup latency of the Simard runtime. Cover: (1) enumerate initialization phases (config loading, registry population, transport setup, prompt asset loading), (2) estimate or measure the cost of each phase, (3) identify which phases can be parallelized or deferred (lazy initialization), (4) propose a concrete startup optimization plan with expected latency reduction. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "refactor-trait-consolidation-multiprocess-copilot",
        title: "Trait consolidation refactoring on multi-process copilot-sdk",
        description: "Identify related traits that can be consolidated into a unified interface through the multi-process topology with the copilot-sdk base type.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::MultiProcess,
        objective: "Identify traits in the Simard codebase that share overlapping responsibilities and could be consolidated. Cover: (1) find trait pairs with similar method signatures or shared implementors, (2) analyze whether consolidation would reduce boilerplate without losing semantic clarity, (3) propose a merged trait definition with before/after code, (4) list all implementors that would need updating and the mechanical changes required. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "refactor-error-type-hierarchy-multiprocess-rusty-clawd",
        title: "Error type hierarchy refactoring on multi-process rusty-clawd",
        description: "Restructure error types to improve ergonomics and reduce boilerplate through the multi-process topology with the rusty-clawd base type.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Analyze the error type hierarchy in the Simard codebase. Cover: (1) map the current SimardError variants and their usage frequency, (2) identify variants that are overly broad or overly narrow, (3) propose a restructured error hierarchy that groups related variants into sub-enums or uses thiserror more effectively, (4) show before/after for at least two error handling call sites. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "refactor-module-boundary-distributed-copilot",
        title: "Module boundary restructuring on distributed copilot-sdk",
        description: "Restructure module boundaries to reduce coupling and improve encapsulation through the distributed topology with the copilot-sdk base type.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Analyze module boundaries in the Simard codebase for coupling issues. Cover: (1) identify modules with high fan-in or fan-out (many cross-module imports), (2) find pub items that are only used by one other module (candidates for moving or inlining), (3) propose a restructured module layout that reduces cross-module dependencies, (4) describe the migration steps to restructure without breaking the public API. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "refactor-api-migration-distributed-terminal",
        title: "API migration planning on distributed terminal-shell",
        description: "Plan a backward-compatible API migration from a deprecated interface to a new one through the distributed topology with the terminal-shell base type.",
        class: BenchmarkClass::Refactoring,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::Distributed,
        objective: "Plan a staged API migration for a public interface in the Simard codebase. Cover: (1) identify a function or trait whose signature should change (e.g., adding a parameter, changing a return type), (2) design a backward-compatible migration with a deprecation period (old API delegates to new), (3) describe the versioned rollout: introduce new API, migrate callers, remove old API, (4) specify how to detect stale callers via compiler warnings or lint rules. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "data-modeling-pipeline-topology-multiprocess-rusty-clawd",
        title: "Data pipeline topology design on multi-process rusty-clawd",
        description: "Design a data pipeline topology for ETL-style processing of benchmark results through the multi-process topology with the rusty-clawd base type.",
        class: BenchmarkClass::DataModeling,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::MultiProcess,
        objective: "Design a data pipeline for processing benchmark run results. Cover: (1) define pipeline stages (extract from JSON artifacts, transform/normalize scores, load into a summary store), (2) specify the data contract between each stage (input type, output type, error type), (3) design backpressure and batching strategy for large result sets, (4) describe how pipeline stages would be distributed across processes. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "data-modeling-stream-validation-distributed-copilot",
        title: "Streaming data validation on distributed copilot-sdk",
        description: "Design a streaming validation layer for incoming data records with schema enforcement and error quarantine through the distributed topology with the copilot-sdk base type.",
        class: BenchmarkClass::DataModeling,
        identity: "simard-gym",
        base_type: "copilot-sdk",
        topology: RuntimeTopology::Distributed,
        objective: "Design a streaming data validation pipeline. Cover: (1) define a validation schema for incoming MemoryRecord or EvidenceRecord payloads (required fields, type constraints, value ranges), (2) design a validation stage that emits valid records downstream and quarantines invalid ones with error annotations, (3) specify how schema evolution is handled (adding optional fields, deprecating fields), (4) describe monitoring and alerting for validation failure rates. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "data-migration-batch-etl-multiprocess-terminal",
        title: "Batch ETL migration on multi-process terminal-shell",
        description: "Design a batch ETL process to migrate historical benchmark data between storage formats through the multi-process topology with the terminal-shell base type.",
        class: BenchmarkClass::DataMigration,
        identity: "simard-gym",
        base_type: "terminal-shell",
        topology: RuntimeTopology::MultiProcess,
        objective: "Design a batch ETL migration for historical benchmark data. Cover: (1) extract phase: enumerate and read existing JSON report artifacts, (2) transform phase: normalize field names, fill defaults for missing fields, compute derived metrics, (3) load phase: write transformed records to a new storage format, (4) verification: checksum-based integrity checks and row-count reconciliation between source and target. Confirm the multi-process transport appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "data-migration-streaming-backfill-distributed-rusty-clawd",
        title: "Streaming backfill migration on distributed rusty-clawd",
        description: "Design a streaming backfill process to populate a new data store from an existing one without downtime through the distributed topology with the rusty-clawd base type.",
        class: BenchmarkClass::DataMigration,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::Distributed,
        objective: "Design a zero-downtime streaming backfill for migrating data across stores. Cover: (1) dual-write strategy: new writes go to both old and new stores during migration, (2) backfill cursor: track progress through historical records with resumable checkpoints, (3) consistency verification: compare record counts and checksums between stores, (4) cutover procedure: switch reads to the new store and decommission the old one. Confirm the distributed topology backend appears in runtime evidence.",
        expected_min_runtime_evidence: 4,
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
        BenchmarkClass::CodeReview => {
            let review_findings = combined.contains("finding")
                || combined.contains("issue")
                || combined.contains("concern")
                || combined.contains("inconsisten")
                || combined.contains("review");
            let severity_assessed = combined.contains("severity")
                || combined.contains("critical")
                || combined.contains("minor")
                || combined.contains("major")
                || combined.contains("nit");
            let fix_suggested = combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("fix")
                || combined.contains("improv")
                || combined.contains("should");
            vec![
                BenchmarkCheckResult {
                    id: "review-findings-present".to_string(),
                    passed: review_findings,
                    detail: format!(
                        "execution output {} review findings",
                        if review_findings { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "review-severity-assessed".to_string(),
                    passed: severity_assessed,
                    detail: format!(
                        "execution output {} severity assessment",
                        if severity_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "review-fix-suggested".to_string(),
                    passed: fix_suggested,
                    detail: format!(
                        "execution output {} fix suggestions",
                        if fix_suggested { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::Debugging => {
            let root_cause_traced = combined.contains("trace")
                || combined.contains("origin")
                || combined.contains("root cause")
                || combined.contains("source of")
                || combined.contains("caused by");
            let call_path_analyzed = combined.contains("call")
                || combined.contains("stack")
                || combined.contains("propagat")
                || combined.contains("invoked")
                || combined.contains("transition");
            let diagnostic_suggested = combined.contains("diagnostic")
                || combined.contains("debug")
                || combined.contains("log")
                || combined.contains("inspect")
                || combined.contains("breakpoint");
            vec![
                BenchmarkCheckResult {
                    id: "debug-root-cause-traced".to_string(),
                    passed: root_cause_traced,
                    detail: format!(
                        "execution output {} root cause tracing",
                        if root_cause_traced {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "debug-call-path-analyzed".to_string(),
                    passed: call_path_analyzed,
                    detail: format!(
                        "execution output {} call path analysis",
                        if call_path_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "debug-diagnostic-suggested".to_string(),
                    passed: diagnostic_suggested,
                    detail: format!(
                        "execution output {} diagnostic suggestions",
                        if diagnostic_suggested {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ConfigManagement => {
            let config_inventoried = combined.contains("config")
                || combined.contains("feature")
                || combined.contains("env")
                || combined.contains("cargo.toml")
                || combined.contains("setting");
            let validation_checked = combined.contains("valid")
                || combined.contains("default")
                || combined.contains("missing")
                || combined.contains("required")
                || combined.contains("optional");
            let matrix_produced = combined.contains("matrix")
                || combined.contains("table")
                || combined.contains("inventory")
                || combined.contains("summary")
                || combined.contains("report");
            vec![
                BenchmarkCheckResult {
                    id: "config-inventoried".to_string(),
                    passed: config_inventoried,
                    detail: format!(
                        "execution output {} configuration inventory",
                        if config_inventoried {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "config-validation-checked".to_string(),
                    passed: validation_checked,
                    detail: format!(
                        "execution output {} validation assessment",
                        if validation_checked {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "config-matrix-produced".to_string(),
                    passed: matrix_produced,
                    detail: format!(
                        "execution output {} configuration matrix",
                        if matrix_produced { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::ConcurrencyAnalysis => {
            let race_condition_analyzed = combined.contains("race")
                || combined.contains("concurrent")
                || combined.contains("shared")
                || combined.contains("mutex")
                || combined.contains("atomic");
            let synchronization_assessed = combined.contains("lock")
                || combined.contains("synchroniz")
                || combined.contains("rwlock")
                || combined.contains("channel")
                || combined.contains("arc");
            let safety_evaluated = combined.contains("deadlock")
                || combined.contains("safe")
                || combined.contains("cancel")
                || combined.contains("await")
                || combined.contains("spawn");
            vec![
                BenchmarkCheckResult {
                    id: "concurrency-race-analyzed".to_string(),
                    passed: race_condition_analyzed,
                    detail: format!(
                        "execution output {} race condition analysis",
                        if race_condition_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "concurrency-sync-assessed".to_string(),
                    passed: synchronization_assessed,
                    detail: format!(
                        "execution output {} synchronization assessment",
                        if synchronization_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "concurrency-safety-evaluated".to_string(),
                    passed: safety_evaluated,
                    detail: format!(
                        "execution output {} concurrency safety evaluation",
                        if safety_evaluated {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::MigrationPlanning => {
            let migration_scope_defined = combined.contains("migrat")
                || combined.contains("schema")
                || combined.contains("version")
                || combined.contains("upgrade")
                || combined.contains("evolution");
            let compatibility_assessed = combined.contains("compat")
                || combined.contains("backward")
                || combined.contains("breaking")
                || combined.contains("deprecat")
                || combined.contains("serde");
            let plan_produced = combined.contains("step")
                || combined.contains("plan")
                || combined.contains("phase")
                || combined.contains("roadmap")
                || combined.contains("checkpoint");
            vec![
                BenchmarkCheckResult {
                    id: "migration-scope-defined".to_string(),
                    passed: migration_scope_defined,
                    detail: format!(
                        "execution output {} migration scope definition",
                        if migration_scope_defined {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "migration-compatibility-assessed".to_string(),
                    passed: compatibility_assessed,
                    detail: format!(
                        "execution output {} compatibility assessment",
                        if compatibility_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "migration-plan-produced".to_string(),
                    passed: plan_produced,
                    detail: format!(
                        "execution output {} migration plan",
                        if plan_produced { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::ObservabilityInstrumentation => {
            let instrumentation_analyzed = combined.contains("log")
                || combined.contains("trac")
                || combined.contains("metric")
                || combined.contains("instrument")
                || combined.contains("observab");
            let coverage_assessed = combined.contains("coverage")
                || combined.contains("gap")
                || combined.contains("missing")
                || combined.contains("module")
                || combined.contains("path");
            let recommendation_present = combined.contains("recommend")
                || combined.contains("suggest")
                || combined.contains("should")
                || combined.contains("add")
                || combined.contains("design");
            vec![
                BenchmarkCheckResult {
                    id: "observability-instrumentation-analyzed".to_string(),
                    passed: instrumentation_analyzed,
                    detail: format!(
                        "execution output {} instrumentation analysis",
                        if instrumentation_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "observability-coverage-assessed".to_string(),
                    passed: coverage_assessed,
                    detail: format!(
                        "execution output {} coverage assessment",
                        if coverage_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "observability-recommendation-present".to_string(),
                    passed: recommendation_present,
                    detail: format!(
                        "execution output {} observability recommendations",
                        if recommendation_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DataModeling => {
            let model_analyzed = combined.contains("type")
                || combined.contains("struct")
                || combined.contains("entity")
                || combined.contains("field")
                || combined.contains("schema");
            let relationships_mapped = combined.contains("relation")
                || combined.contains("reference")
                || combined.contains("owner")
                || combined.contains("contain")
                || combined.contains("cardinality");
            let quality_assessed = combined.contains("consisten")
                || combined.contains("safety")
                || combined.contains("invalid")
                || combined.contains("invariant")
                || combined.contains("newtype");
            vec![
                BenchmarkCheckResult {
                    id: "data-model-analyzed".to_string(),
                    passed: model_analyzed,
                    detail: format!(
                        "execution output {} data model analysis",
                        if model_analyzed { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-relationships-mapped".to_string(),
                    passed: relationships_mapped,
                    detail: format!(
                        "execution output {} relationship mapping",
                        if relationships_mapped {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-quality-assessed".to_string(),
                    passed: quality_assessed,
                    detail: format!(
                        "execution output {} data quality assessment",
                        if quality_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DataMigration => {
            let schema_delta_described = combined.contains("schema")
                || combined.contains("field")
                || combined.contains("column")
                || combined.contains("version")
                || combined.contains("migrat");
            let compatibility_addressed = combined.contains("backward")
                || combined.contains("forward")
                || combined.contains("compat")
                || combined.contains("default")
                || combined.contains("optional")
                || combined.contains("serde");
            let rollout_or_rollback_planned = combined.contains("backfill")
                || combined.contains("rollout")
                || combined.contains("rollback")
                || combined.contains("phased")
                || combined.contains("revert")
                || combined.contains("compatibility window");
            vec![
                BenchmarkCheckResult {
                    id: "data-migration-schema-delta-described".to_string(),
                    passed: schema_delta_described,
                    detail: format!(
                        "execution output {} schema delta description",
                        if schema_delta_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-migration-compatibility-addressed".to_string(),
                    passed: compatibility_addressed,
                    detail: format!(
                        "execution output {} compatibility analysis",
                        if compatibility_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-migration-rollout-or-rollback-planned".to_string(),
                    passed: rollout_or_rollback_planned,
                    detail: format!(
                        "execution output {} rollout/rollback plan",
                        if rollout_or_rollback_planned {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::CicdPipeline => {
            let workflow_structure_described = combined.contains("workflow")
                || combined.contains("github actions")
                || combined.contains("job")
                || combined.contains("step")
                || combined.contains(".yml")
                || combined.contains(".yaml");
            let trigger_or_pin_addressed = combined.contains("trigger")
                || combined.contains("on:")
                || combined.contains("pull_request")
                || combined.contains("push")
                || combined.contains("pin")
                || combined.contains("uses:")
                || combined.contains("@v");
            let verification_or_remediation_present = combined.contains("cargo")
                || combined.contains("test")
                || combined.contains("check")
                || combined.contains("retry")
                || combined.contains("timeout")
                || combined.contains("cache")
                || combined.contains("matrix");
            vec![
                BenchmarkCheckResult {
                    id: "cicd-workflow-structure-described".to_string(),
                    passed: workflow_structure_described,
                    detail: format!(
                        "execution output {} workflow structure description",
                        if workflow_structure_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "cicd-trigger-or-pin-addressed".to_string(),
                    passed: trigger_or_pin_addressed,
                    detail: format!(
                        "execution output {} trigger/version-pin analysis",
                        if trigger_or_pin_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "cicd-verification-or-remediation-present".to_string(),
                    passed: verification_or_remediation_present,
                    detail: format!(
                        "execution output {} verification/remediation steps",
                        if verification_or_remediation_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DependencyUpgrade => {
            let upgrade_target_named = combined.contains("cargo.toml")
                || combined.contains("dependenc")
                || combined.contains("crate")
                || combined.contains("version")
                || combined.contains("major");
            let breakage_analyzed = combined.contains("breaking")
                || combined.contains("breakage")
                || combined.contains("api change")
                || combined.contains("call site")
                || combined.contains("changelog")
                || combined.contains("deprecat");
            let verification_plan_present = combined.contains("cargo check")
                || combined.contains("cargo test")
                || combined.contains("verify")
                || combined.contains("regression")
                || combined.contains("rollout")
                || combined.contains("rollback")
                || combined.contains("staged");
            vec![
                BenchmarkCheckResult {
                    id: "dep-upgrade-target-named".to_string(),
                    passed: upgrade_target_named,
                    detail: format!(
                        "execution output {} upgrade target identification",
                        if upgrade_target_named {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-upgrade-breakage-analyzed".to_string(),
                    passed: breakage_analyzed,
                    detail: format!(
                        "execution output {} breakage analysis",
                        if breakage_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-upgrade-verification-plan-present".to_string(),
                    passed: verification_plan_present,
                    detail: format!(
                        "execution output {} verification/rollback plan",
                        if verification_plan_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ReleaseManagement => {
            let version_bump_planned = combined.contains("version")
                || combined.contains("semver")
                || combined.contains("bump")
                || combined.contains("patch")
                || combined.contains("minor")
                || combined.contains("major");
            let changelog_authored = combined.contains("changelog")
                || combined.contains("release notes")
                || combined.contains("added")
                || combined.contains("changed")
                || combined.contains("fixed")
                || combined.contains("deprecat");
            let tag_or_cutover_addressed = combined.contains("tag")
                || combined.contains("git tag")
                || combined.contains("release")
                || combined.contains("cutover")
                || combined.contains("rollout")
                || combined.contains("rollback")
                || combined.contains("publish");
            vec![
                BenchmarkCheckResult {
                    id: "release-version-bump-planned".to_string(),
                    passed: version_bump_planned,
                    detail: format!(
                        "execution output {} version-bump plan",
                        if version_bump_planned {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "release-changelog-authored".to_string(),
                    passed: changelog_authored,
                    detail: format!(
                        "execution output {} changelog/release notes",
                        if changelog_authored {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "release-tag-or-cutover-addressed".to_string(),
                    passed: tag_or_cutover_addressed,
                    detail: format!(
                        "execution output {} tag/cutover plan",
                        if tag_or_cutover_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::AccessibilityReview => {
            let a11y_issues_identified = combined.contains("aria")
                || combined.contains("alt text")
                || combined.contains("alt-text")
                || combined.contains("label")
                || combined.contains("screen reader")
                || combined.contains("focus")
                || combined.contains("contrast")
                || combined.contains("keyboard");
            let wcag_or_standard_cited = combined.contains("wcag")
                || combined.contains("level a")
                || combined.contains("level aa")
                || combined.contains("level aaa")
                || combined.contains("success criterion")
                || combined.contains("1.1.1")
                || combined.contains("1.4.3")
                || combined.contains("1.4.11")
                || combined.contains("2.1.1")
                || combined.contains("2.4.3")
                || combined.contains("2.4.7")
                || combined.contains("4.1.2");
            let remediation_proposed = combined.contains("remediat")
                || combined.contains("fix")
                || combined.contains("add ")
                || combined.contains("replace")
                || combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("improve");
            vec![
                BenchmarkCheckResult {
                    id: "a11y-issues-identified".to_string(),
                    passed: a11y_issues_identified,
                    detail: format!(
                        "execution output {} accessibility issue identification",
                        if a11y_issues_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "a11y-wcag-cited".to_string(),
                    passed: wcag_or_standard_cited,
                    detail: format!(
                        "execution output {} WCAG/standard citation",
                        if wcag_or_standard_cited {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "a11y-remediation-proposed".to_string(),
                    passed: remediation_proposed,
                    detail: format!(
                        "execution output {} accessibility remediation",
                        if remediation_proposed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::InternationalizationReview => {
            let localizable_strings_identified = combined.contains("hardcoded")
                || combined.contains("string literal")
                || combined.contains("message catalog")
                || combined.contains("translat")
                || combined.contains("l10n")
                || combined.contains("i18n")
                || combined.contains("localiz")
                || combined.contains("message key");
            let locale_handling_described = combined.contains("locale")
                || combined.contains("language tag")
                || combined.contains("bcp 47")
                || combined.contains("bcp-47")
                || combined.contains("accept-language")
                || combined.contains("fallback")
                || combined.contains("cldr")
                || combined.contains("en-us")
                || combined.contains("pt-br")
                || combined.contains("region");
            let pluralization_or_format_addressed = combined.contains("plural")
                || combined.contains("rtl")
                || combined.contains("bidi")
                || combined.contains("date format")
                || combined.contains("number format")
                || combined.contains("currency")
                || combined.contains("icu")
                || combined.contains("messageformat")
                || combined.contains("fluent")
                || combined.contains("gettext");
            vec![
                BenchmarkCheckResult {
                    id: "i18n-localizable-strings-identified".to_string(),
                    passed: localizable_strings_identified,
                    detail: format!(
                        "execution output {} localizable string identification",
                        if localizable_strings_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "i18n-locale-handling-described".to_string(),
                    passed: locale_handling_described,
                    detail: format!(
                        "execution output {} locale-handling description",
                        if locale_handling_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "i18n-pluralization-or-format-addressed".to_string(),
                    passed: pluralization_or_format_addressed,
                    detail: format!(
                        "execution output {} pluralization/format coverage",
                        if pluralization_or_format_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::IncidentResponse => {
            let timeline_reconstructed = combined.contains("timeline")
                || combined.contains("sequence")
                || combined.contains("when ")
                || combined.contains("started at")
                || combined.contains("alert")
                || combined.contains("paged")
                || combined.contains("detected")
                || combined.contains("resolved at");
            let root_cause_or_contributing_identified = combined.contains("root cause")
                || combined.contains("root-cause")
                || combined.contains("contributing")
                || combined.contains("trigger")
                || combined.contains("cascade")
                || combined.contains("fault")
                || combined.contains("latent")
                || combined.contains("blameless");
            let mitigation_or_followup_proposed = combined.contains("mitigat")
                || combined.contains("action item")
                || combined.contains("follow-up")
                || combined.contains("followup")
                || combined.contains("runbook")
                || combined.contains("postmortem")
                || combined.contains("post-mortem")
                || combined.contains("prevention")
                || combined.contains("escalation")
                || combined.contains("on-call")
                || combined.contains("oncall");
            vec![
                BenchmarkCheckResult {
                    id: "incident-timeline-reconstructed".to_string(),
                    passed: timeline_reconstructed,
                    detail: format!(
                        "execution output {} incident timeline reconstruction",
                        if timeline_reconstructed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "incident-root-cause-or-contributing-identified".to_string(),
                    passed: root_cause_or_contributing_identified,
                    detail: format!(
                        "execution output {} root cause/contributing factor analysis",
                        if root_cause_or_contributing_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "incident-mitigation-or-followup-proposed".to_string(),
                    passed: mitigation_or_followup_proposed,
                    detail: format!(
                        "execution output {} mitigation/follow-up proposal",
                        if mitigation_or_followup_proposed {
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
            (BenchmarkClass::CodeReview, "code-review"),
            (BenchmarkClass::Debugging, "debugging"),
            (BenchmarkClass::ConfigManagement, "config-management"),
            (BenchmarkClass::ConcurrencyAnalysis, "concurrency-analysis"),
            (BenchmarkClass::MigrationPlanning, "migration-planning"),
            (
                BenchmarkClass::ObservabilityInstrumentation,
                "observability-instrumentation",
            ),
            (BenchmarkClass::DataModeling, "data-modeling"),
            (BenchmarkClass::DataMigration, "data-migration"),
            (BenchmarkClass::CicdPipeline, "cicd-pipeline"),
            (BenchmarkClass::DependencyUpgrade, "dependency-upgrade"),
            (BenchmarkClass::ReleaseManagement, "release-management"),
            (BenchmarkClass::AccessibilityReview, "accessibility-review"),
            (
                BenchmarkClass::InternationalizationReview,
                "internationalization-review",
            ),
            (BenchmarkClass::IncidentResponse, "incident-response"),
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
                scenario.identity == "simard-gym"
                    || scenario.identity == "simard-engineer"
                    || scenario.identity == "simard-composite-engineer",
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
                scenario.base_type == "local-harness"
                    || scenario.base_type == "terminal-shell"
                    || scenario.base_type == "copilot-sdk"
                    || scenario.base_type == "rusty-clawd",
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
            BenchmarkClass::CodeReview,
            BenchmarkClass::Debugging,
            BenchmarkClass::ConfigManagement,
            BenchmarkClass::ConcurrencyAnalysis,
            BenchmarkClass::MigrationPlanning,
            BenchmarkClass::ObservabilityInstrumentation,
            BenchmarkClass::DataModeling,
            BenchmarkClass::DataMigration,
            BenchmarkClass::CicdPipeline,
            BenchmarkClass::DependencyUpgrade,
            BenchmarkClass::ReleaseManagement,
            BenchmarkClass::AccessibilityReview,
            BenchmarkClass::InternationalizationReview,
            BenchmarkClass::IncidentResponse,
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
