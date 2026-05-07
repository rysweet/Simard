//! class_specific_checks helpers — chunk 10.
//!
//! SelfIntrospection family: scenarios that ask the agent to recall and
//! reason about her own past cycle-report judgments. Each scenario emits the
//! same two checks as the rest of the recall families:
//! `self-introspection-evidence-grounded` (runtime evidence references at
//! least one stored memory record, repo file path, or cycle-report path) and
//! `self-introspection-canonical-token-cited` (the response actually names
//! the canonical tokens — goal ids, phase names, sha256 prompt_version
//! prefixes, or hot-reload concepts — that the objective asked about).
//!
//! Split into its own module to respect the 400-LOC per-module cap (#1266).

use super::super::types::{BenchmarkCheckResult, BenchmarkScenario};
use crate::handoff::RuntimeHandoffSnapshot;

/// Checks for the `SelfIntrospection` family scenarios.
pub(super) fn checks_for_self_introspection(
    scenario: &BenchmarkScenario,
    combined: &str,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let memory_grounded = !exported.memory_records.is_empty();
    let path_cited = combined.contains(".rs")
        || combined.contains("src/")
        || combined.contains("cycle_reports")
        || combined.contains("prompt_assets");
    let evidence_grounded = memory_grounded || path_cited;

    let topic_match = match scenario.id {
        "self-introspection-l1-direct-cycle-recall" => {
            let goal_named = combined.contains("improve-cognitive-memory-persistence");
            let phase_named = combined.contains("act");
            let decision_named = combined.contains("dispatch_engineer");
            let cycle_named = combined.contains("cycle 5") || combined.contains("cycle_5");
            goal_named && phase_named && decision_named && cycle_named
        }
        "self-introspection-l2-multi-cycle-synthesis" => {
            let goal_named = combined.contains("add-more-gym-benchmark-scenarios");
            let on_track_named = combined.contains("on-track") || combined.contains("on track");
            let blocked_named = combined.contains("blocked-on-clippy")
                || (combined.contains("blocked") && combined.contains("clippy"));
            let change_named = combined.contains("changed")
                || combined.contains("differ")
                || combined.contains("different");
            goal_named && on_track_named && blocked_named && change_named
        }
        "self-introspection-l3-brain-vs-fallback" => {
            let llm_named = combined.contains("llm") || combined.contains("brain");
            let fallback_named =
                combined.contains("fallback") || combined.contains("deterministic");
            let prompt_version_named = combined.contains("prompt_version")
                || combined.contains("prompt version")
                || combined.contains("sha256");
            let phases_named = (combined.contains("orient") && combined.contains("decide"))
                && (combined.contains("observe") && combined.contains("act"));
            llm_named && fallback_named && prompt_version_named && phases_named
        }
        "self-introspection-l4-prompt-hot-reload" => {
            let hot_reload_named = combined.contains("hot-reload")
                || combined.contains("hot reload")
                || combined.contains("hotreload");
            let version_change_named =
                combined.contains("aaa111000222") && combined.contains("bbb222000333");
            let mechanism_named = combined.contains("prompt_assets")
                || combined.contains("re-hash")
                || combined.contains("rehash")
                || combined.contains("without") && combined.contains("restart");
            hot_reload_named && version_change_named && mechanism_named
        }
        "self-introspection-l5-rationale-paraphrase" => {
            let goal_named = combined.contains("add-more-gym-benchmark-scenarios")
                || combined.contains("self-introspection")
                || combined.contains("selfintrospection");
            let coverage_named = combined.contains("coverage")
                || combined.contains("gap")
                || combined.contains("lack");
            let longitudinal_named = combined.contains("longitudinal")
                || combined.contains("self-knowledge")
                || combined.contains("self knowledge")
                || combined.contains("introspect");
            let bounded_named = combined.contains("engineer")
                || combined.contains("worktree")
                || combined.contains("500 loc")
                || combined.contains("under 500");
            goal_named && coverage_named && longitudinal_named && bounded_named
        }
        "self-introspection-l6-goal-status-recall" => {
            let goal_named = combined.contains("self-serve-dashboard-improvement")
                || combined.contains("dashboard");
            let status_named = combined.contains("in-progress") || combined.contains("in progress");
            let cycle_named = combined.contains("cycle 9") || combined.contains("cycle_9");
            let deterministic_named =
                combined.contains("deterministic") || combined.contains("fallback");
            goal_named && status_named && cycle_named && deterministic_named
        }
        "self-introspection-l7-rationale-comparison" => {
            let skip_named = combined.contains("skip");
            let dispatch_named =
                combined.contains("dispatch_engineer") || combined.contains("dispatch engineer");
            let blocked_named = combined.contains("blocked")
                || combined.contains("clippy")
                || combined.contains("open worktree");
            let dashboard_named = combined.contains("dashboard") || combined.contains("playwright");
            skip_named && dispatch_named && blocked_named && dashboard_named
        }
        "self-introspection-l8-observe-phase-discrimination" => {
            let observe_named = combined.contains("observe");
            let deterministic_named =
                combined.contains("deterministic") || combined.contains("design");
            let pr_cited = combined.contains("#1458")
                || combined.contains("#1469")
                || combined.contains("#1471")
                || combined.contains("prompt-driven");
            let empty_version_named = combined.contains("empty")
                || combined.contains("no llm")
                || combined.contains("not llm");
            observe_named && deterministic_named && pr_cited && empty_version_named
        }
        _ => false,
    };

    vec![
        BenchmarkCheckResult {
            id: "self-introspection-evidence-grounded".to_string(),
            passed: evidence_grounded,
            detail: format!(
                "runtime evidence {} a stored memory record, repo file path, or cycle-report path",
                if evidence_grounded {
                    "references"
                } else {
                    "lacks"
                }
            ),
        },
        BenchmarkCheckResult {
            id: "self-introspection-canonical-token-cited".to_string(),
            passed: topic_match,
            detail: format!(
                "execution output {} the canonical introspection tokens named by the objective",
                if topic_match { "names" } else { "omits" }
            ),
        },
    ]
}
