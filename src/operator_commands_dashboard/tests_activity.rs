//! Tests for `activity.rs` handler functions (issue #1750).
//!
//! Both `traces()` and `activity()` gracefully handle missing files and
//! failing external commands, so we test them with controlled (empty) state
//! roots and ensure the JSON shape is always well-formed.

use crate::operator_commands_dashboard::activity::{activity, traces};
use crate::test_support::HermeticState;

// ---------------------------------------------------------------------------
// traces()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn traces_returns_valid_json_structure() {
    // Even with no trace files or journald, traces() should return a valid
    // JSON response with the expected top-level keys.
    let result = traces().await;
    let val = &result.0;

    assert!(val.get("span_count").is_some(), "missing 'span_count' key");
    assert!(val.get("spans").is_some(), "missing 'spans' key");
    assert!(val["spans"].is_array(), "spans should be an array");
    assert!(
        val.get("otel_enabled").is_some(),
        "missing 'otel_enabled' key"
    );
    assert!(val.get("timestamp").is_some(), "missing 'timestamp' key");
}

#[tokio::test]
async fn traces_span_count_matches_array_length() {
    let result = traces().await;
    let val = &result.0;

    let count = val["span_count"].as_u64().unwrap_or(0);
    let arr_len = val["spans"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(
        count as usize, arr_len,
        "span_count ({count}) should match spans array length ({arr_len})"
    );
}

#[tokio::test]
async fn traces_otel_enabled_reflects_env_var() {
    // Without the env var set, otel should be disabled.
    let result = traces().await;
    let val = &result.0;
    let has_otel_env = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
    assert_eq!(
        val["otel_enabled"].as_bool().unwrap_or(false),
        has_otel_env,
        "otel_enabled should reflect whether OTEL_EXPORTER_OTLP_ENDPOINT is set"
    );
}

#[tokio::test]
async fn traces_timestamp_is_valid_rfc3339() {
    let result = traces().await;
    let ts = result.0["timestamp"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(ts).is_ok(),
        "timestamp should be valid RFC3339: {ts}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn traces_reads_cost_ledger_when_present() {
    // HermeticState sets SIMARD_STATE_ROOT to a tempdir and cleans up on Drop,
    // so tests never touch $HOME/.simard and panics cannot leave state residue.
    let state = HermeticState::new();
    let ledger_dir = state.state_root().join("costs");
    std::fs::create_dir_all(&ledger_dir).unwrap();

    let ledger_path = ledger_dir.join("ledger.jsonl");
    let test_line = r#"{"model":"test","cost_usd":0.001,"timestamp":"2025-01-01T00:00:00Z"}"#;
    std::fs::write(&ledger_path, format!("{test_line}\n")).unwrap();

    let result = traces().await;
    let spans = result.0["spans"].as_array().unwrap();
    let has_cost = spans.iter().any(|s| s["source"] == "cost");
    assert!(has_cost, "should have at least one cost span from ledger");
}

// ---------------------------------------------------------------------------
// activity()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_returns_valid_json_structure() {
    let result = activity().await;
    let val = &result.0;

    assert!(val.get("daemon").is_some(), "missing 'daemon' key");
    assert!(
        val.get("recent_cycles").is_some(),
        "missing 'recent_cycles' key"
    );
    assert!(val.get("open_prs").is_some(), "missing 'open_prs' key");
    assert!(
        val.get("assigned_issues").is_some(),
        "missing 'assigned_issues' key"
    );
    assert!(val.get("timestamp").is_some(), "missing 'timestamp' key");
}

#[tokio::test]
async fn activity_daemon_section_has_expected_fields() {
    let result = activity().await;
    let daemon = &result.0["daemon"];

    assert!(daemon.get("status").is_some(), "daemon missing 'status'");
    assert!(
        daemon.get("current_cycle").is_some(),
        "daemon missing 'current_cycle'"
    );
    assert!(
        daemon.get("last_heartbeat").is_some(),
        "daemon missing 'last_heartbeat'"
    );
    assert!(
        daemon.get("actions_taken").is_some(),
        "daemon missing 'actions_taken'"
    );
}

#[tokio::test]
async fn activity_timestamp_is_valid_rfc3339() {
    let result = activity().await;
    let ts = result.0["timestamp"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(ts).is_ok(),
        "timestamp should be valid RFC3339: {ts}"
    );
}

#[tokio::test]
async fn activity_recent_cycles_is_array() {
    let result = activity().await;
    assert!(
        result.0["recent_cycles"].is_array(),
        "recent_cycles should be an array"
    );
}

#[tokio::test]
async fn activity_open_prs_is_array() {
    let result = activity().await;
    assert!(
        result.0["open_prs"].is_array(),
        "open_prs should be an array"
    );
}

#[tokio::test]
async fn activity_assigned_issues_is_array() {
    let result = activity().await;
    assert!(
        result.0["assigned_issues"].is_array(),
        "assigned_issues should be an array"
    );
}

#[tokio::test]
async fn activity_daemon_status_is_string() {
    let result = activity().await;
    let status = &result.0["daemon"]["status"];
    assert!(
        status.is_string(),
        "daemon status should be a string, got: {status}"
    );
    let s = status.as_str().unwrap();
    // The value is derived from daemon_health.json — should be a known status string
    assert!(!s.is_empty(), "daemon status should be a non-empty string");
}

/// RAII guard that sets an env var for its lifetime and restores on Drop.
/// Used to redirect XDG_DATA_HOME so `dirs::data_local_dir()` resolves
/// inside the hermetic tempdir.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: tests using this guard are serialised via
        // #[serial(cognitive_memory)].
        unsafe { std::env::set_var(key, value.as_os_str()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn activity_reads_daemon_health_when_present() {
    // HermeticState isolates SIMARD_STATE_ROOT. We also redirect
    // XDG_DATA_HOME so dirs::data_local_dir() resolves inside the tempdir.
    let state = HermeticState::new();
    let xdg_root = state.state_root().join("xdg_data");
    let _xdg_guard = EnvGuard::set("XDG_DATA_HOME", &xdg_root);

    let health_dir = xdg_root.join("simard");
    std::fs::create_dir_all(&health_dir).unwrap();

    let now = chrono::Utc::now().to_rfc3339();
    let fake_health = serde_json::json!({
        "status": "running",
        "cycle_number": 42,
        "timestamp": now,
        "actions_taken": ["advance-goal", "consolidate-memory"]
    });
    std::fs::write(
        health_dir.join("daemon_health.json"),
        fake_health.to_string(),
    )
    .unwrap();

    let result = activity().await;
    let daemon = &result.0["daemon"];
    assert_eq!(daemon["current_cycle"], 42);
    assert_eq!(daemon["status"], "running");
}
