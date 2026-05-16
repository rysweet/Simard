use axum::Json;
use axum::extract::Path;
use serde_json::{Value, json};

use super::goals_status::render_status_and_detail;
use super::routes::resolve_state_root;
use super::{dashboard_goal_board_snapshot, dashboard_save_goal_board};
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};
use crate::goals::goal_slug;

/// Load the dashboard's view of the goal board from cognitive memory.
/// Returns an empty `GoalBoard` when the snapshot is missing or the bridge
/// cannot be opened — the dashboard always renders rather than 500ing.
fn load_board_or_empty() -> GoalBoard {
    let state_root = resolve_state_root();
    dashboard_goal_board_snapshot(&state_root).unwrap_or_default()
}

pub(crate) async fn goals() -> Json<Value> {
    let state_root = resolve_state_root();
    let board = dashboard_goal_board_snapshot(&state_root).unwrap_or_default();

    let active: Vec<Value> = board
        .active
        .into_iter()
        .map(|g| {
            // Issue #1684: render the raw brain-log `current_activity` string
            // into a plain-English `status_chip` + `detail` pair, plus the
            // unredacted `detail_full` for click-to-expand. `current_activity`
            // is kept as-is (alias) so existing consumers do not break.
            let (chip, detail, detail_full) =
                render_status_and_detail(g.current_activity.as_deref());
            json!({
                "id": g.id,
                "description": g.description,
                "priority": g.priority,
                "status": g.status.to_string(),
                "assigned_to": g.assigned_to,
                "current_activity": g.current_activity,
                "status_chip": chip.as_str(),
                "detail": detail,
                "detail_full": detail_full,
                "wip_refs": g.wip_refs,
            })
        })
        .collect();

    let mut backlog: Vec<Value> = board
        .backlog
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "description": g.description,
                "source": g.source,
                "score": g.score,
            })
        })
        .collect();

    // Pull meeting-captured actions and decisions from cognitive memory (#415)
    if let Ok(mem) = NativeCognitiveMemory::open_read_only(&state_root) {
        for tag in &["goal", "action", "decision"] {
            if let Ok(facts) = mem.search_facts(tag, 20, 0.0) {
                for fact in facts {
                    // Skip goal-board snapshots — they contain the entire
                    // serialized GoalBoard, not individual backlog items.
                    if fact.concept.contains("snapshot") || fact.concept.contains("goal-board") {
                        continue;
                    }
                    // Skip facts whose content looks like serialized JSON objects
                    let trimmed = fact.content.trim();
                    if trimmed.starts_with('{') || trimmed.starts_with('[') {
                        continue;
                    }
                    let already_listed = active
                        .iter()
                        .chain(backlog.iter())
                        .any(|g| g.get("id").and_then(|v| v.as_str()) == Some(&fact.node_id));
                    if !already_listed {
                        backlog.push(json!({
                            "id": fact.node_id,
                            "description": fact.content,
                            "source": format!("cognitive-memory/{}", fact.concept),
                            "score": fact.confidence,
                        }));
                    }
                }
            }
        }
    }

    Json(json!({
        "active": active,
        "backlog": backlog,
        "active_count": active.len(),
        "backlog_count": backlog.len(),
    }))
}

pub(crate) async fn seed_goals() -> Json<Value> {
    let state_root = resolve_state_root();
    let existing = dashboard_goal_board_snapshot(&state_root).unwrap_or_default();
    if !existing.active.is_empty() {
        return Json(json!({"status": "already_seeded", "message": "Goals already exist"}));
    }

    let mut board = GoalBoard::new();
    let now = chrono::Utc::now().to_rfc3339();
    board.active.push(ActiveGoal {
        id: "self-improvement".to_string(),
        description:
            "Continuously improve own capabilities through gym scenarios and self-evaluation"
                .to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 0 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some(format!("Goal seeded via dashboard at {now}")),
        wip_refs: vec![],
    });
    board.active.push(ActiveGoal {
        id: "knowledge-growth".to_string(),
        description:
            "Expand knowledge base through meetings, research, and cognitive memory consolidation"
                .to_string(),
        priority: 2,
        status: GoalProgress::InProgress { percent: 0 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some(format!("Goal seeded via dashboard at {now}")),
        wip_refs: vec![],
    });
    board.active.push(ActiveGoal {
        id: "operational-health".to_string(),
        description: "Maintain system health: budget compliance, resource usage, and error rates within thresholds".to_string(),
        priority: 3,
        status: GoalProgress::InProgress { percent: 0 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some(format!("Goal seeded via dashboard at {now}")),
        wip_refs: vec![],
    });
    board.backlog.push(BacklogItem {
        id: "distributed-sync".to_string(),
        description: "Establish hive mind sync with remote Simard instances for cross-agent knowledge sharing".to_string(),
        source: "dashboard-seed".to_string(),
        score: 0.7,
    });
    board.backlog.push(BacklogItem {
        id: "meeting-quality".to_string(),
        description: "Improve meeting facilitation quality and actionable outcome generation"
            .to_string(),
        source: "dashboard-seed".to_string(),
        score: 0.6,
    });

    match dashboard_save_goal_board(&state_root, &board) {
        Ok(()) => {
            Json(json!({"status": "ok", "message": "Seeded 3 active goals and 2 backlog items"}))
        }
        Err(e) => Json(json!({"status": "error", "error": format!("save failed: {e}")})),
    }
}

pub(crate) async fn add_goal(Json(body): Json<Value>) -> Json<Value> {
    let state_root = resolve_state_root();
    let mut board = load_board_or_empty();

    let desc = match body.get("description").and_then(|v| v.as_str()) {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => return Json(json!({"error": "description is required"})),
    };

    let goal_type = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("active");
    let id = goal_slug(&desc);

    if goal_type == "backlog" {
        let score = body.get("score").and_then(|v| v.as_f64()).unwrap_or(0.5);
        board.backlog.push(BacklogItem {
            id: id.clone(),
            description: desc,
            source: "dashboard".to_string(),
            score,
        });
    } else {
        if board.active.len() >= MAX_ACTIVE_GOALS {
            return Json(json!({"error": format!(
                "Maximum {} active goals reached. Remove one first or add to backlog.",
                MAX_ACTIVE_GOALS
            )}));
        }
        let priority = body.get("priority").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
        board.active.push(ActiveGoal {
            id: id.clone(),
            description: desc,
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
        });
    }

    match dashboard_save_goal_board(&state_root, &board) {
        Ok(()) => Json(json!({"status": "ok", "id": id})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn remove_goal(Path(id): Path<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let mut board = load_board_or_empty();

    let before_active = board.active.len();
    let before_backlog = board.backlog.len();
    board.active.retain(|g| g.id != id);
    board.backlog.retain(|g| g.id != id);

    if board.active.len() == before_active && board.backlog.len() == before_backlog {
        return Json(json!({"error": "goal not found"}));
    }

    match dashboard_save_goal_board(&state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn update_goal_status(
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let state_root = resolve_state_root();
    let mut board = load_board_or_empty();

    let status_str = match body.get("status").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Json(json!({"error": "status is required"})),
    };

    let new_status = match status_str {
        "not-started" => GoalProgress::NotStarted,
        "in-progress" => GoalProgress::InProgress { percent: 0 },
        "blocked" => {
            let reason = body
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unspecified")
                .to_string();
            GoalProgress::Blocked(reason)
        }
        "completed" => GoalProgress::Completed,
        other => return Json(json!({"error": format!("unknown status: {other}")})),
    };

    let mut found = false;
    for goal in &mut board.active {
        if goal.id == id {
            goal.status = new_status.clone();
            found = true;
            break;
        }
    }

    if !found {
        return Json(json!({"error": "goal not found in active goals"}));
    }

    match dashboard_save_goal_board(&state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn promote_backlog_item(Path(id): Path<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let mut board = load_board_or_empty();

    if board.active.len() >= MAX_ACTIVE_GOALS {
        return Json(json!({"error": format!(
            "Maximum {} active goals reached. Remove one first.",
            MAX_ACTIVE_GOALS
        )}));
    }

    let pos = board.backlog.iter().position(|g| g.id == id);
    let item = match pos {
        Some(i) => board.backlog.remove(i),
        None => return Json(json!({"error": "backlog item not found"})),
    };

    board.active.push(ActiveGoal {
        id: item.id,
        description: item.description,
        priority: 3,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });

    match dashboard_save_goal_board(&state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn demote_goal(Path(id): Path<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let mut board = load_board_or_empty();

    let pos = board.active.iter().position(|g| g.id == id);
    let goal = match pos {
        Some(i) => board.active.remove(i),
        None => return Json(json!({"error": "active goal not found"})),
    };

    board.backlog.push(BacklogItem {
        id: goal.id,
        description: goal.description,
        source: "demoted".to_string(),
        score: 0.0,
    });

    match dashboard_save_goal_board(&state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}
