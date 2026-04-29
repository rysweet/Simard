//! class_specific_checks helpers — chunk 8.
//!
//! KnowledgeRecall **cross-session** sub-family (issue #1459, capstone PR):
//! scenarios that ask the agent to recall something stored in a **prior**
//! gym session. These directly stress-test cognitive memory persistence
//! across session boundaries — the same subsystem the still-wedged
//! `improve-cognitive-memory-persistence` daemon goal is trying to fix.
//!
//! Split into its own module so `checks_7.rs` keeps its narrower
//! repo-knowledge focus and to respect the 400-LOC per-module cap (#1266).

use super::super::types::{BenchmarkCheckResult, BenchmarkScenario};
use crate::handoff::RuntimeHandoffSnapshot;

/// Checks for the `KnowledgeRecall` cross-session scenarios.
///
/// Each scenario produces the same two checks as the rest of the family:
/// `knowledge-recall-evidence-grounded` (runtime evidence references at
/// least one stored memory record or repo file path) and
/// `knowledge-recall-topic-cited` (the response actually names the
/// canonical tokens the objective asked about — including tokens that
/// only make sense if the agent read accumulated cross-session memory,
/// not just the current session's prompt context).
pub(super) fn checks_for_knowledge_recall_cross_session(
    scenario: &BenchmarkScenario,
    combined: &str,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let memory_grounded = !exported.memory_records.is_empty();
    let path_cited =
        combined.contains(".rs") || combined.contains("src/") || combined.contains("docs/");
    let evidence_grounded = memory_grounded || path_cited;

    let topic_match = match scenario.id {
        "knowledge-recall-cross-session-fact" => {
            let canary_named = combined.contains("gym-cross-session-canary");
            let prior_session_named =
                combined.contains("prior session") || combined.contains("previous session");
            let memory_layer_named = combined.contains("cognitive memory")
                || combined.contains("cognitive_memory")
                || combined.contains("accumulated");
            canary_named && prior_session_named && memory_layer_named
        }
        "knowledge-recall-cross-session-preference" => {
            let stance_named =
                combined.contains("prompt-driven") || combined.contains("prompt driven");
            let pattern_named = combined.contains("prompt_assets/simard")
                || combined.contains("include_str!")
                || combined.contains("oodabrain");
            let date_named = combined.contains("apr 29")
                || combined.contains("2026-04-29")
                || combined.contains("april 29");
            stance_named && pattern_named && date_named
        }
        _ => false,
    };

    vec![
        BenchmarkCheckResult {
            id: "knowledge-recall-evidence-grounded".to_string(),
            passed: evidence_grounded,
            detail: format!(
                "runtime evidence {} a stored memory record or repo file path",
                if evidence_grounded {
                    "references"
                } else {
                    "lacks"
                }
            ),
        },
        BenchmarkCheckResult {
            id: "knowledge-recall-topic-cited".to_string(),
            passed: topic_match,
            detail: format!(
                "execution output {} the cross-session recall topic named by the objective",
                if topic_match { "names" } else { "omits" }
            ),
        },
    ]
}
