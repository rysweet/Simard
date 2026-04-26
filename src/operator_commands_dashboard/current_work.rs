use axum::Json;
use serde_json::{Value, json};

use super::routes::{is_pid_alive, resolve_state_root, truncate_with_ellipsis};
use crate::agent_registry::{AgentRegistry, FileBackedAgentRegistry};
use crate::goal_curation::GoalBoard;
use crate::goals::GoalRecord;

/// Real-time snapshot of what Simard is doing right now.
///
/// Composes data from `daemon_health.json` (cycle/phase), `goal_records.json`
/// (active goals), and the agent registry (spawned engineers).
pub(crate) async fn current_work() -> Json<Value> {
    let health_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
        .join("simard")
        .join("daemon_health.json");

    let daemon_health: Option<Value> = std::fs::read_to_string(&health_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let cycle_number = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_number"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let cycle_phase = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_phase"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let cycle_phase_display = {
        let mut chars = cycle_phase.chars();
        match chars.next() {
            Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
            None => "Unknown".to_string(),
        }
    };

    let cycle_start_epoch = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_start_epoch"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let uptime_seconds = if cycle_start_epoch > 0 {
        now_epoch.saturating_sub(cycle_start_epoch)
    } else {
        0
    };

    let interval_secs = daemon_health
        .as_ref()
        .and_then(|h| h.get("interval_secs"))
        .and_then(|v| v.as_u64())
        .unwrap_or(300);

    let last_cycle_summary = daemon_health
        .as_ref()
        .and_then(|h| h.get("last_cycle_summary"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let cycle_duration_secs = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_duration_secs"))
        .and_then(|v| v.as_u64());

    let next_cycle_eta_seconds = if cycle_phase == "sleep" {
        if let Some(dur) = cycle_duration_secs {
            let next_start = cycle_start_epoch + dur + interval_secs;
            next_start.saturating_sub(now_epoch)
        } else {
            interval_secs
        }
    } else {
        0
    };

    // Active goals from goal_records.json
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let active_goals: Vec<Value> = std::fs::read_to_string(&goal_path)
        .ok()
        .and_then(|content| serde_json::from_str::<GoalBoard>(&content).ok())
        .map(|board| {
            board
                .active
                .iter()
                .map(|g| {
                    let (status_str, blocker) = match &g.status {
                        crate::goal_curation::GoalProgress::NotStarted => {
                            ("not_started".to_string(), None)
                        }
                        crate::goal_curation::GoalProgress::InProgress { percent } => {
                            (format!("in_progress({}%)", percent), None)
                        }
                        crate::goal_curation::GoalProgress::Blocked(reason) => {
                            ("blocked".to_string(), Some(reason.clone()))
                        }
                        crate::goal_curation::GoalProgress::Completed => {
                            ("completed".to_string(), None)
                        }
                    };
                    let mut goal_json = json!({
                        "name": g.id,
                        "description": g.description,
                        "status": status_str,
                        "priority": g.priority,
                    });
                    if let Some(b) = blocker {
                        goal_json["blocker"] = json!(b);
                    }
                    if let Some(ref assignee) = g.assigned_to {
                        goal_json["assigned_to"] = json!(assignee);
                    }
                    goal_json
                })
                .collect()
        })
        .unwrap_or_default();

    // Spawned engineers from agent registry
    let reg = FileBackedAgentRegistry::new(&state_root);
    let spawned_engineers: Vec<Value> = reg
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| {
            let alive = is_pid_alive(entry.pid);
            json!({
                "id": entry.id,
                "pid": entry.pid,
                "role": entry.role,
                "host": entry.host,
                "state": format!("{:?}", entry.state),
                "alive": alive,
                "start_time": entry.start_time.to_rfc3339(),
                "last_heartbeat": entry.last_heartbeat.to_rfc3339(),
            })
        })
        .collect();

    Json(json!({
        "cycle_number": cycle_number,
        "cycle_phase": cycle_phase_display,
        "uptime_seconds": uptime_seconds,
        "active_goals": active_goals,
        "spawned_engineers": spawned_engineers,
        "last_cycle_summary": last_cycle_summary,
        "next_cycle_eta_seconds": next_cycle_eta_seconds,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Builds the entries for the dashboard's "Recent Actions" panel from a
/// single cycle report. Prefers `outcomes[].detail` (informative — e.g.
/// "spawn_engineer dispatched: agent='…', task='…'"), truncated to 200
/// characters with an ellipsis on overflow. Falls back to
/// `outcomes[].action_description`, then to `planned_actions[].description`
/// for older cycles that have no outcomes recorded, then to the cycle
/// summary as a last resort.
pub(crate) fn format_recent_actions_for_cycle(cycle_num: u64, report: &Value) -> Vec<Value> {
    const MAX_LEN: usize = 200;

    // The wrapper from `read_recent_cycle_reports` may put the parsed cycle
    // JSON under `report` (parsed) or expose `summary` directly (plain text).
    let rpt = report.get("report").unwrap_or(report);
    let timestamp = rpt
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let mut out: Vec<Value> = Vec::new();

    if let Some(outcomes) = rpt.get("outcomes").and_then(|v| v.as_array())
        && !outcomes.is_empty()
    {
        for o in outcomes {
            let kind = o
                .get("action_kind")
                .and_then(|v| v.as_str())
                .unwrap_or("action");
            let raw = o
                .get("detail")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| o.get("action_description").and_then(|v| v.as_str()))
                .unwrap_or("(no detail)");
            out.push(json!({
                "cycle": cycle_num,
                "action": kind,
                "target": "",
                "result": truncate_with_ellipsis(raw, MAX_LEN),
                "at": timestamp,
            }));
        }
        return out;
    }

    if let Some(planned) = rpt.get("planned_actions").and_then(|v| v.as_array())
        && !planned.is_empty()
    {
        for a in planned {
            let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("action");
            let desc = a
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("(no description)");
            out.push(json!({
                "cycle": cycle_num,
                "action": kind,
                "target": "",
                "result": truncate_with_ellipsis(desc, MAX_LEN),
                "at": timestamp,
            }));
        }
        return out;
    }

    // Last resort: cycle summary text (top-level for plain-text reports,
    // nested for parsed JSON reports).
    let summary = report
        .get("summary")
        .and_then(|v| v.as_str())
        .or_else(|| rpt.get("summary").and_then(|v| v.as_str()))
        .or_else(|| rpt.get("actions_taken").and_then(|v| v.as_str()))
        .unwrap_or("");
    if !summary.is_empty() {
        out.push(json!({
            "cycle": cycle_num,
            "action": "cycle-summary",
            "target": "",
            "result": truncate_with_ellipsis(summary, MAX_LEN),
            "at": timestamp,
        }));
    }
    out
}

pub(crate) fn read_recent_cycle_reports(state_root: &std::path::Path, n: usize) -> Vec<Value> {
    // The daemon writes to `state_root/state/cycle_reports/` while
    // resolve_state_root() may return the parent. Check both locations.
    let candidates = [
        state_root.join("cycle_reports"),
        state_root.join("state").join("cycle_reports"),
    ];

    let mut entries: Vec<(u32, String)> = Vec::new();

    for dir in &candidates {
        if let Ok(listing) = std::fs::read_dir(dir) {
            for entry in listing.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Files are named cycle_<N>.json
                if let Some(num_str) = name
                    .strip_prefix("cycle_")
                    .and_then(|s| s.strip_suffix(".json"))
                    && let Ok(num) = num_str.parse::<u32>()
                    && let Ok(contents) = std::fs::read_to_string(entry.path())
                {
                    entries.push((num, contents));
                }
            }
        }
    }

    // Deduplicate by cycle number (prefer higher-numbered path if duplicates exist)
    entries.sort_by_key(|b| std::cmp::Reverse(b.0));
    entries.dedup_by_key(|e| e.0);
    entries.truncate(n);

    entries
        .into_iter()
        .map(|(num, summary)| {
            // Try parsing as JSON first; if it's plain text, wrap it.
            match serde_json::from_str::<Value>(&summary) {
                Ok(v) => json!({"cycle_number": num, "report": v}),
                Err(_) => json!({"cycle_number": num, "summary": summary}),
            }
        })
        .collect()
}
