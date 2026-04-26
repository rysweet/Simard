use axum::Json;
use serde_json::{Value, json};

use super::logs::read_tail;
use super::routes::{resolve_state_root, run_gh_json};
use super::current_work::read_recent_cycle_reports;

pub(crate) async fn traces() -> Json<Value> {
    // Read recent spans from the trace log file
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    let trace_sources = vec![(
        std::path::PathBuf::from(&home).join(".simard/costs/ledger.jsonl"),
        "cost",
    )];

    let mut spans: Vec<Value> = Vec::new();

    for (path, source) in &trace_sources {
        if let Some(lines) = read_tail(&path.to_string_lossy(), 100) {
            for line in lines.iter().rev().take(50) {
                if let Ok(val) = serde_json::from_str::<Value>(line) {
                    spans.push(json!({
                        "source": source,
                        "data": val,
                    }));
                }
            }
        }
    }

    // Also read from journalctl if available (last 100 simard-ooda entries)
    if let Ok(output) = tokio::process::Command::new("journalctl")
        .args([
            "--user",
            "-u",
            "simard-ooda",
            "--no-pager",
            "-n",
            "50",
            "-o",
            "json",
        ])
        .output()
        .await
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines().take(50) {
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                spans.push(json!({"source": "journald", "data": val}));
            }
        }
    }

    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();

    // Include in-process span data from SpanCollectorLayer
    let recent_spans: Vec<Value> = crate::trace_collector::drain_recent(100)
        .into_iter()
        .map(|s| {
            json!({
                "source": "in-process",
                "data": {
                    "name": s.name,
                    "target": s.target,
                    "level": s.level,
                    "duration_us": s.duration_us,
                    "fields": s.fields,
                    "timestamp_epoch_ms": s.timestamp_epoch_ms,
                }
            })
        })
        .collect();
    spans.extend(recent_spans);

    Json(json!({
        "span_count": spans.len(),
        "spans": spans,
        "otel_enabled": otel_endpoint.is_some(),
        "otel_endpoint": otel_endpoint,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Live activity view: current OODA state, in-flight actions, recent cycle
/// outcomes, open PRs, and assigned issues.
pub(crate) async fn activity() -> Json<Value> {
    // --- 1. Daemon health (current cycle & phase) ---
    let health_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
        .join("simard")
        .join("daemon_health.json");

    let daemon_health: Option<Value> = std::fs::read_to_string(&health_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let current_cycle = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_number"))
        .cloned()
        .unwrap_or(json!(null));

    let daemon_status = daemon_health
        .as_ref()
        .and_then(|h| h.get("status"))
        .cloned()
        .unwrap_or(json!("stopped"));

    let last_heartbeat = daemon_health
        .as_ref()
        .and_then(|h| h.get("timestamp"))
        .cloned()
        .unwrap_or(json!(null));

    let actions_taken = daemon_health
        .as_ref()
        .and_then(|h| h.get("actions_taken"))
        .cloned()
        .unwrap_or(json!(null));

    // --- 2. Recent cycle reports ---
    let state_root = resolve_state_root();
    let recent_cycles = read_recent_cycle_reports(&state_root, 10);

    // --- 3. Open PRs & assigned issues (concurrent) ---
    let (open_prs, assigned_issues) = tokio::join!(
        run_gh_json(&[
            "pr",
            "list",
            "--author",
            "@me",
            "--state",
            "open",
            "--json",
            "number,title,url,createdAt,headRefName",
        ]),
        run_gh_json(&[
            "issue",
            "list",
            "--assignee",
            "@me",
            "--state",
            "open",
            "--json",
            "number,title,url,labels",
        ])
    );

    Json(json!({
        "daemon": {
            "status": daemon_status,
            "current_cycle": current_cycle,
            "last_heartbeat": last_heartbeat,
            "actions_taken": actions_taken,
        },
        "recent_cycles": recent_cycles,
        "open_prs": open_prs,
        "assigned_issues": assigned_issues,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}
