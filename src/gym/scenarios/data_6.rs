//! Auto-split BENCHMARK_SCENARIOS data — chunk 6 of 6.
//!
//! KnowledgeRecall family (issue #1459): scenarios that measure longitudinal
//! learning. Each scenario asks the agent to recall something she should
//! already know — about her own code, her tools, the repos she maintains, or
//! the user's stated preferences — and grounds the answer in stored memories
//! or real repository file paths rather than confabulation.

use super::super::types::{BenchmarkClass, BenchmarkScenario};
use crate::runtime::RuntimeTopology;

pub(super) static SCENARIOS: [BenchmarkScenario; 5] = [
    BenchmarkScenario {
        id: "knowledge-recall-self-code",
        title: "Knowledge recall: locate the OodaBrain trait and its wire-in site",
        description: "Verify the agent can recall structural facts about her own codebase: which file defines the OodaBrain trait and where in the OODA action layer it is wired in.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Identify the file containing the OodaBrain trait definition and cite its single wire-in site in the OODA action layer.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "knowledge-recall-user-preference",
        title: "Knowledge recall: user stance on --no-verify",
        description: "Verify the agent can recall a user-stated preference — the prohibition on bypassing pre-push verification with --no-verify — and cite the approved alternative for known-flaky local tests.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall the user-mandated stance on bypassing pre-push verification (--no-verify) and explain the approved alternative for known-flaky local tests.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "knowledge-recall-tool-amplihack-recipe",
        title: "Knowledge recall: amplihack recipe runner invocation",
        description: "Verify the agent can recall how the amplihack recipe runner is invoked for development and investigation work — the sub-command, the recipe name, and at least one required environment variable — rather than confabulating an interface.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall how the amplihack recipe runner is invoked for development and investigation work, including the sub-command, the recipe name, and at least one required environment variable.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "knowledge-recall-tool-pre-push-skip",
        title: "Knowledge recall: SKIP=cargo-test pre-push override",
        description: "Verify the agent can recall the approved environment variable used to skip the cargo-test stage of the local pre-push hook when known-flaky tests are tripping it, and explain why --no-verify is forbidden as a bypass.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall the approved environment variable used to skip the cargo-test stage of the local pre-push hook for known-flaky tests, and explain why --no-verify is forbidden.",
        expected_min_runtime_evidence: 2,
    },
    BenchmarkScenario {
        id: "knowledge-recall-tool-redeploy-script",
        title: "Knowledge recall: redeploy-local.sh and SIMARD_SHARED_TARGET",
        description: "Verify the agent can recall the script and target-directory environment variable used to rebuild and reinstall the running simard daemon binary after a main-branch merge, instead of guessing at a cargo install command.",
        class: BenchmarkClass::KnowledgeRecall,
        identity: "simard-gym",
        base_type: "rusty-clawd",
        topology: RuntimeTopology::SingleProcess,
        objective: "Recall the script and target-directory environment variable used to rebuild and reinstall the running simard daemon binary after a main-branch merge.",
        expected_min_runtime_evidence: 2,
    },
];
