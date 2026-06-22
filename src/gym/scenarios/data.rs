//! V1 high-signal benchmark scenarios.
//!
//! Per `Specs/ProductArchitecture.md` (lines 196-215) the gym prefers a small,
//! high-signal benchmark set over a large, noisy suite. This file defines a
//! curated set of scenarios — three per sanctioned `BenchmarkClass` — that
//! together exercise the four initial benchmark classes across the local
//! harness and the multi-process / external-backend topologies.
//!
//! Scenarios whose `base_type` requires external authentication (e.g.
//! `rusty-clawd`, `copilot-sdk`) are skipped gracefully at gate-time on stock
//! dev machines (see `gym::is_skippable_auth_error`), so the local-harness and
//! terminal-shell scenarios are the ones that always produce benchmark output.

use super::super::types::{BenchmarkClass, BenchmarkScenario};
use crate::runtime::RuntimeTopology;

// NEEDLE-XYZ-GYM-MARKER: long-context-needle-in-haystack benchmark searches for this exact comment.

pub(super) static SCENARIOS: [BenchmarkScenario; 12] = [
    // --- RepoExploration ---
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
    // --- Documentation ---
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
        id: "doc-generation-public-fn",
        title: "Generate doc comment for public function",
        description: "Given a source file in the Simard repository, generate a syntactically valid Rust doc comment for a public function. Scored on whether the comment is valid rustdoc, mentions parameters and return type, and accurately describes behavior.",
        class: BenchmarkClass::Documentation,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Read the function `pub fn benchmark_scenarios()` in src/gym/scenarios/mod.rs. Generate a complete Rust doc comment (/// style) for it that: (1) describes what the function returns, (2) mentions the BenchmarkScenario type, (3) notes the static lifetime of the returned slice, (4) is syntactically valid rustdoc. Output the doc comment text.",
        expected_min_runtime_evidence: 3,
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
    // --- SafeCodeChange ---
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
        id: "safe-change-add-enum-variant",
        title: "Safely add a new enum variant",
        description: "Safely add a new variant to an existing enum, updating all match arms across the codebase. Scored on whether all match expressions are found and updated, the change compiles, and no fallthrough behavior is introduced.",
        class: BenchmarkClass::SafeCodeChange,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Plan the addition of a new variant to an existing enum in the Simard codebase (e.g., a new RuntimeTopology variant). Describe: (1) the enum to modify and the new variant name, (2) every match expression across the codebase that handles this enum (list file and line for each), (3) what the new arm should do in each match, (4) any Display, Serialize, or other trait implementations that need updating. Verify the plan would result in a compiling codebase with no unhandled match arms.",
        expected_min_runtime_evidence: 3,
    },
    // --- SessionQuality ---
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
];
