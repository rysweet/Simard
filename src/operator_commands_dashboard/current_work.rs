use axum::Json;
use serde_json::{Value, json};

use super::dashboard_goal_board_snapshot;
use super::routes::{resolve_state_root, truncate_with_ellipsis};

/// Real-time snapshot of what Simard is doing right now.
///
/// Composes data from `daemon_health.json` (cycle/phase), the cognitive
/// memory `goal-board:snapshot` fact (active goals), and the agent
/// registry (spawned engineers).
pub(crate) async fn current_work() -> Json<Value> {
    let health_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
        .join("simard")
        .join("daemon_health.json");

    let daemon_health: Option<Value> = std::fs::read_to_string(&health_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    // Persistent cumulative cycle number (issue #1680): survives daemon
    // restarts so this endpoint reports the same "Cycle #N" as the Thinking
    // tab and Recent Actions rather than the process-local "#1" in
    // daemon_health.
    let cycle_number = super::cycle_source::authoritative_cycle_number(
        &resolve_state_root(),
        daemon_health.as_ref(),
    );

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

    // Active goals from the cognitive-memory goal-board snapshot.
    let state_root = resolve_state_root();
    let active_goals: Vec<Value> = dashboard_goal_board_snapshot(&state_root)
        .ok()
        .map(|board| {
            board
                .active
                .iter()
                .map(|g| {
                    let (status_str, blocker) = match &g.status {
                        crate::goal_curation::GoalProgress::Proposed => {
                            ("proposed".to_string(), None)
                        }
                        crate::goal_curation::GoalProgress::NotStarted => {
                            ("not_started".to_string(), None)
                        }
                        crate::goal_curation::GoalProgress::InProgress { percent } => {
                            (format!("in_progress({}%)", percent), None)
                        }
                        crate::goal_curation::GoalProgress::Blocked(reason) => {
                            ("blocked".to_string(), Some(reason.clone()))
                        }
                        crate::goal_curation::GoalProgress::Paused => ("paused".to_string(), None),
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

    // Active engineers — the TRUE live set (issue #2580). Previously this read
    // the `FileBackedAgentRegistry` (`agent_registry.json`), which nothing
    // writes in production, so the panel always showed ZERO even while the
    // daemon ran engineers. Now unions live subagent sessions with live
    // engineer-worktree claim sentinels (which also surface bare
    // `bin/simard engineer run single-process` subprocesses).
    let spawned_engineers: Vec<Value> = super::live_engineers::live_engineers(&state_root)
        .into_iter()
        .map(|e| {
            let started_at = e
                .started_at
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                .map(|dt| dt.to_rfc3339());
            json!({
                "id": e.goal_id,
                "pid": e.pid,
                "role": "engineer",
                "task": e.goal_id,
                "source": e.source,
                "alive": e.alive,
                "start_time": started_at,
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

    // Collect only (cycle_number, path) refs first — no file contents yet. The
    // `cycle_reports/` directory grows unbounded (one file per cycle, never
    // pruned), so reading every file's contents just to keep the newest `n`
    // wastes disk I/O and memory on hot, repeatedly-polled dashboard endpoints.
    //
    // Enumerating the directories and sorting the refs is still O(total files) —
    // the filesystem offers no cheaper way to find the newest entries — but that
    // work is light (a filename parse plus one small tuple per file). The costly
    // part is reading and JSON-parsing each report's full contents; deferring
    // those until we know which cycles survive drops them to O(n) in the common
    // case (a run of unreadable newest files can still force more read attempts).
    let mut refs: Vec<(u32, std::path::PathBuf)> = Vec::new();

    for dir in &candidates {
        if let Ok(listing) = std::fs::read_dir(dir) {
            for entry in listing.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Files are named cycle_<N>.json
                if let Some(num_str) = name
                    .strip_prefix("cycle_")
                    .and_then(|s| s.strip_suffix(".json"))
                    && let Ok(num) = num_str.parse::<u32>()
                {
                    refs.push((num, entry.path()));
                }
            }
        }
    }

    // Newest cycle first. The sort is stable, so for a duplicate cycle number
    // the first candidate directory keeps priority (same as before).
    refs.sort_by_key(|(num, _)| std::cmp::Reverse(*num));

    // Read contents only for the newest `n` distinct cycles, in order. For a
    // duplicated cycle number the first readable candidate wins; a number whose
    // only copies fail to read is skipped entirely (mirrors the previous
    // read-error handling, which never added unreadable files).
    let mut entries: Vec<(u32, String)> = Vec::with_capacity(n.min(refs.len()));
    let mut last_taken: Option<u32> = None;
    for (num, path) in refs {
        if entries.len() >= n {
            break;
        }
        // Already read this cycle number — skip the remaining duplicate copies.
        if last_taken == Some(num) {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            entries.push((num, contents));
            last_taken = Some(num);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- format_recent_actions_for_cycle -----------------------------------

    #[test]
    fn format_actions_from_outcomes() {
        let report = json!({
            "report": {
                "timestamp": "2026-01-01T00:00:00Z",
                "outcomes": [
                    {
                        "action_kind": "advance-goal",
                        "detail": "opened PR #42",
                        "action_description": "fallback desc"
                    }
                ]
            }
        });
        let actions = format_recent_actions_for_cycle(5, &report);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["cycle"], 5);
        assert_eq!(actions[0]["action"], "advance-goal");
        assert!(actions[0]["result"].as_str().unwrap().contains("PR #42"));
    }

    #[test]
    fn format_actions_prefers_detail_over_description() {
        let report = json!({
            "report": {
                "timestamp": "2026-01-01T00:00:00Z",
                "outcomes": [
                    {
                        "action_kind": "test",
                        "detail": "specific detail",
                        "action_description": "generic desc"
                    }
                ]
            }
        });
        let actions = format_recent_actions_for_cycle(1, &report);
        assert!(
            actions[0]["result"]
                .as_str()
                .unwrap()
                .contains("specific detail")
        );
    }

    #[test]
    fn format_actions_from_planned_when_no_outcomes() {
        let report = json!({
            "report": {
                "timestamp": "2026-01-01T00:00:00Z",
                "planned_actions": [
                    {"kind": "advance-goal", "description": "plan to fix bug"}
                ]
            }
        });
        let actions = format_recent_actions_for_cycle(3, &report);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["action"], "advance-goal");
        assert!(
            actions[0]["result"]
                .as_str()
                .unwrap()
                .contains("plan to fix bug")
        );
    }

    #[test]
    fn format_actions_from_summary_as_last_resort() {
        let report = json!({
            "summary": "Cycle completed with no actions"
        });
        let actions = format_recent_actions_for_cycle(7, &report);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["action"], "cycle-summary");
    }

    #[test]
    fn format_actions_empty_when_no_data() {
        let report = json!({});
        let actions = format_recent_actions_for_cycle(1, &report);
        assert!(actions.is_empty());
    }

    // ---- read_recent_cycle_reports ----------------------------------------

    #[test]
    fn reads_cycle_reports_sorted_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        std::fs::write(
            cycle_dir.join("cycle_1.json"),
            r#"{"cycle_number":1,"summary":"first"}"#,
        )
        .unwrap();
        std::fs::write(
            cycle_dir.join("cycle_3.json"),
            r#"{"cycle_number":3,"summary":"third"}"#,
        )
        .unwrap();
        std::fs::write(
            cycle_dir.join("cycle_2.json"),
            r#"{"cycle_number":2,"summary":"second"}"#,
        )
        .unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 10);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0]["cycle_number"], 3);
        assert_eq!(reports[1]["cycle_number"], 2);
        assert_eq!(reports[2]["cycle_number"], 1);
    }

    #[test]
    fn reads_cycle_reports_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        for i in 1..=5 {
            std::fs::write(
                cycle_dir.join(format!("cycle_{i}.json")),
                format!(r#"{{"cycle_number":{i}}}"#),
            )
            .unwrap();
        }

        let reports = read_recent_cycle_reports(dir.path(), 2);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0]["cycle_number"], 5);
        assert_eq!(reports[1]["cycle_number"], 4);
    }

    #[test]
    fn reads_cycle_reports_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let reports = read_recent_cycle_reports(dir.path(), 10);
        assert!(reports.is_empty());
    }

    #[test]
    fn reads_plain_text_reports_as_summary() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();
        std::fs::write(cycle_dir.join("cycle_1.json"), "Just plain text").unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 10);
        assert_eq!(reports.len(), 1);
        assert!(reports[0].get("summary").is_some());
    }

    // Locks in the semantics the lazy-read optimization must preserve: when the
    // directory holds far more files than `n` AND a cycle number is duplicated
    // across both candidate directories, only the newest `n` distinct cycles
    // are returned, and the first candidate directory wins the duplicate.
    #[test]
    fn reads_newest_n_and_first_dir_wins_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let dir_a = dir.path().join("cycle_reports");
        let dir_b = dir.path().join("state").join("cycle_reports");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        // 20 cycles in dir_a, far more than the requested 3.
        for i in 1..=20 {
            std::fs::write(
                dir_a.join(format!("cycle_{i}.json")),
                format!(r#"{{"cycle_number":{i},"src":"a"}}"#),
            )
            .unwrap();
        }
        // Duplicate the newest cycle in dir_b with different content.
        std::fs::write(
            dir_b.join("cycle_20.json"),
            r#"{"cycle_number":20,"src":"b"}"#,
        )
        .unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 3);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0]["cycle_number"], 20);
        assert_eq!(reports[1]["cycle_number"], 19);
        assert_eq!(reports[2]["cycle_number"], 18);
        // First candidate directory (dir_a) wins the duplicate cycle 20.
        assert_eq!(reports[0]["report"]["src"], "a");
    }
}
