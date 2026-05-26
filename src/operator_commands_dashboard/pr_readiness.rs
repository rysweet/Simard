//! Per-PR Readiness panel endpoint (#2042).
//!
//! Surfaces every open PR under Simard's management with its CI status,
//! review state, and remaining blockers so an operator can assess readiness
//! without switching to GitHub.
//!
//! Data sources:
//!   - `gh pr list` for live PR metadata (title, CI checks, review state,
//!     mergeable status, labels).
//!
//! ## JSON contract
//!
//! ```json
//! {
//!   "prs": [
//!     {
//!       "number": 2001,
//!       "title": "feat: add PR readiness panel",
//!       "url": "https://github.com/rysweet/Simard/pull/2001",
//!       "author": "simard-bot",
//!       "head_branch": "feat/pr-readiness",
//!       "base_branch": "main",
//!       "ci_status": "passing",
//!       "review_status": "approved",
//!       "mergeable": "MERGEABLE",
//!       "labels": ["enhancement"],
//!       "blockers": [],
//!       "ready": true,
//!       "created_at": "2026-05-24T10:00:00Z",
//!       "updated_at": "2026-05-25T09:42:00Z"
//!     }
//!   ],
//!   "summary": {
//!     "total": 10,
//!     "ready": 3,
//!     "blocked": 5,
//!     "pending": 2
//!   },
//!   "timestamp": "2026-05-25T10:00:00Z"
//! }
//! ```

use axum::Json;
use serde_json::{Value, json};

/// Repo the panel targets.
const HOME_REPO: &str = "rysweet/Simard";

/// Maximum PRs to fetch.
const PR_LIMIT: u32 = 50;

/// Classify CI check status from the statusCheckRollup entries.
fn classify_ci_status(checks: &[Value]) -> &'static str {
    if checks.is_empty() {
        return "none";
    }
    let mut has_pending = false;
    for check in checks {
        let state = check
            .get("state")
            .or_else(|| check.get("conclusion"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match state {
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" => return "failing",
            "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "" => has_pending = true,
            _ => {} // SUCCESS, NEUTRAL, SKIPPED — all fine
        }
    }
    if has_pending { "pending" } else { "passing" }
}

/// Classify review status from reviewDecision.
fn classify_review_status(review_decision: &str) -> &'static str {
    match review_decision {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "changes_requested",
        "REVIEW_REQUIRED" => "review_required",
        "" => "none",
        _ => "unknown",
    }
}

/// Determine blockers for a PR.
fn determine_blockers(
    ci_status: &str,
    review_status: &str,
    mergeable: &str,
    is_draft: bool,
) -> Vec<String> {
    let mut blockers = Vec::new();
    if is_draft {
        blockers.push("PR is a draft".to_string());
    }
    if ci_status == "failing" {
        blockers.push("CI checks are failing".to_string());
    }
    if ci_status == "pending" {
        blockers.push("CI checks still running".to_string());
    }
    if review_status == "changes_requested" {
        blockers.push("Changes requested by reviewer".to_string());
    }
    if review_status == "review_required" {
        blockers.push("Review required".to_string());
    }
    if mergeable == "CONFLICTING" {
        blockers.push("Merge conflicts".to_string());
    }
    if mergeable == "UNKNOWN" {
        blockers.push("Mergeability unknown".to_string());
    }
    blockers
}

/// Build the response JSON from raw PR data. Separated from the handler
/// for unit testability.
pub fn build_pr_readiness_response(raw_prs: &[Value]) -> Value {
    let mut prs = Vec::with_capacity(raw_prs.len());
    let mut total_ready = 0u32;
    let mut total_blocked = 0u32;
    let mut total_pending = 0u32;

    for pr in raw_prs {
        let number = pr.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
        let title = pr
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let url = pr
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let author = pr
            .get("author")
            .and_then(|v| v.get("login"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let head_branch = pr
            .get("headRefName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let base_branch = pr
            .get("baseRefName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mergeable = pr
            .get("mergeable")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();
        let is_draft = pr.get("isDraft").and_then(|v| v.as_bool()).unwrap_or(false);
        let review_decision = pr
            .get("reviewDecision")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let created_at = pr
            .get("createdAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let updated_at = pr
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let labels: Vec<String> = pr
            .get("labels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let checks: Vec<Value> = pr
            .get("statusCheckRollup")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let ci_status = classify_ci_status(&checks);
        let review_status = classify_review_status(review_decision);
        let blockers = determine_blockers(ci_status, review_status, &mergeable, is_draft);
        let ready = blockers.is_empty();

        if ready {
            total_ready += 1;
        } else if ci_status == "pending" {
            total_pending += 1;
        } else {
            total_blocked += 1;
        }

        prs.push(json!({
            "number": number,
            "title": title,
            "url": url,
            "author": author,
            "head_branch": head_branch,
            "base_branch": base_branch,
            "ci_status": ci_status,
            "review_status": review_status,
            "mergeable": mergeable,
            "is_draft": is_draft,
            "labels": labels,
            "blockers": blockers,
            "ready": ready,
            "created_at": created_at,
            "updated_at": updated_at,
        }));
    }

    json!({
        "prs": prs,
        "summary": {
            "total": raw_prs.len(),
            "ready": total_ready,
            "blocked": total_blocked,
            "pending": total_pending,
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

/// `GET /api/prs` handler.
///
/// Queries `gh pr list` for all open PRs with CI, review, and merge status,
/// then returns a structured JSON response for the dashboard panel.
pub(crate) async fn pr_readiness() -> Json<Value> {
    let list_result = tokio::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            HOME_REPO,
            "--state",
            "open",
            "--limit",
            &PR_LIMIT.to_string(),
            "--json",
            "number,title,url,author,headRefName,baseRefName,mergeable,isDraft,\
             reviewDecision,labels,statusCheckRollup,createdAt,updatedAt",
        ])
        .output()
        .await;

    match list_result {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout);
            match serde_json::from_str::<Vec<Value>>(&raw) {
                Ok(prs) => Json(build_pr_readiness_response(&prs)),
                Err(e) => Json(json!({
                    "prs": [],
                    "summary": { "total": 0, "ready": 0, "blocked": 0, "pending": 0 },
                    "error": format!("failed to parse gh output: {e}"),
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })),
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Json(json!({
                "prs": [],
                "summary": { "total": 0, "ready": 0, "blocked": 0, "pending": 0 },
                "error": format!("gh pr list failed: {}", stderr.chars().take(300).collect::<String>()),
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }))
        }
        Err(e) => Json(json!({
            "prs": [],
            "summary": { "total": 0, "ready": 0, "blocked": 0, "pending": 0 },
            "error": format!("failed to run gh: {e}"),
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })),
    }
}

// ─────────────────────────── Tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pr(number: u64, ci_state: &str, review: &str, mergeable: &str) -> Value {
        json!({
            "number": number,
            "title": format!("test PR {number}"),
            "url": format!("https://github.com/rysweet/Simard/pull/{number}"),
            "author": { "login": "test-user" },
            "headRefName": format!("feat/test-{number}"),
            "baseRefName": "main",
            "mergeable": mergeable,
            "isDraft": false,
            "reviewDecision": review,
            "labels": [{ "name": "enhancement" }],
            "statusCheckRollup": [{ "state": ci_state }],
            "createdAt": "2026-05-24T10:00:00Z",
            "updatedAt": "2026-05-25T09:00:00Z",
        })
    }

    #[test]
    fn ready_pr_has_no_blockers() {
        let prs = vec![make_pr(1, "SUCCESS", "APPROVED", "MERGEABLE")];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["prs"][0]["ready"], true);
        assert!(resp["prs"][0]["blockers"].as_array().unwrap().is_empty());
        assert_eq!(resp["summary"]["ready"], 1);
        assert_eq!(resp["summary"]["blocked"], 0);
    }

    #[test]
    fn failing_ci_is_blocker() {
        let prs = vec![make_pr(2, "FAILURE", "APPROVED", "MERGEABLE")];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["prs"][0]["ready"], false);
        assert_eq!(resp["prs"][0]["ci_status"], "failing");
        let blockers: Vec<&str> = resp["prs"][0]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect();
        assert!(blockers.iter().any(|b| b.contains("CI")));
        assert_eq!(resp["summary"]["blocked"], 1);
    }

    #[test]
    fn pending_ci_is_pending_not_blocked() {
        let prs = vec![make_pr(3, "PENDING", "APPROVED", "MERGEABLE")];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["prs"][0]["ready"], false);
        assert_eq!(resp["prs"][0]["ci_status"], "pending");
        assert_eq!(resp["summary"]["pending"], 1);
        assert_eq!(resp["summary"]["blocked"], 0);
    }

    #[test]
    fn changes_requested_is_blocker() {
        let prs = vec![make_pr(4, "SUCCESS", "CHANGES_REQUESTED", "MERGEABLE")];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["prs"][0]["ready"], false);
        let blockers: Vec<&str> = resp["prs"][0]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect();
        assert!(blockers.iter().any(|b| b.contains("Changes requested")));
    }

    #[test]
    fn merge_conflict_is_blocker() {
        let prs = vec![make_pr(5, "SUCCESS", "APPROVED", "CONFLICTING")];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["prs"][0]["ready"], false);
        let blockers: Vec<&str> = resp["prs"][0]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect();
        assert!(blockers.iter().any(|b| b.contains("conflict")));
    }

    #[test]
    fn draft_pr_is_blocker() {
        let prs = vec![json!({
            "number": 6,
            "title": "draft PR",
            "url": "https://github.com/rysweet/Simard/pull/6",
            "author": { "login": "test-user" },
            "headRefName": "feat/draft",
            "baseRefName": "main",
            "mergeable": "MERGEABLE",
            "isDraft": true,
            "reviewDecision": "APPROVED",
            "labels": [],
            "statusCheckRollup": [{ "state": "SUCCESS" }],
            "createdAt": "2026-05-24T10:00:00Z",
            "updatedAt": "2026-05-25T09:00:00Z",
        })];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["prs"][0]["ready"], false);
        let blockers: Vec<&str> = resp["prs"][0]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect();
        assert!(blockers.iter().any(|b| b.contains("draft")));
    }

    #[test]
    fn summary_aggregates_mixed_states() {
        let prs = vec![
            make_pr(1, "SUCCESS", "APPROVED", "MERGEABLE"),
            make_pr(2, "SUCCESS", "APPROVED", "MERGEABLE"),
            make_pr(3, "FAILURE", "APPROVED", "MERGEABLE"),
            make_pr(4, "PENDING", "APPROVED", "MERGEABLE"),
            make_pr(5, "SUCCESS", "CHANGES_REQUESTED", "MERGEABLE"),
        ];
        let resp = build_pr_readiness_response(&prs);
        assert_eq!(resp["summary"]["total"], 5);
        assert_eq!(resp["summary"]["ready"], 2);
        assert_eq!(resp["summary"]["blocked"], 2);
        assert_eq!(resp["summary"]["pending"], 1);
    }

    #[test]
    fn empty_prs_returns_empty_response() {
        let resp = build_pr_readiness_response(&[]);
        assert_eq!(resp["summary"]["total"], 0);
        assert!(resp["prs"].as_array().unwrap().is_empty());
        assert!(resp["timestamp"].is_string());
    }

    #[test]
    fn classify_ci_status_no_checks() {
        assert_eq!(classify_ci_status(&[]), "none");
    }

    #[test]
    fn classify_ci_status_all_passing() {
        let checks = vec![json!({"state": "SUCCESS"}), json!({"state": "NEUTRAL"})];
        assert_eq!(classify_ci_status(&checks), "passing");
    }

    #[test]
    fn classify_ci_status_one_failure() {
        let checks = vec![json!({"state": "SUCCESS"}), json!({"state": "FAILURE"})];
        assert_eq!(classify_ci_status(&checks), "failing");
    }

    #[test]
    fn classify_ci_status_pending_without_failure() {
        let checks = vec![json!({"state": "SUCCESS"}), json!({"state": "PENDING"})];
        assert_eq!(classify_ci_status(&checks), "pending");
    }

    #[test]
    fn classify_review_approved() {
        assert_eq!(classify_review_status("APPROVED"), "approved");
    }

    #[test]
    fn classify_review_changes_requested() {
        assert_eq!(
            classify_review_status("CHANGES_REQUESTED"),
            "changes_requested"
        );
    }

    #[test]
    fn classify_review_empty() {
        assert_eq!(classify_review_status(""), "none");
    }
}
