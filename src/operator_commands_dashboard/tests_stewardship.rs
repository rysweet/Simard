//! TDD handler tests for the Stewardship + Self-Understanding API
//! (issue #1172).
//!
//! Pins behavioral contract for two new endpoints:
//!   * GET /api/stewardship       -> reads ~/.simard/stewardship.json
//!   * GET /api/self-understanding -> tails ~/.simard/metrics/metrics.jsonl
//!     + summarizes RuntimeSnapshot
//!
//! Both handlers MUST be fail-soft (always return a valid JSON envelope,
//! never panic, never expose raw file contents in errors) and MUST be
//! wired inside the `require_auth` middleware scope by `build_router()`.
//!
//! These tests will FAIL to compile until Step 8 introduces:
//!   * `super::routes::stewardship_handler`
//!   * `super::routes::self_understanding_handler`
//!   * `super::routes::stewardship_config_path`
//!   * `super::routes::metrics_jsonl_path`

use axum::Json;
use serde_json::Value;
use serial_test::serial;
use std::fs;

use super::routes::{
    build_router, metrics_jsonl_path, self_understanding_handler, stewardship_config_path,
    stewardship_handler,
};

/// Set HOME to a temp dir for the duration of the closure. All tests in this
/// file must be `#[serial]` because HOME is process-global.
fn with_home<F: FnOnce(&std::path::Path)>(f: F) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let prev = std::env::var_os("HOME");
    // SAFETY: tests are #[serial], no other test mutates HOME concurrently.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(tmp.path())));
    // Restore HOME before re-raising any panic.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

// ---------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------

#[test]
#[serial]
fn stewardship_config_path_is_under_dot_simard() {
    with_home(|home| {
        let p = stewardship_config_path();
        assert_eq!(
            p,
            home.join(".simard").join("stewardship.json"),
            "stewardship_config_path must resolve to ~/.simard/stewardship.json"
        );
    });
}

#[test]
#[serial]
fn metrics_jsonl_path_is_under_dot_simard_metrics() {
    with_home(|home| {
        let p = metrics_jsonl_path();
        assert_eq!(
            p,
            home.join(".simard").join("metrics").join("metrics.jsonl"),
            "metrics_jsonl_path must resolve to ~/.simard/metrics/metrics.jsonl"
        );
    });
}

// ---------------------------------------------------------------------
// stewardship_handler — fail-soft envelope
// ---------------------------------------------------------------------

fn unwrap_json(j: Json<Value>) -> Value {
    j.0
}

#[tokio::test]
#[serial]
async fn stewardship_handler_returns_empty_repos_when_file_missing() {
    with_home(|_home| {});
    // Use the inner block to keep HOME stable across the await.
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }

    let body = unwrap_json(stewardship_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let repos = body
        .get("repos")
        .and_then(|v| v.as_array())
        .expect("envelope must have a 'repos' array (fail-soft: empty when file absent)");
    assert!(
        repos.is_empty(),
        "missing stewardship.json must yield empty repos array, got: {body}"
    );
    // Optional warning surface: when the file is missing we expose a hint via
    // a `warning` field rather than a non-200 status. Non-strict shape: presence
    // is allowed but not required.
    if let Some(w) = body.get("warning") {
        assert!(
            w.is_string(),
            "if present, 'warning' must be a string, got: {w}"
        );
    }
}

#[tokio::test]
#[serial]
async fn stewardship_handler_returns_repos_from_valid_json() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let dir = tmp.path().join(".simard");
    fs::create_dir_all(&dir).unwrap();
    let payload = serde_json::json!([
        {"repo": "rysweet/Simard", "role": "primary", "last_activity": "2026-04-22", "notes": "active"},
        {"repo": "rysweet/lbug",   "role": "support", "last_activity": "2026-04-15", "notes": ""}
    ]);
    fs::write(dir.join("stewardship.json"), payload.to_string()).unwrap();

    let body = unwrap_json(stewardship_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let repos = body
        .get("repos")
        .and_then(|v| v.as_array())
        .expect("envelope must have a 'repos' array");
    assert_eq!(
        repos.len(),
        2,
        "expected 2 repos round-tripped, got: {body}"
    );
    assert_eq!(repos[0]["repo"], "rysweet/Simard");
    assert_eq!(repos[1]["role"], "support");
}

#[tokio::test]
#[serial]
async fn stewardship_handler_failsoft_on_malformed_json() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let dir = tmp.path().join(".simard");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("stewardship.json"), b"{ this is not json").unwrap();

    let body = unwrap_json(stewardship_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    // Fail-soft: still a valid envelope, repos is an empty array, warning surfaced.
    let repos = body
        .get("repos")
        .and_then(|v| v.as_array())
        .expect("envelope must have a 'repos' array even on parse failure");
    assert!(
        repos.is_empty(),
        "malformed JSON must produce empty repos, not panic, got: {body}"
    );
    let warning = body
        .get("warning")
        .and_then(|v| v.as_str())
        .expect("malformed JSON must surface a 'warning' string in the envelope");
    assert!(
        !warning.contains("this is not json"),
        "warning must NOT echo raw file contents (info-disclosure), got: {warning}"
    );
}

#[tokio::test]
#[serial]
async fn stewardship_handler_failsoft_on_oversize_file() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let dir = tmp.path().join(".simard");
    fs::create_dir_all(&dir).unwrap();
    // > 1 MiB cap (design says ≤ 1 MiB).
    let blob = vec![b'a'; 2 * 1024 * 1024];
    fs::write(dir.join("stewardship.json"), &blob).unwrap();

    let body = unwrap_json(stewardship_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let repos = body
        .get("repos")
        .and_then(|v| v.as_array())
        .expect("envelope must have a 'repos' array even on oversize");
    assert!(
        repos.is_empty(),
        "oversize stewardship.json must yield empty repos (DoS cap), got: {body}"
    );
    let warning = body
        .get("warning")
        .and_then(|v| v.as_str())
        .expect("oversize must surface a 'warning' string");
    assert!(
        warning.to_lowercase().contains("size")
            || warning.to_lowercase().contains("large")
            || warning.to_lowercase().contains("limit"),
        "oversize warning should mention size/large/limit, got: {warning}"
    );
}

// ---------------------------------------------------------------------
// self_understanding_handler — envelope contract
// ---------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn self_understanding_handler_returns_envelope_with_uptime_when_no_metrics() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }

    let body = unwrap_json(self_understanding_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    // Always-present uptime_secs (sourced from process-start instant, never panics).
    assert!(
        body.get("uptime_secs").and_then(|v| v.as_u64()).is_some(),
        "envelope must always include uptime_secs:u64, got: {body}"
    );
    // Metrics array must be present (empty when file absent).
    let metrics = body
        .get("metrics")
        .and_then(|v| v.as_array())
        .expect("envelope must have a 'metrics' array");
    assert!(
        metrics.is_empty(),
        "missing metrics.jsonl must yield empty metrics, got: {body}"
    );
    // snapshot key must be present (object or null) — never absent.
    assert!(
        body.get("snapshot").is_some(),
        "envelope must include a 'snapshot' key (may be null), got: {body}"
    );
}

#[tokio::test]
#[serial]
async fn self_understanding_handler_tails_recent_metric_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let dir = tmp.path().join(".simard").join("metrics");
    fs::create_dir_all(&dir).unwrap();
    // Write 50 valid JSONL lines; handler should return at most ~20 (last N).
    let mut lines = String::new();
    for i in 0..50 {
        lines.push_str(&format!(
            "{{\"timestamp\":\"2026-04-22T00:00:{i:02}Z\",\"name\":\"m\",\"value\":{i}}}\n"
        ));
    }
    fs::write(dir.join("metrics.jsonl"), &lines).unwrap();

    let body = unwrap_json(self_understanding_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let metrics = body
        .get("metrics")
        .and_then(|v| v.as_array())
        .expect("metrics array");
    assert!(
        !metrics.is_empty() && metrics.len() <= 20,
        "metrics tail must be 1..=20 lines, got len={}, body={body}",
        metrics.len()
    );
    // Last line should be the highest-numbered (tail returns most recent).
    let last_value = metrics
        .last()
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_u64())
        .expect("last metric must have numeric 'value'");
    assert_eq!(
        last_value, 49,
        "tail must end with the most-recent line (value=49), got {last_value}"
    );
}

#[tokio::test]
#[serial]
async fn self_understanding_handler_skips_malformed_jsonl_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let dir = tmp.path().join(".simard").join("metrics");
    fs::create_dir_all(&dir).unwrap();
    let lines = "\
{\"timestamp\":\"t1\",\"name\":\"a\",\"value\":1}
this is not json
{\"timestamp\":\"t2\",\"name\":\"b\",\"value\":2}
\n";
    fs::write(dir.join("metrics.jsonl"), lines).unwrap();

    let body = unwrap_json(self_understanding_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let metrics = body
        .get("metrics")
        .and_then(|v| v.as_array())
        .expect("metrics array");
    assert_eq!(
        metrics.len(),
        2,
        "malformed lines must be skipped silently (got {}), body={body}",
        metrics.len()
    );
}

#[tokio::test]
#[serial]
async fn self_understanding_handler_never_panics_on_oversize_metrics() {
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let dir = tmp.path().join(".simard").join("metrics");
    fs::create_dir_all(&dir).unwrap();
    // 4 MiB of garbage — handler must not OOM or panic; it must bound its scan.
    let blob = vec![b'x'; 4 * 1024 * 1024];
    fs::write(dir.join("metrics.jsonl"), &blob).unwrap();

    let body = unwrap_json(self_understanding_handler().await);

    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    // Envelope still well-formed; bounded scan yields zero parsed lines.
    assert!(body.get("uptime_secs").is_some(), "uptime_secs required");
    let metrics = body
        .get("metrics")
        .and_then(|v| v.as_array())
        .expect("metrics array");
    assert!(
        metrics.is_empty(),
        "oversize garbage must yield empty metrics, not panic; got len={}",
        metrics.len()
    );
}

// ---------------------------------------------------------------------
// Routing — both endpoints registered inside auth scope
//
// We intentionally avoid pulling in `tower` as a dev-dep just to call
// `oneshot()`. Instead we (a) construct `build_router()` to prove it
// doesn't panic with the new routes wired in, and (b) read the source
// of `routes.rs` to assert each new route is registered BEFORE the
// `.layer(middleware::from_fn(require_auth))` line — which is how this
// router scopes auth to all preceding routes.
// ---------------------------------------------------------------------

#[test]
fn build_router_constructs_with_new_routes() {
    // Smoke test: this will only succeed once the new handlers exist
    // (compile-time) AND `build_router` registers them without panicking.
    let _router = build_router();
}

#[test]
fn stewardship_routes_are_registered_inside_require_auth_scope() {
    let src = std::fs::read_to_string("src/operator_commands_dashboard/routes.rs")
        .expect("routes.rs must be readable from the workspace cwd");
    let auth_anchor = ".layer(middleware::from_fn(require_auth))";
    let auth_pos = src
        .find(auth_anchor)
        .expect("require_auth layer must remain in build_router");

    let stewardship_route = ".route(\"/api/stewardship\"";
    let self_und_route = ".route(\"/api/self-understanding\"";

    let s_pos = src
        .find(stewardship_route)
        .expect("/api/stewardship route must be registered in build_router");
    let u_pos = src
        .find(self_und_route)
        .expect("/api/self-understanding route must be registered in build_router");

    assert!(
        s_pos < auth_pos,
        "/api/stewardship must be registered BEFORE the require_auth layer \
         so it inherits authentication"
    );
    assert!(
        u_pos < auth_pos,
        "/api/self-understanding must be registered BEFORE the require_auth \
         layer so it inherits authentication"
    );
}
