//! class_specific_checks helpers — chunk 9.
//!
//! ErrorHandlingDebug sub-family (issue #1461): each scenario asks the agent
//! to diagnose a real Simard runtime error and propose the documented
//! remediation. The two checks per scenario verify that runtime evidence
//! references at least one concrete signal (a file path, a stored memory
//! record, or a canonical token) and that the response actually names the
//! canonical tokens the objective asked about.
//!
//! Split into its own module so `checks_4::checks_for_error_handling`
//! retains its generic catch-all behavior and to respect the 400-LOC
//! per-module cap (#1266).

use super::super::types::{BenchmarkCheckResult, BenchmarkScenario};
use crate::handoff::RuntimeHandoffSnapshot;

/// Checks for the `ErrorHandling` debug sub-family scenarios.
pub(super) fn checks_for_error_handling_debug(
    scenario: &BenchmarkScenario,
    combined: &str,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let memory_grounded = !exported.memory_records.is_empty();
    let path_cited = combined.contains(".rs")
        || combined.contains("src/")
        || combined.contains("docs/")
        || combined.contains(".yml")
        || combined.contains(".yaml")
        || combined.contains("~/.simard/");
    let evidence_grounded = memory_grounded || path_cited;

    let topic_match = match scenario.id {
        "error-handling-debug-stale-engineer-worktree" => {
            let dispatch_check_named = combined.contains("find_live_engineer_for_goal");
            let worktree_path_named = combined.contains("engineer-worktrees");
            let liveness_named = combined.contains("sentinel")
                || combined.contains("alive")
                || combined.contains("liveness")
                || combined.contains("exited");
            dispatch_check_named && worktree_path_named && liveness_named
        }
        "error-handling-debug-pre-push-clippy-failure" => {
            let lint_named = combined.contains("unused_imports");
            let tool_named = combined.contains("clippy");
            let bypass_forbidden = combined.contains("--no-verify")
                && (combined.contains("forbid")
                    || combined.contains("prohibit")
                    || combined.contains("not allowed")
                    || combined.contains("never"));
            lint_named && tool_named && bypass_forbidden
        }
        "error-handling-debug-mkdocs-strict-broken-link" => {
            let config_named = combined.contains("mkdocs.yml");
            let strict_named = combined.contains("strict");
            let docs_tree_named = combined.contains("docs/");
            let outside_tree_named = combined.contains("prompt_assets");
            config_named && strict_named && docs_tree_named && outside_tree_named
        }
        "error-handling-debug-recipe-runner-hollow-success" => {
            let guard_named = combined.contains("step-08c");
            let symptom_named = combined.contains("hollow") && combined.contains("worktree");
            let fallback_named = (combined.contains("sub-agent") || combined.contains("subagent"))
                && (combined.contains("opus") || combined.contains("4.7"));
            guard_named && symptom_named && fallback_named
        }
        _ => false,
    };

    vec![
        BenchmarkCheckResult {
            id: "error-handling-debug-evidence-grounded".to_string(),
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
            id: "error-handling-debug-canonical-token-cited".to_string(),
            passed: topic_match,
            detail: format!(
                "execution output {} the canonical diagnosis tokens named by the objective",
                if topic_match { "names" } else { "omits" }
            ),
        },
    ]
}
