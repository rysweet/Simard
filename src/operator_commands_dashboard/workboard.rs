use axum::Json;
use serde_json::{Value, json};

use super::current_work::format_recent_actions_for_cycle;
use super::current_work::read_recent_cycle_reports;
use super::cycle_source;
use super::dashboard_goal_board_snapshot;
use super::routes::resolve_state_root;
use crate::memory_ipc::open_reader_bridge;

// ---------------------------------------------------------------------------
// Workboard API — aggregated view of Simard's current mental state
// ---------------------------------------------------------------------------

/// Human-readable label for a working memory slot type (#1683).
fn human_slot_type(raw: &str) -> &'static str {
    match raw {
        "context" | "Context" => "Task context",
        "plan" | "Plan" => "Current plan",
        "observation" | "Observation" => "Observation",
        "hypothesis" | "Hypothesis" => "Hypothesis",
        "decision" | "Decision" => "Decision",
        "action" | "Action" => "Action taken",
        "result" | "Result" => "Result",
        "goal" | "Goal" => "Goal detail",
        _ => "Note",
    }
}

/// Human-readable category for a fact concept string (#1683).
fn human_fact_category(concept: &str) -> &'static str {
    let c = concept.to_lowercase();
    if c.contains("goal") || c.contains("objective") {
        "Goal"
    } else if c.contains("action") || c.contains("task") {
        "Action"
    } else if c.contains("decision") {
        "Decision"
    } else if c.contains("episode") || c.contains("event") {
        "Event"
    } else if c.contains("observation") || c.contains("insight") {
        "Insight"
    } else if c.contains("meeting") || c.contains("discussion") {
        "Meeting note"
    } else if c.contains("snapshot") {
        "Snapshot"
    } else {
        "Fact"
    }
}

pub(crate) async fn workboard() -> Json<Value> {
    let state_root = resolve_state_root();

    // --- 1. Daemon health → cycle info ---
    let health_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
        .join("simard")
        .join("daemon_health.json");

    let daemon_health: Option<Value> = std::fs::read_to_string(&health_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let cycle_number = cycle_source::health_cycle_number(daemon_health.as_ref());

    // The persistent, cross-restart cycle number shown in every "Cycle #N"
    // widget. `cycle_number` above is process-local (resets to 1 on daemon
    // restart) and is kept only for the uptime heuristic below; the displayed
    // counter must agree with the Thinking tab and Recent Actions (#1680).
    let display_cycle_number =
        cycle_source::authoritative_cycle_number(&state_root, daemon_health.as_ref());

    let cycle_phase = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_phase"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let cycle_start_epoch = daemon_health
        .as_ref()
        .and_then(|h| h.get("cycle_start_epoch"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let interval_secs = daemon_health
        .as_ref()
        .and_then(|h| h.get("interval_secs"))
        .and_then(|v| v.as_u64())
        .unwrap_or(300);

    let health_timestamp = daemon_health
        .as_ref()
        .and_then(|h| h.get("timestamp"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let cycle_duration_ms = if cycle_start_epoch > 0 {
        (now_epoch.saturating_sub(cycle_start_epoch)) * 1000
    } else {
        0
    };

    // ETA: if sleeping, estimate time remaining until next cycle
    let next_cycle_eta_seconds = if cycle_phase == "sleep" {
        let cycle_dur = daemon_health
            .as_ref()
            .and_then(|h| h.get("cycle_duration_secs"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cycle_end = cycle_start_epoch + cycle_dur;
        let next_start = cycle_end + interval_secs;
        next_start.saturating_sub(now_epoch)
    } else {
        0
    };

    let uptime_seconds = if !health_timestamp.is_empty() {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&health_timestamp) {
            let age = chrono::Utc::now().signed_duration_since(ts);
            (cycle_number * interval_secs).max(age.num_seconds().unsigned_abs())
        } else {
            cycle_number * interval_secs
        }
    } else {
        0
    };

    let started_at_str = if cycle_start_epoch > 0 {
        chrono::DateTime::from_timestamp(cycle_start_epoch as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let cycle_info = json!({
        "number": display_cycle_number,
        "phase": cycle_phase,
        "started_at": started_at_str,
        "duration_ms": cycle_duration_ms,
    });

    // --- 2. Goals with enriched status ---
    let goal_board = match dashboard_goal_board_snapshot(&state_root) {
        Ok(b) => Some(b),
        Err(e) => {
            // Surface bridge failures so the dashboard doesn't silently
            // render "no goals" when cognitive memory is unreachable.
            // Fail-open returns None (same as before) but logs why.
            tracing::warn!(
                error = %e,
                "cognitive-memory goal-board snapshot unavailable; dashboard rendering 0 goals"
            );
            None
        }
    };

    let goals_json: Vec<Value> = goal_board
        .as_ref()
        .map(|board| {
            board
                .active
                .iter()
                .map(|g| {
                    let (status_str, progress_pct) = match &g.status {
                        crate::goal_curation::GoalProgress::Proposed => {
                            ("proposed".to_string(), 0u32)
                        }
                        crate::goal_curation::GoalProgress::NotStarted => {
                            ("queued".to_string(), 0u32)
                        }
                        crate::goal_curation::GoalProgress::InProgress { percent } => {
                            ("in_progress".to_string(), *percent)
                        }
                        crate::goal_curation::GoalProgress::Blocked(reason) => {
                            (format!("blocked: {reason}"), 0)
                        }
                        crate::goal_curation::GoalProgress::Paused => ("paused".to_string(), 0),
                        crate::goal_curation::GoalProgress::Completed => ("done".to_string(), 100),
                    };
                    json!({
                        "name": g.id,
                        "description": g.description,
                        "status": status_str,
                        "progress_pct": progress_pct,
                        "priority": g.priority,
                        "assigned_to": g.assigned_to,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // --- 3. Active engineers from the live subagent-session registry ---
    // Read from the same source the Terminal tab uses
    // (`/api/subagent-sessions`) so the Whiteboard's "Active Engineers" panel
    // agrees with it. The agent registry used previously is empty for the
    // running daemon, which produced the "No spawned engineers" contradiction
    // (#1678). "Live" == no recorded end time, matching the Terminal tab.
    let mut sessions: Vec<crate::subagent_sessions::SubagentSession> =
        crate::subagent_sessions::load()
            .sessions
            .into_iter()
            .filter(|s| s.ended_at.is_none())
            .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    let mut spawned_engineers: Vec<Value> = sessions
        .iter()
        .map(|s| {
            let alive = super::routes::is_pid_alive(s.pid);
            let started_at = chrono::DateTime::from_timestamp(s.created_at, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            let task = if !s.goal_id.is_empty() {
                s.goal_id.clone()
            } else {
                s.session_name.clone()
            };
            json!({
                "pid": s.pid,
                "task": task,
                "alive": alive,
                "host": s.host,
                "started_at": started_at,
            })
        })
        .collect();

    // --- 3b. Union live worktree dispatch claims (#2432, the TRUE live set) ---
    // Root-cause fix for the "ZERO active engineers" defect: on a cold-start or
    // after a daemon restart the subagent registry can be empty/stale while
    // engineers are genuinely in-flight as worktree dispatch claims (a live PID
    // holding a `.simard-engineer-claim` sentinel). Union those live claims —
    // the single source of truth (design G4) — so the gauge reflects the real
    // live engineer set and can never render 0 while an engineer is running.
    // Dedup by PID so a claim already surfaced as a subagent session above is
    // not double-counted.
    let mut seen_pids: std::collections::HashSet<i64> = spawned_engineers
        .iter()
        .filter_map(|e| e["pid"].as_i64())
        .collect();
    for claim in crate::ooda_brain::live_engineer_claims(&state_root) {
        if !seen_pids.insert(claim.pid as i64) {
            continue;
        }
        spawned_engineers.push(json!({
            "pid": claim.pid,
            "task": claim.worktree_name,
            "alive": true,
            "host": "local",
            "started_at": "",
        }));
    }

    // --- 4. Recent actions from cycle reports ---
    let recent_reports = read_recent_cycle_reports(&state_root, 5);
    let mut recent_actions: Vec<Value> = Vec::new();

    // Include the current cycle's in-flight action from daemon_health. Skip
    // the daemon's "Starting cycle #N" placeholder: it carries no action
    // detail and embeds the process-local counter, which would re-introduce
    // the #1680 contradiction (e.g. "#1159 · Starting cycle #6") right next to
    // the persistent cycle number used everywhere else.
    if let Some(actions) = daemon_health
        .as_ref()
        .and_then(|h| h.get("actions_taken"))
        .and_then(|v| v.as_str())
        && !actions.is_empty()
        && !actions.starts_with("Starting cycle #")
    {
        recent_actions.push(json!({
            "cycle": display_cycle_number,
            "action": "current",
            "target": "",
            "result": actions,
            "at": health_timestamp,
        }));
    }

    for report in &recent_reports {
        let cycle_num = report
            .get("cycle_number")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        recent_actions.extend(format_recent_actions_for_cycle(cycle_num, report));
    }
    recent_actions.truncate(10);

    // --- 5. Task memory from cognitive memory ---
    let mut facts_count = 0u64;
    let mut recent_facts: Vec<Value> = Vec::new();
    let mut working_memory: Vec<Value> = Vec::new();
    let mut cognitive_stats: Option<Value> = None;

    if let Ok(reader) = open_reader_bridge(&state_root) {
        let mem = reader.ops();
        // Cognitive statistics
        if let Ok(stats) = mem.get_statistics() {
            facts_count = stats.semantic_count;
            cognitive_stats = Some(json!({
                "sensory_count": stats.sensory_count,
                "working_count": stats.working_count,
                "episodic_count": stats.episodic_count,
                "semantic_count": stats.semantic_count,
                "procedural_count": stats.procedural_count,
                "prospective_count": stats.prospective_count,
                "total": stats.total(),
            }));
        }

        // Working memory slots for each active goal (#1683: human-readable labels)
        if let Some(board) = &goal_board {
            for goal in &board.active {
                if let Ok(slots) = mem.get_working(&goal.id) {
                    for slot in slots {
                        let type_label = human_slot_type(&slot.slot_type);
                        let goal_label = &goal.description;
                        let relevance_label = if slot.relevance >= 0.8 {
                            "High"
                        } else if slot.relevance >= 0.5 {
                            "Medium"
                        } else {
                            "Low"
                        };
                        working_memory.push(json!({
                            "type_label": type_label,
                            "content": slot.content,
                            "goal": goal_label,
                            "relevance_label": relevance_label,
                            "relevance": slot.relevance,
                        }));
                    }
                }
            }
        }

        // Recent semantic facts (search across common tags, collect up to 20)
        // (#1683: provide human-readable category labels instead of raw IDs)
        for tag in &[
            "action",
            "goal",
            "decision",
            "episode",
            "observation",
            "insight",
        ] {
            if let Ok(facts) = mem.search_facts(tag, 10, 0.0) {
                for fact in facts {
                    if recent_facts.len() < 20 {
                        let category = human_fact_category(&fact.concept);
                        recent_facts.push(json!({
                            "category": category,
                            "concept": fact.concept,
                            "content": fact.content,
                            "confidence": fact.confidence,
                            "tags": fact.tags,
                        }));
                    }
                }
            }
        }
    }

    Json(json!({
        "cycle": cycle_info,
        "uptime_seconds": uptime_seconds,
        "next_cycle_eta_seconds": next_cycle_eta_seconds,
        "goals": goals_json,
        "spawned_engineers": spawned_engineers,
        "recent_actions": recent_actions,
        "working_memory": working_memory,
        "task_memory": {
            "facts_count": facts_count,
            "recent_facts": recent_facts,
        },
        "cognitive_statistics": cognitive_stats,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}
