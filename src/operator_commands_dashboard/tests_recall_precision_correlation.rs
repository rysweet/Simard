//! Failing TDD tests for the HYBRID correlation endpoint (issue #2491 /
//! measurement issue #2494, G1 hybrid measurement, Step 7).
//!
//! `GET /api/cognition/recall-precision` is the read-only, query-time join that
//! makes the measurement *hybrid*: it pairs the latest FIXED-corpus benchmark
//! score with the recent LIVE trend on the **same** metric name
//! (`recall_precision_at_k`) and emits a correlation verdict — so a claimed
//! cognition improvement is validated on the benchmark AND observed live, not
//! just one.
//!
//! Reference: `docs/reference/recall-precision-hybrid-api.md#correlation-endpoint`
//!
//! Contract under test (all not-yet-implemented — the compile failure is the
//! intended TDD red state):
//!
//! ```rust
//! // src/operator_commands_dashboard/metrics.rs
//! pub(crate) enum CorrelationVerdict { /* as_str -> documented kebab strings */ }
//! pub(crate) fn correlation_verdict(
//!     benchmark_delta: Option<f64>,   // None => fewer than 2 benchmark records
//!     live_trend_delta: Option<f64>,  // None => fewer than 2 in-window samples
//!     threshold: f64,
//! ) -> CorrelationVerdict;
//! pub(crate) fn clamp_bench_limit(v: Option<u64>) -> u64;   // default 20,  1..=200
//! pub(crate) fn clamp_live_limit(v: Option<u64>) -> u64;    // default 200, 1..=2000
//! pub(crate) fn clamp_window_hours(v: Option<u64>) -> u64;  // default 168, 1..=8760
//! pub(crate) fn recall_precision_correlation_core(
//!     bench_db_path: &Path,
//!     state_root: &Path,             // live rail reads <state_root>/metrics/metrics.jsonl
//!     bench_limit: Option<u64>,
//!     live_limit: Option<u64>,
//!     window_hours: Option<u64>,
//! ) -> (StatusCode, Json<Value>);
//! ```

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use axum::Json;
    use axum::http::StatusCode;
    use chrono::{Duration, Utc};
    use tempfile::TempDir;

    use crate::gym_history::{ScoreHistory, ScoreRecord};
    use crate::operator_commands_dashboard::metrics::{
        CorrelationVerdict, clamp_bench_limit, clamp_live_limit, clamp_window_hours,
        correlation_verdict, recall_precision_correlation_core,
    };
    use crate::self_metrics::MetricEntry;

    const METRIC: &str = "recall_precision_at_k";
    const SUITE: &str = "cognition";
    const T: f64 = 0.01; // the gym regression threshold, per the doc.

    // ── seed helpers (all hermetic, temp-dir scoped, parallel-safe) ─────────

    fn seed_bench(db_path: &Path, scores: &[(f64, i64)]) {
        let history = ScoreHistory::open(db_path).expect("open bench history");
        for (score, ts) in scores {
            history
                .record(&ScoreRecord {
                    suite_id: SUITE.to_string(),
                    scenario_id: METRIC.to_string(),
                    score: *score,
                    timestamp: *ts,
                    commit_hash: Some("deadbeef".to_string()),
                })
                .expect("record bench score");
        }
    }

    fn live_line(value: f64, hours_ago: i64, samples: u64) -> String {
        let entry = MetricEntry {
            timestamp: Utc::now() - Duration::hours(hours_ago),
            metric_name: METRIC.to_string(),
            value,
            context: format!("{{\"site\":\"recall_facts_ranked\",\"samples\":{samples}}}"),
        };
        serde_json::to_string(&entry).expect("serialize MetricEntry")
    }

    fn write_live(state_root: &Path, lines: &[String]) {
        let dir = state_root.join("metrics");
        std::fs::create_dir_all(&dir).expect("create metrics dir");
        let mut body = lines.join("\n");
        body.push('\n');
        std::fs::write(dir.join("metrics.jsonl"), body).expect("write metrics.jsonl");
    }

    // ── HYBRID happy path: benchmark up + live up => confirmed ───────────────

    #[tokio::test]
    async fn correlation_confirms_when_both_rails_improve() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("gym_history.db");
        // Benchmark rose 0.78 -> 0.83 (delta +0.05, up).
        seed_bench(&db, &[(0.78, 100), (0.83, 200)]);
        // Live rose 0.80 -> 0.82 in-window (trend delta +0.02, up).
        write_live(tmp.path(), &[live_line(0.80, 2, 6), live_line(0.82, 1, 9)]);

        let (status, Json(body)) =
            recall_precision_correlation_core(&db, tmp.path(), None, None, None);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["metric"], METRIC,
            "response advertises the shared metric name"
        );
        assert!(
            body["benchmark"].is_object(),
            "benchmark rail must be populated"
        );
        assert!(body["live"].is_object(), "live rail must be populated");
        assert_eq!(
            body["live"]["samples"], 2,
            "both in-window live samples must be counted"
        );
        assert_eq!(
            body["correlation"]["verdict"], "confirmed",
            "both rails up beyond the threshold => confirmed"
        );
        assert!(
            body["generated_at"].is_string(),
            "response must carry a generated_at timestamp"
        );

        // DATA-1: no on-disk paths may leak into the response body.
        let text = serde_json::to_string(&body).unwrap();
        assert!(
            !text.contains(tmp.path().to_string_lossy().as_ref()),
            "response must not leak the temp state-root/db path (DATA-1)"
        );
    }

    /// A rail with no history degrades to `insufficient`, HTTP stays 200, and
    /// the missing section is null — never a panic or a 500.
    #[tokio::test]
    async fn correlation_insufficient_when_rails_empty() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("gym_history.db"); // never created => no records

        let (status, Json(body)) =
            recall_precision_correlation_core(&db, tmp.path(), None, None, None);

        assert_eq!(status, StatusCode::OK, "empty rails still return 200");
        assert!(body["benchmark"].is_null(), "absent benchmark rail is null");
        assert_eq!(
            body["correlation"]["verdict"], "insufficient",
            "no data on either rail => insufficient verdict"
        );
    }

    // ── MUST #1 (VAL-3): corrupt live rows are skipped, not fatal ───────────

    #[tokio::test]
    async fn corrupt_live_rows_are_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("gym_history.db");
        seed_bench(&db, &[(0.78, 100), (0.83, 200)]);
        // A garbage line wedged between two valid samples must be skipped.
        write_live(
            tmp.path(),
            &[
                live_line(0.80, 2, 6),
                "this is not valid json at all".to_string(),
                live_line(0.82, 1, 9),
            ],
        );

        let (status, Json(body)) =
            recall_precision_correlation_core(&db, tmp.path(), None, None, None);

        assert_eq!(
            status,
            StatusCode::OK,
            "a corrupt row must not 500 the endpoint"
        );
        assert_eq!(
            body["live"]["samples"], 2,
            "the two valid samples are read; the garbage row is skipped (VAL-3)"
        );
        let text = serde_json::to_string(&body).unwrap();
        assert!(
            !text.contains("this is not valid json"),
            "raw corrupt content must never be echoed into the response"
        );
    }

    // ── MUST #2 (DATA-1): read error yields a generic body, no leak ─────────

    #[tokio::test]
    async fn read_error_returns_generic_body_without_leaking_specifics() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("gym_history.db");
        // A non-SQLite file: opening/querying the score history fails.
        std::fs::write(&db, b"this file is not a sqlite database\x00\x01\x02").unwrap();
        // Live rail is fine — it must degrade independently of the DB failure.
        write_live(tmp.path(), &[live_line(0.80, 2, 6), live_line(0.82, 1, 9)]);

        let (status, Json(body)) =
            recall_precision_correlation_core(&db, tmp.path(), None, None, None);

        assert_eq!(
            status,
            StatusCode::OK,
            "a read error degrades to 200, not 500"
        );
        assert!(
            body["benchmark"].is_null(),
            "the failed benchmark rail is null"
        );
        assert!(
            body["error"].is_string(),
            "a read error surfaces a top-level generic error message"
        );

        let text = serde_json::to_string(&body).unwrap();
        let low = text.to_lowercase();
        assert!(
            !text.contains(tmp.path().to_string_lossy().as_ref()),
            "error body must not leak the on-disk path (DATA-1)"
        );
        assert!(
            !low.contains("sqlite") && !low.contains("not a database") && !low.contains("sql"),
            "error body must not leak SQL/engine specifics; those go to tracing::warn! only (DATA-1)"
        );
    }

    // ── MUST #3: query params are clamped, never rejected ───────────────────

    #[test]
    fn bench_limit_is_clamped_to_1_200_default_20() {
        assert_eq!(clamp_bench_limit(None), 20, "default");
        assert_eq!(clamp_bench_limit(Some(0)), 1, "0 clamps up to the min");
        assert_eq!(clamp_bench_limit(Some(1)), 1);
        assert_eq!(clamp_bench_limit(Some(50)), 50, "in-range is identity");
        assert_eq!(
            clamp_bench_limit(Some(9_999)),
            200,
            "clamps down to the max"
        );
    }

    #[test]
    fn live_limit_is_clamped_to_1_2000_default_200() {
        assert_eq!(clamp_live_limit(None), 200, "default");
        assert_eq!(clamp_live_limit(Some(0)), 1);
        assert_eq!(clamp_live_limit(Some(500)), 500);
        assert_eq!(clamp_live_limit(Some(1_000_000)), 2000);
    }

    #[test]
    fn window_hours_is_clamped_to_1_8760_default_168() {
        assert_eq!(clamp_window_hours(None), 168, "default one week");
        assert_eq!(clamp_window_hours(Some(0)), 1);
        assert_eq!(clamp_window_hours(Some(720)), 720);
        assert_eq!(
            clamp_window_hours(Some(1_000_000)),
            8760,
            "clamps to one year"
        );
    }

    // ── MUST #4 (AUTH): endpoint is inside the require_auth layer ───────────

    fn routes_source() -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/operator_commands_dashboard/routes.rs");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
    }

    #[test]
    fn correlation_route_is_registered() {
        let src = routes_source();
        assert!(
            src.contains("/api/cognition/recall-precision"),
            "GET /api/cognition/recall-precision must be registered in routes.rs"
        );
    }

    #[test]
    fn correlation_route_is_inside_require_auth_scope() {
        let src = routes_source();
        let auth_layer = src
            .find(".layer(middleware::from_fn(require_auth))")
            .expect("require_auth layer must be applied in build_router");
        let route_pos = src
            .find("/api/cognition/recall-precision")
            .expect("route must be present in routes.rs");
        assert!(
            route_pos < auth_layer,
            "the correlation route must be registered BEFORE .layer(require_auth) so it is \
             fail-closed under auth (AUTH)"
        );
    }

    // ── Correlation verdict: the total truth table ──────────────────────────
    //
    // Directions at threshold t=0.01: up=+0.05, flat=0.0, down=-0.05.
    // Rows = benchmark, columns = live trend (per the reference matrix).

    fn verdict(b: Option<f64>, l: Option<f64>) -> String {
        correlation_verdict(b, l, T).as_str().to_string()
    }

    #[test]
    fn verdict_matrix_is_total_and_matches_the_reference() {
        let up = Some(0.05);
        let flat = Some(0.0);
        let down = Some(-0.05);

        // bench up
        assert_eq!(verdict(up, up), "confirmed");
        assert_eq!(verdict(up, flat), "benchmark-only");
        assert_eq!(verdict(up, down), "diverging");
        // bench flat
        assert_eq!(verdict(flat, up), "live-only");
        assert_eq!(verdict(flat, flat), "stable");
        assert_eq!(verdict(flat, down), "regressed");
        // bench down
        assert_eq!(verdict(down, up), "diverging");
        assert_eq!(verdict(down, flat), "regressed");
        assert_eq!(verdict(down, down), "regressed");
    }

    #[test]
    fn verdict_is_insufficient_when_a_rail_lacks_history() {
        assert_eq!(verdict(None, Some(0.05)), "insufficient");
        assert_eq!(verdict(Some(0.05), None), "insufficient");
        assert_eq!(verdict(None, None), "insufficient");
    }

    #[test]
    fn verdict_boundary_is_flat_at_exactly_the_threshold() {
        // |delta| == t is flat (up requires strictly greater than +t).
        assert_eq!(verdict(Some(T), Some(0.05)), "live-only", "b flat, l up");
        assert_eq!(
            verdict(Some(0.05), Some(-T)),
            "benchmark-only",
            "b up, l flat"
        );
        assert_eq!(verdict(Some(-T), Some(T)), "stable", "both within ±t");
    }

    #[test]
    fn verdict_diverging_is_first_class_not_a_regression() {
        // One rail up while the other regresses is a distinct, stronger distrust
        // signal — it must not be folded into "regressed".
        let v = correlation_verdict(Some(0.05), Some(-0.05), T);
        assert!(matches!(v, CorrelationVerdict::Diverging));
        assert_eq!(v.as_str(), "diverging");
    }
}
