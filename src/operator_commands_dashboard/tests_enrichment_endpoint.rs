//! TDD (Step 7) — failing tests for the enrichment observability dashboard
//! endpoint `GET /api/enrichment` (issue #2942).
//!
//! Surfaces on the Memory tab whether recall is reaching decisions: the
//! attach-rate and average facts/procedures/preamble-bytes injected per
//! decision, read from the **live** store
//! (`<state_root>/telemetry/metrics_snapshot.json` `enrichment` section),
//! consistent with the goal-board-live-read direction. Reference:
//! `docs/reference/enrichment-observability-api.md#endpoint-get-apienrichment`.
//!
//! These live **in-crate** (not `tests/`) because `mod
//! operator_commands_dashboard` is crate-private, so `build_router` and the
//! `pub(crate)` core are unreachable from an integration test. This mirrors the
//! sibling `tests_recall_precision_correlation` module exactly.
//!
//! Contract under test (not-yet-implemented — the compile/assert failures are
//! the intended TDD red state):
//!
//! ```rust
//! // src/operator_commands_dashboard/enrichment.rs
//! pub(crate) fn clamp_window_hours(v: Option<u64>) -> u64; // default 24,  1..=8760
//! pub(crate) fn clamp_limit(v: Option<u64>) -> u64;        // default 500, 1..=1000
//! pub(crate) fn enrichment_core(
//!     state_root: &Path,
//!     window_hours: Option<u64>,
//!     limit: Option<u64>,
//! ) -> (StatusCode, Json<Value>);
//! pub(crate) async fn enrichment(/* Query params */) -> (StatusCode, Json<Value>);
//! ```

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use axum::Json;
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use crate::operator_commands_dashboard::enrichment::{
        clamp_limit, clamp_window_hours, enrichment_core,
    };

    // ── seed helpers (hermetic, temp-dir scoped) ────────────────────────────

    fn seed_snapshot(state_root: &Path, enrichment: Value) {
        let path = crate::telemetry::snapshot::snapshot_path(state_root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let snap = json!({
            "schema_version": 1,
            // Written "now" so the reader classifies it as `live`.
            "captured_at": crate::telemetry::snapshot::now_rfc3339(),
            "counters": [],
            "gauges": [],
            "histograms": [],
            "overflow_series": 0,
            "enrichment": enrichment,
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&snap).unwrap()).unwrap();
    }

    /// The documented `enrichment` snapshot section (the reference JSON example).
    fn full_enrichment_section() -> Value {
        json!({
            "window_start": "2026-07-07T18:00:00Z",
            "window_end":   "2026-07-07T20:00:00Z",
            "decisions": 42,
            "attached": 40,
            "attach_rate": 0.9524,
            "degraded": { "memory_ipc": 2, "knowledge_launch": 0 },
            "avg_facts_injected": 6.3,
            "avg_procedures_injected": 2.8,
            "avg_preamble_bytes": 771.5,
            "last": {
                "attached": true,
                "facts_injected": 7,
                "procedures_injected": 3,
                "preamble_bytes": 812,
                "at": "2026-07-07T19:58:11Z"
            }
        })
    }

    // ── query-param clamps (total function; never reject) ───────────────────

    #[test]
    fn window_hours_is_clamped_to_1_8760_default_24() {
        assert_eq!(clamp_window_hours(None), 24, "default trailing window");
        assert_eq!(clamp_window_hours(Some(0)), 1, "0 clamps up to the min");
        assert_eq!(clamp_window_hours(Some(24)), 24, "in-range is identity");
        assert_eq!(
            clamp_window_hours(Some(1_000_000)),
            8760,
            "clamps down to one year"
        );
    }

    #[test]
    fn limit_is_clamped_to_1_1000_default_500() {
        assert_eq!(clamp_limit(None), 500, "default scan cap");
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(500)), 500);
        assert_eq!(clamp_limit(Some(1_000_000)), 1000, "clamps to the max");
    }

    // ── missing snapshot degrades safely (no panic, never 4xx/5xx) ──────────

    #[test]
    fn missing_snapshot_is_degrade_safe() {
        let tmp = TempDir::new().unwrap();
        // No telemetry/metrics_snapshot.json exists under this state root.

        let (status, Json(body)) = enrichment_core(tmp.path(), None, None);

        assert_eq!(
            status,
            StatusCode::OK,
            "a missing snapshot still returns 200"
        );
        assert_eq!(
            body["available"], false,
            "a missing snapshot is reported as unavailable"
        );
        assert_eq!(
            body["freshness"], "missing",
            "freshness must be 'missing' so the panel never renders a false 0%"
        );
        assert!(
            body["attach_rate"].is_null(),
            "magnitudes are null when there is no snapshot"
        );
        assert!(
            body["avg_facts_injected"].is_null(),
            "average magnitudes are null when unavailable"
        );
    }

    // ── populated snapshot surfaces the live figures ────────────────────────

    #[test]
    fn populated_snapshot_surfaces_attach_rate_and_averages() {
        let tmp = TempDir::new().unwrap();
        seed_snapshot(tmp.path(), full_enrichment_section());

        let (status, Json(body)) = enrichment_core(tmp.path(), None, None);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["available"], true, "a fresh snapshot is available");
        assert_eq!(
            body["freshness"], "live",
            "a just-written snapshot must be classified live"
        );
        assert_eq!(body["decisions"], 42);
        assert_eq!(body["attached"], 40);
        let attach_rate = body["attach_rate"]
            .as_f64()
            .expect("attach_rate must be a number when decisions > 0");
        assert!(
            (attach_rate - 0.9524).abs() < 1e-6,
            "attach_rate must surface the live value, got {attach_rate}"
        );
        assert_eq!(
            body["degraded"]["memory_ipc"], 2,
            "the memory-ipc degrade count is surfaced (operator's cue a reader is down)"
        );
        assert_eq!(body["degraded"]["knowledge_launch"], 0);
        assert_eq!(body["avg_facts_injected"], 6.3);
        assert_eq!(body["avg_procedures_injected"], 2.8);
        assert_eq!(body["avg_preamble_bytes"], 771.5);
        // The most recent decision is available for spot-checking.
        assert_eq!(body["last"]["attached"], true);
        assert_eq!(body["last"]["preamble_bytes"], 812);

        // DATA-1: the on-disk state-root path must never leak into the body.
        let text = serde_json::to_string(&body).unwrap();
        assert!(
            !text.contains(tmp.path().to_string_lossy().as_ref()),
            "response must not leak the temp state-root path (DATA-1)"
        );
    }

    // ── out-of-range params are clamped, still 200 ──────────────────────────

    #[test]
    fn out_of_range_params_are_clamped_not_rejected() {
        let tmp = TempDir::new().unwrap();
        seed_snapshot(tmp.path(), full_enrichment_section());

        // window_hours=0 and limit=huge are clamped internally, not rejected.
        let (status, Json(body)) = enrichment_core(tmp.path(), Some(0), Some(10_000_000));
        assert_eq!(status, StatusCode::OK, "bad params are clamped, never 4xx");
        assert_eq!(body["available"], true);
        // The echoed window reflects the clamp (1..=8760), never the raw 0.
        assert_eq!(
            body["window_hours"], 1,
            "window_hours=0 is clamped up to the minimum and echoed"
        );
    }

    // ── freshness window matches the once-per-cycle flush cadence ───────────
    //
    // The daemon flushes this snapshot once per OODA cycle, so a healthy reader
    // routinely sees a `captured_at` several hundred seconds old (cycle runtime
    // + the ~300s inter-cycle sleep). The freshness window is 900s — matching
    // the dashboard's daemon-liveness check — so such a snapshot is `live`, not
    // a false `stale`. Regression guard for the historical 300s threshold that
    // fired on essentially every healthy cycle.

    fn seed_snapshot_aged(state_root: &Path, enrichment: Value, age_secs: i64) {
        let path = crate::telemetry::snapshot::snapshot_path(state_root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let captured_at = (chrono::Utc::now() - chrono::Duration::seconds(age_secs)).to_rfc3339();
        let snap = json!({
            "schema_version": 1,
            "captured_at": captured_at,
            "counters": [],
            "gauges": [],
            "histograms": [],
            "overflow_series": 0,
            "enrichment": enrichment,
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&snap).unwrap()).unwrap();
    }

    #[test]
    fn once_per_cycle_aged_snapshot_is_live_not_stale() {
        let tmp = TempDir::new().unwrap();
        // 600s ≈ one full cycle old: well past the old 300s threshold but within
        // a single healthy cycle. Must NOT be classified stale.
        seed_snapshot_aged(tmp.path(), full_enrichment_section(), 600);

        let (status, Json(body)) = enrichment_core(tmp.path(), None, None);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["freshness"], "live",
            "a snapshot flushed one cycle ago (600s) must be live, not a false stale"
        );
        assert_eq!(body["available"], true);
    }

    #[test]
    fn genuinely_old_snapshot_is_stale() {
        let tmp = TempDir::new().unwrap();
        // 1000s > the 900s freshness window: a daemon that stopped flushing.
        seed_snapshot_aged(tmp.path(), full_enrichment_section(), 1000);

        let (_status, Json(body)) = enrichment_core(tmp.path(), None, None);

        assert_eq!(
            body["freshness"], "stale",
            "a snapshot older than the freshness window is stale"
        );
    }

    // ── route registration + fail-closed auth (source scan) ─────────────────

    fn routes_source() -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/operator_commands_dashboard/routes.rs");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
    }

    #[test]
    fn enrichment_route_is_registered() {
        assert!(
            routes_source().contains("/api/enrichment"),
            "GET /api/enrichment must be registered in routes.rs"
        );
    }

    #[test]
    fn enrichment_route_is_inside_require_auth_scope() {
        let src = routes_source();
        let auth_layer = src
            .find(".layer(middleware::from_fn(require_auth))")
            .expect("require_auth layer must be applied in build_router");
        let route_pos = src
            .find("/api/enrichment")
            .expect("route must be present in routes.rs");
        assert!(
            route_pos < auth_layer,
            "the enrichment route must be registered BEFORE .layer(require_auth) so it is \
             fail-closed under auth (AUTHZ)"
        );
    }

    // ── real HTTP: unauth 401, authed 200 (not 404) ─────────────────────────

    /// Drives the REAL `build_router()` over an ephemeral loopback server:
    /// an unauthenticated `GET /api/enrichment` must be denied `401`, and an
    /// authenticated one must reach the handler (`200`, not `404`). `SIMARD_
    /// STATE_ROOT` is scoped to an empty temp dir so the handler reads a
    /// controlled (missing) snapshot and stays degrade-safe. Mutates process
    /// env, so it carries the `cognitive_memory` serial key.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn enrichment_endpoint_denies_unauth_and_serves_authed() {
        use crate::operator_commands_dashboard::auth;
        use crate::operator_commands_dashboard::routes::build_router;
        use std::net::SocketAddr;
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn http_get(addr: SocketAddr, path: &str, bearer: Option<&str>) -> (u16, String) {
            let mut stream = tokio::net::TcpStream::connect(addr)
                .await
                .expect("connect to ephemeral dashboard server");
            let mut req =
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
            if let Some(b) = bearer {
                req.push_str(&format!("Authorization: Bearer {b}\r\n"));
            }
            req.push_str("\r\n");
            stream
                .write_all(req.as_bytes())
                .await
                .expect("write request");
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await.expect("read response");
            let text = String::from_utf8_lossy(&raw).into_owned();
            let code = text
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(0);
            let body = text
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            (code, body)
        }

        let tmp = TempDir::new().unwrap();
        let prev_state_root = std::env::var_os("SIMARD_STATE_ROOT");
        let prev_token = std::env::var_os("SIMARD_DASHBOARD_TOKEN");
        // SAFETY: env mutation is serialised by `#[serial(cognitive_memory)]`;
        // both vars are restored before the test returns / propagates a panic.
        unsafe {
            std::env::set_var("SIMARD_STATE_ROOT", tmp.path());
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.block_on(async {
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .expect("bind ephemeral loopback port");
                let addr: SocketAddr = listener.local_addr().expect("local addr");
                tokio::spawn(async move {
                    let _ = axum::serve(listener, build_router()).await;
                });

                // Unauthenticated: must be denied 401 (fail-closed).
                let (unauth, _) = tokio::time::timeout(
                    Duration::from_secs(30),
                    http_get(addr, "/api/enrichment", None),
                )
                .await
                .expect("unauth /api/enrichment timed out");
                assert_eq!(
                    unauth, 401,
                    "unauthenticated GET /api/enrichment must be denied (401)"
                );

                // Authenticated: must reach the handler (200), not 404.
                auth::init_login_code();
                let token = "itest-enrichment-2942";
                unsafe { std::env::set_var("SIMARD_DASHBOARD_TOKEN", token) };
                let (ok, body) = tokio::time::timeout(
                    Duration::from_secs(30),
                    http_get(addr, "/api/enrichment", Some(token)),
                )
                .await
                .expect("authed /api/enrichment timed out");
                assert_eq!(
                    ok, 200,
                    "authenticated GET /api/enrichment must reach the handler (200), not 404; \
                     body={body:?}"
                );
                let json: Value =
                    serde_json::from_str(&body).expect("/api/enrichment must return a JSON object");
                // Empty state root => degrade-safe, not an error.
                assert_eq!(
                    json["available"], false,
                    "an empty state root yields available:false, not a 5xx"
                );
            });
        }));

        // SAFETY: restore env under the same serial key before resuming a panic.
        unsafe {
            match prev_state_root {
                Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
                None => std::env::remove_var("SIMARD_STATE_ROOT"),
            }
            match prev_token {
                Some(v) => std::env::set_var("SIMARD_DASHBOARD_TOKEN", v),
                None => std::env::remove_var("SIMARD_DASHBOARD_TOKEN"),
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }
}
