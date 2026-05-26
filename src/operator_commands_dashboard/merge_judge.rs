//! Merge-Judge Decisions panel endpoint (#2041).
//!
//! Surfaces past merge-judge decisions — which PRs were evaluated, the
//! verdict (merge / reject / defer), the reasoning summary, and when the
//! evaluation happened — so an operator can see the full decision history
//! without digging through logs.
//!
//! ## Verdict persistence
//!
//! The merge-judge does not yet persist its `JudgeOutcome` to a store the
//! dashboard can read (tracked in blocking issue #1893). Until that lands,
//! this endpoint returns an empty `decisions` array with `persistence_available:
//! false` and a human-readable explanation. The panel renders a clear
//! empty-state message rather than hiding entirely.
//!
//! The endpoint always returns HTTP 200 so the frontend panel is never
//! broken by a missing feature — it gracefully degrades to "no decisions
//! recorded yet".
//!
//! ## JSON contract
//!
//! ```json
//! {
//!   "decisions": [],
//!   "persistence_available": false,
//!   "persistence_reason": "Merge-judge verdicts are not yet saved to disk. …",
//!   "summary": { "total": 0, "approved": 0, "rejected": 0, "deferred": 0 },
//!   "timestamp": "2026-05-25T10:00:00Z"
//! }
//! ```
//!
//! Once persistence lands the `decisions` array will contain objects:
//!
//! ```json
//! {
//!   "pr_number": 2001,
//!   "verdict": "ready",
//!   "rationale": "All merge-ready criteria are met.",
//!   "blockers": [],
//!   "evaluated_at": "2026-05-25T09:42:00Z"
//! }
//! ```

use axum::Json;
use serde_json::{Value, json};

/// `GET /api/merge-judge` handler.
///
/// Returns the merge-judge decision history. Currently always returns an
/// empty array because verdict persistence has not yet landed (#1893).
/// The panel renders a clear empty-state message instead of hiding.
pub(crate) async fn merge_judge_decisions() -> Json<Value> {
    // Once #1893 lands, this will read persisted verdicts from the state
    // root (e.g. `<state_root>/merge_judge_verdicts.json` or the
    // cognitive-memory store). Until then, return the stub.
    let decisions: Vec<Value> = Vec::new();

    Json(json!({
        "decisions": decisions,
        "persistence_available": false,
        "persistence_reason": "Merge-judge verdicts are not yet saved to disk. \
            Each merge evaluation currently runs in-memory and the result is \
            discarded after the merge decision completes. A future update will \
            persist every verdict so this panel populates automatically. \
            Tracked in issue #1893.",
        "summary": {
            "total": 0,
            "approved": 0,
            "rejected": 0,
            "deferred": 0,
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

// ─────────────────────────── Tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn merge_judge_decisions_returns_200_with_empty_decisions() {
        let Json(body) = merge_judge_decisions().await;
        assert!(body["decisions"].is_array());
        assert_eq!(body["decisions"].as_array().unwrap().len(), 0);
        assert_eq!(body["persistence_available"], false);
        assert!(
            body["persistence_reason"]
                .as_str()
                .unwrap()
                .contains("#1893")
        );
        assert_eq!(body["summary"]["total"], 0);
        assert_eq!(body["summary"]["approved"], 0);
        assert_eq!(body["summary"]["rejected"], 0);
        assert_eq!(body["summary"]["deferred"], 0);
        assert!(body["timestamp"].is_string());
    }

    #[tokio::test]
    async fn merge_judge_response_is_deterministic_shape() {
        let Json(a) = merge_judge_decisions().await;
        let Json(b) = merge_judge_decisions().await;
        // Shape is identical (timestamps differ but all keys present).
        let keys_a: Vec<&str> = a.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        let keys_b: Vec<&str> = b.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        assert_eq!(keys_a, keys_b);
    }
}
