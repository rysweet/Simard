//! Auto-split BENCHMARK_SCENARIOS data — chunk 6 of 6.
//!
//! KnowledgeRecall family (issue #1459): scenarios that measure longitudinal
//! learning. Each scenario asks the agent to recall something she should
//! already know — about her own code, her tools, the repos she maintains, or
//! the user's stated preferences — and grounds the answer in stored memories
//! or real repository file paths rather than confabulation.

use super::super::types::{BenchmarkClass, BenchmarkScenario};
use crate::runtime::RuntimeTopology;

pub(super) static SCENARIOS: [BenchmarkScenario; 2] = [
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
];
