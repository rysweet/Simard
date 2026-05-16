//! Merge Readiness panel endpoint (#1880).
//!
//! Surfaces:
//!
//! - The active merge-judge implementation (`llm` vs `refusing`) so the
//!   operator can spot the silent fall-back to [`RefusingMergeJudge`] that
//!   motivated #1870 → #1880.
//! - Per-PR objective-gate verdict (`baseRefName` allow-list, `mergeable`,
//!   `statusCheckRollup`). This is the **cheap deterministic** half of the
//!   gate from [`crate::stewardship::merge_authority`]; the expensive agentic
//!   judge is **not** invoked per refresh.
//! - Per-PR `last_judge_verdict` placeholder — currently always `null`
//!   because the judge does not yet persist its `JudgeOutcome` anywhere the
//!   dashboard can read. Tracked in the follow-up issue filed alongside this
//!   panel ("stewardship: persist merge-judge verdicts for dashboard
//!   consumption (blocks #1880)").
//!
//! The handler returns the JSON contract documented in #1880:
//!
//! ```json
//! {
//!   "judge_configured": true,
//!   "judge_kind": "llm",
//!   "base_allowlist": ["main"],
//!   "open_prs": [
//!     {
//!       "number": 1870,
//!       "title": "...",
//!       "head_ref_name": "feat/...",
//!       "base_ref_name": "main",
//!       "url": "https://...",
//!       "objective_gates_pass": true,
//!       "objective_blocker": null,
//!       "readiness_state": "ready",
//!       "last_judge_verdict": null
//!     }
//!   ],
//!   "summary": { "objective_ready": 3, "objective_blocked": 7, "total_open": 10 },
//!   "timestamp": "2026-05-16T17:30:00Z"
//! }
//! ```

use axum::Json;
use serde_json::{Value, json};

use crate::stewardship::merge_authority::{
    OpenPrSummary, PrGhClient, RealPrGhClient, base_allowlist_from_env, evaluate_objective_gates,
};
use crate::stewardship::merge_judge::{MergeJudgeKind, build_merge_judge};

/// `gh pr list` page size for the panel. Matches the limit recommended in
/// #1880; 50 covers our active repo without paginating.
pub const DASHBOARD_PR_LIMIT: u32 = 50;

/// Repo the dashboard's merge-authority panel targets. Hardcoded to
/// `rysweet/Simard` to match [`crate::operator_cli::merge::HOME_REPO`] —
/// the merge subsystem is single-repo by design (see #1880 "Out of scope").
pub const HOME_REPO: &str = "rysweet/Simard";

/// The deterministic per-PR readiness state surfaced in the panel. Drives
/// the badge colour on the dashboard:
///
/// - `Ready` (green): every objective gate passes.
/// - `NotReady` (red): at least one objective gate fails (CI red, wrong
///   base, not mergeable).
/// - `Pending` (yellow): an objective check is still in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrReadinessState {
    Ready,
    NotReady,
    Pending,
}

impl PrReadinessState {
    pub fn as_str(self) -> &'static str {
        match self {
            PrReadinessState::Ready => "ready",
            PrReadinessState::NotReady => "not_ready",
            PrReadinessState::Pending => "pending",
        }
    }
}

/// CI check states that mean "still running" — the PR isn't ready yet but
/// it isn't a hard failure either. Treated as `Pending` so the operator
/// knows to wait rather than chase a phantom blocker. Keep this in sync
/// with [`merge_authority::is_passing_state`]'s negative space.
fn is_pending_state(state: &str) -> bool {
    matches!(state, "PENDING" | "QUEUED" | "IN_PROGRESS")
}

/// Inspect a PR's checks for any still-running entries. Used to map a
/// failing `evaluate_objective_gates` result into `Pending` vs `NotReady`
/// for the dashboard badge — the underlying gate function intentionally
/// has no opinion about whether a failure is transient.
fn pr_has_pending_check(pr: &OpenPrSummary) -> bool {
    pr.checks.iter().any(|c| is_pending_state(&c.state))
}

/// Pure builder: turn an in-process judge kind + the open-PR list from
/// `gh pr list` into the JSON the panel renders. Extracted from the route
/// handler so it can be unit-tested without an HTTP layer or `gh` shell-out.
pub fn build_merge_readiness_response(
    judge_kind: MergeJudgeKind,
    open_prs: &[OpenPrSummary],
    base_allowlist: &[String],
) -> Value {
    let mut pr_values = Vec::with_capacity(open_prs.len());
    let mut objective_ready = 0u32;
    let mut objective_blocked = 0u32;
    let mut objective_pending = 0u32;

    for pr in open_prs {
        let snapshot = pr.to_snapshot();
        let gate_result = evaluate_objective_gates(&snapshot, base_allowlist);
        let (gates_pass, blocker) = match gate_result {
            Ok(()) => (true, None),
            Err(reason) => (false, Some(reason)),
        };
        let readiness = if gates_pass {
            objective_ready += 1;
            PrReadinessState::Ready
        } else if pr_has_pending_check(pr) {
            objective_pending += 1;
            PrReadinessState::Pending
        } else {
            objective_blocked += 1;
            PrReadinessState::NotReady
        };

        pr_values.push(json!({
            "number": pr.number,
            "title": pr.title,
            "head_ref_name": pr.head_ref_name,
            "base_ref_name": pr.base_ref_name,
            "mergeable": pr.mergeable,
            "url": pr.url,
            "objective_gates_pass": gates_pass,
            "objective_blocker": blocker,
            "readiness_state": readiness.as_str(),
            // last_judge_verdict is always null until verdict persistence
            // (the follow-up issue) lands. Surfaced as an explicit key
            // rather than omitted so the frontend can render "—" / "verdict
            // unavailable" without checking for missing properties.
            "last_judge_verdict": Value::Null,
        }));
    }

    json!({
        "judge_configured": judge_kind.is_configured(),
        "judge_kind": match judge_kind {
            MergeJudgeKind::Llm => "llm",
            MergeJudgeKind::Refusing => "refusing",
        },
        "base_allowlist": base_allowlist,
        "open_prs": pr_values,
        "summary": {
            "objective_ready": objective_ready,
            "objective_blocked": objective_blocked,
            "objective_pending": objective_pending,
            "total_open": open_prs.len(),
        },
        "verdict_persistence": {
            // Documents the current stub for the frontend and any
            // operator inspecting the raw JSON. Removed once the follow-up
            // persistence work lands.
            "available": false,
            "reason": "merge-judge verdicts are not yet persisted; see follow-up issue \
                       'stewardship: persist merge-judge verdicts for dashboard consumption \
                       (blocks #1880)'.",
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

/// `GET /api/merge-readiness` handler. Resolves the active judge kind via
/// [`build_merge_judge`] (the same constructor production uses), fetches
/// open PRs via [`RealPrGhClient::list_open_prs`], then delegates to
/// [`build_merge_readiness_response`].
///
/// Degrades gracefully:
///
/// - If `gh pr list` fails, returns the config panel with `open_prs: []`
///   and a top-level `gh_error` string so the panel can surface the failure
///   instead of disappearing.
/// - If no LLM is configured, `judge_configured` is `false` and the
///   `judge_kind` is `"refusing"`; the operator sees the explicit red badge.
pub(crate) async fn merge_readiness() -> Json<Value> {
    // Resolve the judge kind without invoking the judge. `build_merge_judge`
    // is cheap (no network) — it just decides which Box to return.
    let judge_kind = build_merge_judge().kind();
    let base_allowlist = base_allowlist_from_env();

    // Shell out to `gh pr list` on a blocking thread — `RealPrGhClient` uses
    // `std::process::Command` and we're inside a tokio handler.
    let gh = RealPrGhClient::new();
    let list_result =
        tokio::task::spawn_blocking(move || gh.list_open_prs(HOME_REPO, DASHBOARD_PR_LIMIT)).await;

    let (open_prs, gh_error): (Vec<OpenPrSummary>, Option<String>) = match list_result {
        Ok(Ok(prs)) => (prs, None),
        Ok(Err(e)) => (Vec::new(), Some(e.to_string())),
        Err(e) => (Vec::new(), Some(format!("join error: {e}"))),
    };

    let mut body = build_merge_readiness_response(judge_kind, &open_prs, &base_allowlist);
    if let Some(err) = gh_error {
        body.as_object_mut()
            .expect("build_merge_readiness_response returns an object")
            .insert("gh_error".to_string(), Value::String(err));
    }
    Json(body)
}

// ─────────────────────────── Tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stewardship::merge_authority::CheckRollupEntry;

    fn allowlist() -> Vec<String> {
        vec!["main".to_string()]
    }

    fn pr_ready(number: u32) -> OpenPrSummary {
        OpenPrSummary {
            number,
            title: format!("ready PR {number}"),
            head_ref_name: format!("feat/ready-{number}"),
            base_ref_name: "main".into(),
            mergeable: "MERGEABLE".into(),
            checks: vec![
                CheckRollupEntry {
                    name: "ci".into(),
                    state: "SUCCESS".into(),
                },
                CheckRollupEntry {
                    name: "lint".into(),
                    state: "NEUTRAL".into(),
                },
            ],
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
        }
    }

    fn pr_ci_failing(number: u32) -> OpenPrSummary {
        OpenPrSummary {
            number,
            title: format!("ci-failing PR {number}"),
            head_ref_name: format!("feat/broken-{number}"),
            base_ref_name: "main".into(),
            mergeable: "MERGEABLE".into(),
            checks: vec![CheckRollupEntry {
                name: "ci".into(),
                state: "FAILURE".into(),
            }],
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
        }
    }

    fn pr_pending(number: u32) -> OpenPrSummary {
        OpenPrSummary {
            number,
            title: format!("pending PR {number}"),
            head_ref_name: format!("feat/pending-{number}"),
            base_ref_name: "main".into(),
            mergeable: "MERGEABLE".into(),
            checks: vec![CheckRollupEntry {
                name: "ci".into(),
                state: "IN_PROGRESS".into(),
            }],
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
        }
    }

    fn pr_wrong_base(number: u32) -> OpenPrSummary {
        OpenPrSummary {
            number,
            title: format!("wrong-base PR {number}"),
            head_ref_name: format!("feat/wrong-{number}"),
            base_ref_name: "develop".into(),
            mergeable: "MERGEABLE".into(),
            checks: vec![CheckRollupEntry {
                name: "ci".into(),
                state: "SUCCESS".into(),
            }],
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
        }
    }

    #[test]
    fn config_panel_reflects_llm_judge() {
        let v = build_merge_readiness_response(MergeJudgeKind::Llm, &[], &allowlist());
        assert_eq!(v["judge_configured"], true);
        assert_eq!(v["judge_kind"], "llm");
        assert_eq!(v["summary"]["total_open"], 0);
        assert_eq!(v["summary"]["objective_ready"], 0);
        assert_eq!(v["verdict_persistence"]["available"], false);
    }

    #[test]
    fn config_panel_reflects_refusing_judge() {
        let v = build_merge_readiness_response(MergeJudgeKind::Refusing, &[], &allowlist());
        assert_eq!(v["judge_configured"], false);
        assert_eq!(v["judge_kind"], "refusing");
    }

    #[test]
    fn pr_row_ready_state() {
        let v =
            build_merge_readiness_response(MergeJudgeKind::Llm, &[pr_ready(1870)], &allowlist());
        let row = &v["open_prs"][0];
        assert_eq!(row["number"], 1870);
        assert_eq!(row["objective_gates_pass"], true);
        assert_eq!(row["readiness_state"], "ready");
        assert!(row["objective_blocker"].is_null());
        assert!(
            row["last_judge_verdict"].is_null(),
            "verdict persistence is a follow-up; stub must remain null"
        );
        assert_eq!(v["summary"]["objective_ready"], 1);
        assert_eq!(v["summary"]["objective_blocked"], 0);
        assert_eq!(v["summary"]["objective_pending"], 0);
    }

    #[test]
    fn pr_row_not_ready_when_ci_failing() {
        let v = build_merge_readiness_response(
            MergeJudgeKind::Llm,
            &[pr_ci_failing(1801)],
            &allowlist(),
        );
        let row = &v["open_prs"][0];
        assert_eq!(row["objective_gates_pass"], false);
        assert_eq!(row["readiness_state"], "not_ready");
        let blocker = row["objective_blocker"]
            .as_str()
            .expect("blocker must be present");
        assert!(
            blocker.contains("FAILURE") || blocker.contains("CI check"),
            "blocker mentions the failing check: {blocker}"
        );
        assert_eq!(v["summary"]["objective_blocked"], 1);
        assert_eq!(v["summary"]["objective_ready"], 0);
    }

    #[test]
    fn pr_row_pending_when_ci_in_progress() {
        let v =
            build_merge_readiness_response(MergeJudgeKind::Llm, &[pr_pending(1802)], &allowlist());
        let row = &v["open_prs"][0];
        assert_eq!(row["objective_gates_pass"], false);
        assert_eq!(
            row["readiness_state"], "pending",
            "IN_PROGRESS check must map to pending, not not_ready"
        );
        assert_eq!(v["summary"]["objective_pending"], 1);
        assert_eq!(v["summary"]["objective_blocked"], 0);
    }

    #[test]
    fn pr_row_not_ready_when_wrong_base() {
        let v = build_merge_readiness_response(
            MergeJudgeKind::Llm,
            &[pr_wrong_base(1803)],
            &allowlist(),
        );
        let row = &v["open_prs"][0];
        assert_eq!(row["readiness_state"], "not_ready");
        let blocker = row["objective_blocker"]
            .as_str()
            .expect("blocker must be present");
        assert!(
            blocker.contains("base branch") || blocker.contains("allow-list"),
            "blocker mentions the base-branch gate: {blocker}"
        );
    }

    #[test]
    fn summary_aggregates_mixed_pr_states() {
        let prs = vec![
            pr_ready(1),
            pr_ready(2),
            pr_ci_failing(3),
            pr_pending(4),
            pr_wrong_base(5),
        ];
        let v = build_merge_readiness_response(MergeJudgeKind::Llm, &prs, &allowlist());
        assert_eq!(v["summary"]["total_open"], 5);
        assert_eq!(v["summary"]["objective_ready"], 2);
        assert_eq!(v["summary"]["objective_blocked"], 2);
        assert_eq!(v["summary"]["objective_pending"], 1);
        assert_eq!(v["open_prs"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn allowlist_is_round_tripped_into_response() {
        let custom = vec!["main".to_string(), "release/v2".to_string()];
        let v = build_merge_readiness_response(MergeJudgeKind::Llm, &[], &custom);
        let echoed: Vec<String> = v["base_allowlist"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert_eq!(echoed, custom);
    }
}
