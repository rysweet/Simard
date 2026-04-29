//! Auto-split BENCHMARK_SCENARIOS data — chunk 7.
//!
//! ErrorHandlingDebug sub-family (issue #1461): scenarios that ask the agent
//! to diagnose canonical Simard runtime errors and propose the documented
//! remediation. Complements the KnowledgeRecall family added in #1467 — both
//! families were proposed by the prompt-driven OODA brain itself in cycle 2,
//! validating the brain → goal → implementation → observation loop introduced
//! in PR #1458.

use super::super::types::{BenchmarkClass, BenchmarkScenario};
use crate::runtime::RuntimeTopology;

pub(super) static SCENARIOS: [BenchmarkScenario; 4] = [
    BenchmarkScenario {
        id: "error-handling-debug-stale-engineer-worktree",
        title: "Error handling/debug: stale engineer worktree with no live process",
        description: "Verify the agent can diagnose why an engineer subagent's output artifacts are missing from a worktree under ~/.simard/engineer-worktrees/, given that the worktree is alive but the subagent process has exited, and cite the OODA dispatch layer's liveness check.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Diagnose why an engineer subagent's output artifacts are missing from a worktree under ~/.simard/engineer-worktrees/, given that the worktree is alive but the subagent process has exited. Cite the specific check the OODA dispatch layer performs (find_live_engineer_for_goal) and propose a remediation.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "error-handling-debug-pre-push-clippy-failure",
        title: "Error handling/debug: pre-push clippy unused_imports failure",
        description: "Verify the agent can diagnose a pre-push hook failure where clippy fails with a single unused_imports warning under -D warnings, explain why bypassing via --no-verify is forbidden, and propose the correct fix.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Diagnose a pre-push hook failure where 'cargo clippy --all-targets --all-features --locked -- -D warnings' fails with a single 'unused_imports' warning. Explain why bypassing via --no-verify is forbidden and propose the correct fix.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "error-handling-debug-mkdocs-strict-broken-link",
        title: "Error handling/debug: mkdocs strict mode broken cross-document link",
        description: "Verify the agent can diagnose a docs/build CI failure caused by mkdocs strict mode rejecting a broken cross-document link, cite where strict mode is enabled, and explain why links from docs/ to files outside the docs/ tree cannot resolve.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Diagnose a 'docs/build' CI failure caused by mkdocs strict mode rejecting a broken cross-document link. Cite where mkdocs strict mode is enabled (mkdocs.yml) and explain why links from docs/ to files outside the docs/ tree (e.g. prompt_assets/) cannot resolve.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "error-handling-debug-recipe-runner-hollow-success",
        title: "Error handling/debug: smart-orchestrator hollow-success no-op-guard trip",
        description: "Verify the agent can diagnose a smart-orchestrator hollow-success failure where the recipe completed step-08-implement structurally but step-08c-implementation-no-op-guard reported 'produced no output', and propose the documented Opus 4.7 sub-agent fallback remediation.",
        class: BenchmarkClass::ErrorHandling,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Diagnose a smart-orchestrator hollow-success failure: the recipe completed step-08-implement structurally but step-08c-implementation-no-op-guard reported 'produced no output'. Identify the symptom (worktree never created), reference the documented Opus 4.7 sub-agent fallback pattern, and propose the remediation.",
        expected_min_runtime_evidence: 2,
    },
];
