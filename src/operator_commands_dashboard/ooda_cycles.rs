//! `/api/ooda-cycles` endpoint — per-cycle history with duration trend
//! (issue #2135).
//!
//! Returns the last N OODA cycles from `cycle_reports/cycle_*.json` with:
//!   - cycle_number, phase (final phase of the cycle), duration_secs,
//!     actions_taken, summary, timestamp.
//!
//! This lets the dashboard render a time-series of cycle durations and
//! actions so Simard-as-reader can answer: "Are my cycles getting faster
//! or slower?" and "Did my last cycle improve things?"

use axum::Json;
use serde_json::{Value, json};

use super::current_work::read_recent_cycle_reports;
use super::routes::resolve_state_root;

/// Maximum number of cycles to return (most recent first).
const MAX_CYCLES: usize = 50;

pub(crate) async fn ooda_cycles() -> Json<Value> {
    let state_root = resolve_state_root();
    let raw_reports = read_recent_cycle_reports(&state_root, MAX_CYCLES);

    let mut cycles: Vec<Value> = Vec::new();
    let mut durations: Vec<f64> = Vec::new();

    for entry in &raw_reports {
        let cycle_number = entry
            .get("cycle_number")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // The entry is either {cycle_number, report: {...}} or {cycle_number, summary: "..."}
        let report = entry.get("report");

        let timestamp = report
            .and_then(|r| r.get("timestamp"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let duration_secs = report
            .and_then(|r| r.get("duration_secs"))
            .and_then(|v| v.as_f64())
            .or_else(|| {
                report
                    .and_then(|r| r.get("cycle_duration_secs"))
                    .and_then(|v| v.as_f64())
            });

        let actions_taken = extract_actions_taken(report);

        let summary = report
            .and_then(|r| r.get("summary"))
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("summary").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        // Determine the final phase from outcomes or planned_actions
        let phase = extract_final_phase(report);

        let action_count = actions_taken.len();

        if let Some(d) = duration_secs {
            durations.push(d);
        }

        cycles.push(json!({
            "cycle_number": cycle_number,
            "phase": phase,
            "duration_secs": duration_secs,
            "actions_taken": actions_taken,
            "action_count": action_count,
            "summary": summary,
            "timestamp": timestamp,
        }));
    }

    // Compute trend: average of first half vs second half of durations.
    let trend = compute_duration_trend(&durations);

    Json(json!({
        "cycles": cycles,
        "total_cycles": cycles.len(),
        "duration_trend": trend,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Extract a list of action descriptions from a cycle report.
fn extract_actions_taken(report: Option<&Value>) -> Vec<String> {
    let mut actions = Vec::new();
    let report = match report {
        Some(r) => r,
        None => return actions,
    };

    // Prefer outcomes (completed actions)
    if let Some(outcomes) = report.get("outcomes").and_then(|v| v.as_array()) {
        for o in outcomes {
            let kind = o
                .get("action_kind")
                .and_then(|v| v.as_str())
                .unwrap_or("action");
            let desc = o
                .get("action_description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let success = o.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
            let status = if success { "ok" } else { "failed" };
            actions.push(format!("{kind} [{status}]: {desc}"));
        }
        return actions;
    }

    // Fall back to planned_actions
    if let Some(planned) = report.get("planned_actions").and_then(|v| v.as_array()) {
        for a in planned {
            let kind = a.get("kind").and_then(|v| v.as_str()).unwrap_or("action");
            let desc = a.get("description").and_then(|v| v.as_str()).unwrap_or("");
            actions.push(format!("{kind}: {desc}"));
        }
    }

    actions
}

/// Determine the final phase reached in a cycle from the report structure.
fn extract_final_phase(report: Option<&Value>) -> String {
    let report = match report {
        Some(r) => r,
        None => return "unknown".to_string(),
    };

    if report
        .get("outcomes")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        == Some(true)
    {
        return "act".to_string();
    }
    if report
        .get("planned_actions")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        == Some(true)
    {
        return "decide".to_string();
    }
    if report
        .get("priorities")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        == Some(true)
    {
        return "orient".to_string();
    }
    if report.get("observation").is_some() {
        return "observe".to_string();
    }

    "unknown".to_string()
}

/// Compute a simple duration trend: "improving" if the second half is
/// faster, "degrading" if slower, "stable" if within 10%, or "insufficient"
/// if fewer than 4 data points.
fn compute_duration_trend(durations: &[f64]) -> Value {
    if durations.len() < 4 {
        return json!({
            "direction": "insufficient_data",
            "detail": "Need at least 4 cycles with duration data to compute trend",
        });
    }

    let mid = durations.len() / 2;
    // durations is newest-first, so second half = older cycles
    let recent_avg: f64 = durations[..mid].iter().sum::<f64>() / mid as f64;
    let older_avg: f64 = durations[mid..].iter().sum::<f64>() / (durations.len() - mid) as f64;

    if older_avg == 0.0 {
        return json!({
            "direction": "insufficient_data",
            "detail": "Older cycles have zero duration",
        });
    }

    let change_pct = ((recent_avg - older_avg) / older_avg) * 100.0;

    let direction = if change_pct < -10.0 {
        "improving"
    } else if change_pct > 10.0 {
        "degrading"
    } else {
        "stable"
    };

    json!({
        "direction": direction,
        "recent_avg_secs": (recent_avg * 10.0).round() / 10.0,
        "older_avg_secs": (older_avg * 10.0).round() / 10.0,
        "change_pct": (change_pct * 10.0).round() / 10.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_final_phase_from_outcomes() {
        let report = json!({
            "outcomes": [{"action_kind": "advance-goal", "success": true}],
        });
        assert_eq!(extract_final_phase(Some(&report)), "act");
    }

    #[test]
    fn extract_final_phase_from_planned_actions() {
        let report = json!({
            "planned_actions": [{"kind": "advance-goal", "description": "test"}],
        });
        assert_eq!(extract_final_phase(Some(&report)), "decide");
    }

    #[test]
    fn extract_final_phase_from_priorities() {
        let report = json!({
            "priorities": [{"goal_id": "g1", "urgency": 0.8}],
        });
        assert_eq!(extract_final_phase(Some(&report)), "orient");
    }

    #[test]
    fn extract_final_phase_from_observation() {
        let report = json!({
            "observation": {"goal_count": 3},
        });
        assert_eq!(extract_final_phase(Some(&report)), "observe");
    }

    #[test]
    fn extract_final_phase_none_returns_unknown() {
        assert_eq!(extract_final_phase(None), "unknown");
    }

    #[test]
    fn extract_final_phase_empty_report() {
        let report = json!({});
        assert_eq!(extract_final_phase(Some(&report)), "unknown");
    }

    #[test]
    fn extract_actions_taken_from_outcomes() {
        let report = json!({
            "outcomes": [
                {"action_kind": "advance-goal", "action_description": "opened PR", "success": true},
                {"action_kind": "consolidate-memory", "action_description": "merged 10 facts", "success": false},
            ],
        });
        let actions = extract_actions_taken(Some(&report));
        assert_eq!(actions.len(), 2);
        assert!(actions[0].contains("advance-goal"));
        assert!(actions[0].contains("[ok]"));
        assert!(actions[1].contains("[failed]"));
    }

    #[test]
    fn extract_actions_taken_from_planned() {
        let report = json!({
            "planned_actions": [{"kind": "advance-goal", "description": "do thing"}],
        });
        let actions = extract_actions_taken(Some(&report));
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("advance-goal"));
    }

    #[test]
    fn extract_actions_taken_none() {
        assert!(extract_actions_taken(None).is_empty());
    }

    #[test]
    fn compute_trend_insufficient_data() {
        let trend = compute_duration_trend(&[1.0, 2.0]);
        assert_eq!(
            trend.get("direction").and_then(|v| v.as_str()),
            Some("insufficient_data")
        );
    }

    #[test]
    fn compute_trend_improving() {
        // newer cycles are faster (smaller duration)
        let durations = vec![10.0, 12.0, 20.0, 25.0];
        let trend = compute_duration_trend(&durations);
        assert_eq!(
            trend.get("direction").and_then(|v| v.as_str()),
            Some("improving")
        );
    }

    #[test]
    fn compute_trend_degrading() {
        // newer cycles are slower
        let durations = vec![25.0, 30.0, 10.0, 12.0];
        let trend = compute_duration_trend(&durations);
        assert_eq!(
            trend.get("direction").and_then(|v| v.as_str()),
            Some("degrading")
        );
    }

    #[test]
    fn compute_trend_stable() {
        let durations = vec![10.0, 10.5, 10.2, 10.3];
        let trend = compute_duration_trend(&durations);
        assert_eq!(
            trend.get("direction").and_then(|v| v.as_str()),
            Some("stable")
        );
    }

    #[test]
    fn ooda_cycles_reads_from_disk() {
        // Integration test: create temp cycle reports and verify the handler reads them.
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        let report1 = json!({
            "cycle_number": 1,
            "timestamp": "2026-05-27T10:00:00Z",
            "duration_secs": 120.0,
            "summary": "First cycle",
            "observation": {"goal_count": 2},
            "priorities": [{"goal_id": "g1", "urgency": 0.8, "reason": "test"}],
            "planned_actions": [{"kind": "advance-goal", "description": "test", "goal_id": "g1"}],
            "outcomes": [{"action_kind": "advance-goal", "action_description": "opened PR", "success": true}],
        });
        std::fs::write(
            cycle_dir.join("cycle_1.json"),
            serde_json::to_string(&report1).unwrap(),
        )
        .unwrap();

        let report2 = json!({
            "cycle_number": 2,
            "timestamp": "2026-05-27T10:10:00Z",
            "duration_secs": 90.0,
            "summary": "Second cycle",
            "outcomes": [{"action_kind": "consolidate-memory", "action_description": "merged facts", "success": true}],
        });
        std::fs::write(
            cycle_dir.join("cycle_2.json"),
            serde_json::to_string(&report2).unwrap(),
        )
        .unwrap();

        // read_recent_cycle_reports should find both
        let reports = read_recent_cycle_reports(dir.path(), 50);
        assert_eq!(reports.len(), 2);

        // Verify newest first
        let first_num = reports[0]
            .get("cycle_number")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(first_num, 2);
    }
}
