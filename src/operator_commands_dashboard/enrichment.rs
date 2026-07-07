//! Dashboard endpoint `GET /api/enrichment` (issue #2942).
//!
//! Surfaces on the Memory tab whether recall is reaching decisions: the
//! attach-rate and the average facts/procedures/preamble-bytes injected per
//! decision, read from the **live** store — the `enrichment` section of
//! `<state_root>/telemetry/metrics_snapshot.json`, the same `resolve_state_root()`
//! read-through every other dashboard tab uses. It is a **total function**: it
//! always returns HTTP `200` with a degrade-safe body, never `4xx`/`5xx` for bad
//! input, and never leaks the on-disk state-root path.

use std::path::Path;

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::routes::resolve_state_root;

/// Snapshot freshness threshold (seconds). Matches the status provider's
/// `SNAPSHOT_FRESHNESS_SECS` so `live`/`stale` mean the same thing everywhere.
const FRESHNESS_SECS: i64 = 300;

/// Trailing window in hours for the attach-rate/averages (default 24, clamped
/// `1..=8760`). Out-of-range values are clamped, never rejected.
pub(crate) fn clamp_window_hours(v: Option<u64>) -> u64 {
    v.unwrap_or(24).clamp(1, 8760)
}

/// Max `metrics.jsonl` records to scan within the window (default 500, clamped
/// `1..=1000`).
pub(crate) fn clamp_limit(v: Option<u64>) -> u64 {
    v.unwrap_or(500).clamp(1, 1000)
}

/// Classify a snapshot `captured_at` timestamp into `(freshness, age_seconds)`.
///
/// An unparseable timestamp is treated as `live` with an unknown age, matching
/// the tolerant behaviour of the status provider (never falsely `stale`).
fn classify_freshness(captured_at: &str) -> (&'static str, Option<i64>) {
    match chrono::DateTime::parse_from_rfc3339(captured_at) {
        Ok(ts) => {
            let age = chrono::Utc::now()
                .signed_duration_since(ts.with_timezone(&chrono::Utc))
                .num_seconds();
            let label = if age > FRESHNESS_SECS {
                "stale"
            } else {
                "live"
            };
            (label, Some(age.max(0)))
        }
        Err(_) => ("live", None),
    }
}

/// Compute the `GET /api/enrichment` body from the metrics snapshot under
/// `state_root`. Degrades safely: a missing/corrupt snapshot or absent enrichment
/// section returns `available:false` with `null` magnitudes and HTTP `200`.
pub(crate) fn enrichment_core(
    state_root: &Path,
    window_hours: Option<u64>,
    limit: Option<u64>,
) -> (StatusCode, Json<Value>) {
    let window_hours = clamp_window_hours(window_hours);
    // `limit` bounds a future per-record scan; clamp it now so the echoed
    // contract is stable even though the current rollup read is O(1).
    let _limit = clamp_limit(limit);

    // Degrade-safe default body: nothing available, no false 0%.
    let mut body = json!({
        "available": false,
        "freshness": "missing",
        "snapshot_age_seconds": Value::Null,
        "window_hours": window_hours,
        "decisions": Value::Null,
        "attached": Value::Null,
        "attach_rate": Value::Null,
        "degraded": Value::Null,
        "avg_facts_injected": Value::Null,
        "avg_procedures_injected": Value::Null,
        "avg_preamble_bytes": Value::Null,
        "last": Value::Null,
    });

    let path = crate::telemetry::snapshot::snapshot_path(state_root);
    let Some(snapshot) = crate::telemetry::snapshot::read(&path) else {
        // No snapshot yet (fresh brain / daemon not running).
        return (StatusCode::OK, Json(body));
    };

    let (freshness, age) = classify_freshness(&snapshot.captured_at);
    body["freshness"] = json!(freshness);
    body["snapshot_age_seconds"] = age.map(|a| json!(a)).unwrap_or(Value::Null);

    // The snapshot exists but has no enrichment section yet: report unavailable
    // (so the panel shows "Not tracked yet") rather than a false 0%.
    let Some(section) = snapshot.enrichment.as_ref().filter(|v| v.is_object()) else {
        return (StatusCode::OK, Json(body));
    };

    body["available"] = json!(true);
    let field = |key: &str| section.get(key).cloned().unwrap_or(Value::Null);
    body["decisions"] = field("decisions");
    body["attached"] = field("attached");
    body["attach_rate"] = field("attach_rate");
    body["degraded"] = field("degraded");
    body["avg_facts_injected"] = field("avg_facts_injected");
    body["avg_procedures_injected"] = field("avg_procedures_injected");
    body["avg_preamble_bytes"] = field("avg_preamble_bytes");
    body["last"] = field("last");

    (StatusCode::OK, Json(body))
}

/// `GET /api/enrichment` — the enrichment attach-rate/averages endpoint.
///
/// Query params (`window_hours`, `limit`) are parsed leniently and clamped, never
/// rejected: an unparseable value falls back to the default bound.
pub(crate) async fn enrichment(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let parse = |key: &str| params.get(key).and_then(|v| v.parse::<u64>().ok());
    enrichment_core(&resolve_state_root(), parse("window_hours"), parse("limit"))
}
