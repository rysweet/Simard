//! Tests for `activity.rs` handler functions (issue #1750).
//!
//! Both `traces()` and `activity()` gracefully handle missing files and
//! failing external commands, so we test them with controlled (empty) state
//! roots and ensure the JSON shape is always well-formed.

use crate::operator_commands_dashboard::activity::{activity, traces};

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
async fn traces_reads_cost_ledger_when_present() {
    // Write a fake cost ledger line and verify traces picks it up
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    let ledger_path = std::path::PathBuf::from(&home).join(".simard/costs/ledger.jsonl");
    let existed = ledger_path.exists();

    // Only test if the file exists or we can create the dir
    if let Some(parent) = ledger_path.parent()
        && (parent.exists() || std::fs::create_dir_all(parent).is_ok())
    {
        let test_line = r#"{"model":"test","cost_usd":0.001,"timestamp":"2025-01-01T00:00:00Z"}"#;
        let had_content = std::fs::read_to_string(&ledger_path)
            .ok()
            .unwrap_or_default();

        // Append a test line
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ledger_path)
            .unwrap();
        writeln!(f, "{test_line}").unwrap();
        drop(f);

        let result = traces().await;
        let spans = result.0["spans"].as_array().unwrap();
        let has_cost = spans.iter().any(|s| s["source"] == "cost");
        assert!(has_cost, "should have at least one cost span from ledger");

        // Restore original content if file didn't exist before
        if !existed {
            let _ = std::fs::remove_file(&ledger_path);
        } else {
            // Restore original content
            std::fs::write(&ledger_path, had_content).unwrap();
        }
    }
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

#[tokio::test]
async fn activity_reads_daemon_health_when_present() {
    // Write a fake daemon_health.json and verify activity() reads it.
    let health_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
        .join("simard");
    let health_path = health_dir.join("daemon_health.json");
    let existed = health_path.exists();
    let backup = if existed {
        std::fs::read_to_string(&health_path).ok()
    } else {
        None
    };

    std::fs::create_dir_all(&health_dir).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let fake_health = serde_json::json!({
        "status": "running",
        "cycle_number": 42,
        "timestamp": now,
        "actions_taken": ["advance-goal", "consolidate-memory"]
    });
    std::fs::write(&health_path, fake_health.to_string()).unwrap();

    let result = activity().await;
    let daemon = &result.0["daemon"];
    assert_eq!(daemon["current_cycle"], 42);
    assert_eq!(daemon["status"], "running");

    // Restore
    if let Some(content) = backup {
        std::fs::write(&health_path, content).unwrap();
    } else {
        let _ = std::fs::remove_file(&health_path);
    }
}
