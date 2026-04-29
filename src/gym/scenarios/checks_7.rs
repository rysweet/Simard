//! class_specific_checks helpers — chunk 7.
//!
//! KnowledgeRecall **repo-knowledge** sub-family (issue #1459): scenarios
//! that ask the agent to recall structural facts about Simard's own
//! repository layout — OODA loop module structure, the cognitive memory
//! storage backend, and the engineer-subagent worktree pattern. Split out
//! of `checks_6.rs` to keep that module under the 400-LOC cap (#1266).

use super::super::types::{BenchmarkCheckResult, BenchmarkScenario};
use crate::handoff::RuntimeHandoffSnapshot;

/// Checks for the `KnowledgeRecall` repo-knowledge scenarios.
///
/// Each scenario produces the same two checks as the rest of the family:
/// `knowledge-recall-evidence-grounded` (runtime evidence references at
/// least one stored memory record or repo file path) and
/// `knowledge-recall-topic-cited` (the response actually names the canonical
/// tokens the objective asked about).
pub(super) fn checks_for_knowledge_recall_repo(
    scenario: &BenchmarkScenario,
    combined: &str,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let memory_grounded = !exported.memory_records.is_empty();
    let path_cited =
        combined.contains(".rs") || combined.contains("src/") || combined.contains("docs/");
    let evidence_grounded = memory_grounded || path_cited;

    let topic_match = match scenario.id {
        "knowledge-recall-repo-ooda-loop-layout" => {
            let phase_named = combined.contains("observe")
                || combined.contains("orient")
                || combined.contains("decide")
                || combined.contains("act");
            let layout_named = combined.contains("src/ooda_loop/")
                || combined.contains("cycle.rs")
                || combined.contains("mod.rs");
            phase_named && layout_named
        }
        "knowledge-recall-repo-cognitive-memory-store" => {
            let backend_named =
                combined.contains("ladybug") || combined.contains("cognitive_memory.ladybug");
            let path_named = combined.contains("~/.simard/")
                || combined.contains(".simard/cognitive_memory")
                || combined.contains("cognitive_memory.ladybug");
            backend_named && path_named
        }
        "knowledge-recall-repo-engineer-worktree-pattern" => {
            let dir_named = combined.contains("engineer-worktrees")
                || combined.contains("~/.simard/engineer-worktrees/");
            let convention_named = combined.contains("engineer-<goal-id>-<timestamp>")
                || combined.contains("engineer-<goal-id>")
                || combined.contains("goal-id")
                || combined.contains("goal id");
            dir_named && convention_named
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
                "execution output {} the recall topic named by the objective",
                if topic_match { "names" } else { "omits" }
            ),
        },
    ]
}
