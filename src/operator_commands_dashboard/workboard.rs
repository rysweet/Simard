use axum::Json;
use serde_json::{Value, json};

use super::routes::{is_pid_alive, read_recent_cycle_reports, resolve_state_root};
use crate::agent_registry::{AgentRegistry, FileBackedAgentRegistry};
use crate::goal_curation::GoalBoard;
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use super::routes::format_recent_actions_for_cycle;

// ---------------------------------------------------------------------------
// Workboard API — aggregated view of Simard's current mental state
// ---------------------------------------------------------------------------

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
        "number": cycle_number,
        "phase": cycle_phase,
        "started_at": started_at_str,
        "duration_ms": cycle_duration_ms,
    });

    // --- 2. Goals with enriched status ---
    let goal_path = state_root.join("goal_records.json");
    let goal_content = std::fs::read_to_string(&goal_path).unwrap_or_default();
    let goal_board = if goal_content.trim().is_empty() {
        None
    } else {
        match serde_json::from_str::<GoalBoard>(&goal_content) {
            Ok(b) => Some(b),
            Err(e) => {
                // Surface parse failures so the dashboard doesn't silently
                // render "no goals" when the file is malformed. Fail-open
                // returns None (same as before) but logs why.
                tracing::warn!(
                    path = %goal_path.display(),
                    error = %e,
                    "goal_records.json failed to parse; dashboard rendering 0 goals"
                );
                None
            }
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
                        crate::goal_curation::GoalProgress::NotStarted => {
                            ("queued".to_string(), 0u32)
                        }
                        crate::goal_curation::GoalProgress::InProgress { percent } => {
                            ("in_progress".to_string(), *percent)
                        }
                        crate::goal_curation::GoalProgress::Blocked(reason) => {
                            (format!("blocked: {reason}"), 0)
                        }
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

    // --- 3. Spawned engineers from agent registry ---
    let reg = FileBackedAgentRegistry::new(&state_root);
    let spawned_engineers: Vec<Value> = reg
        .list()
        .unwrap_or_default()
        .iter()
        .map(|e| {
            let alive = std::path::Path::new(&format!("/proc/{}", e.pid)).exists();
            json!({
                "pid": e.pid,
                "task": format!("{} ({})", e.role, e.id),
                "alive": alive,
                "state": format!("{:?}", e.state),
                "started_at": e.start_time.to_rfc3339(),
                "last_heartbeat": e.last_heartbeat.to_rfc3339(),
            })
        })
        .collect();

    // --- 4. Recent actions from cycle reports ---
    let recent_reports = read_recent_cycle_reports(&state_root, 5);
    let mut recent_actions: Vec<Value> = Vec::new();

    // Include current cycle's actions from daemon_health
    if let Some(actions) = daemon_health
        .as_ref()
        .and_then(|h| h.get("actions_taken"))
        .and_then(|v| v.as_str())
        && !actions.is_empty()
    {
        recent_actions.push(json!({
            "cycle": cycle_number,
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

    if let Ok(mem) = NativeCognitiveMemory::open_read_only(&state_root) {
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

        // Working memory slots for each active goal
        if let Some(board) = &goal_board {
            for goal in &board.active {
                if let Ok(slots) = mem.get_working(&goal.id) {
                    for slot in slots {
                        working_memory.push(json!({
                            "id": slot.node_id,
                            "slot_type": slot.slot_type,
                            "content": slot.content,
                            "task_id": slot.task_id,
                            "relevance": slot.relevance,
                        }));
                    }
                }
            }
        }

        // Recent semantic facts (search across common tags, collect up to 20)
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
                        recent_facts.push(json!({
                            "id": fact.node_id,
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
