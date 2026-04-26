use axum::Json;
use axum::extract::Path;
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory, as_f64, as_i64, as_str};
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};
use crate::goals::{GoalRecord, goal_slug};

pub(crate) async fn goals() -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let content = std::fs::read_to_string(&goal_path).unwrap_or_default();

    let (active, mut backlog) = match serde_json::from_str::<GoalBoard>(&content) {
        // GoalBoard from OODA loop — already has the right schema.
        Ok(board) => {
            let a: Vec<Value> = board
                .active
                .into_iter()
                .map(|g| {
                    json!({
                        "id": g.id,
                        "description": g.description,
                        "priority": g.priority,
                        "status": g.status.to_string(),
                        "assigned_to": g.assigned_to,
                        "current_activity": g.current_activity,
                        "wip_refs": g.wip_refs,
                    })
                })
                .collect();
            let b: Vec<Value> = board
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
            (a, b)
        }
        Err(_) => match serde_json::from_str::<Vec<GoalRecord>>(&content) {
            // Flat array of GoalRecord from FileBackedGoalStore — map fields.
            Ok(records) => {
                let mapped: Vec<Value> = records
                    .into_iter()
                    .map(|r| {
                        json!({
                            "id": r.slug,
                            "description": r.title,
                            "priority": r.priority,
                            "status": r.status.to_string(),
                            "assigned_to": r.owner_identity,
                        })
                    })
                    .collect();
                (mapped, vec![])
            }
            Err(_) => (vec![], vec![]),
        },
    };

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
    let goal_path = state_root.join("goal_records.json");

    // Only seed if no goals exist yet
    if goal_path.exists()
        && let Ok(content) = std::fs::read_to_string(&goal_path)
        && let Ok(val) = serde_json::from_str::<Value>(&content)
    {
        let has_goals = val
            .get("active")
            .and_then(|a| a.as_array())
            .is_some_and(|a| !a.is_empty());
        if has_goals {
            return Json(json!({"status": "already_seeded", "message": "Goals already exist"}));
        }
    }

    let seed_board = json!({
        "active": [
            {
                "id": "self-improvement",
                "description": "Continuously improve own capabilities through gym scenarios and self-evaluation",
                "priority": 1,
                "status": "in_progress",
                "assigned_to": "simard",
                "progress": [{"timestamp": chrono::Utc::now().to_rfc3339(), "note": "Goal seeded via dashboard"}]
            },
            {
                "id": "knowledge-growth",
                "description": "Expand knowledge base through meetings, research, and cognitive memory consolidation",
                "priority": 2,
                "status": "in_progress",
                "assigned_to": "simard",
                "progress": [{"timestamp": chrono::Utc::now().to_rfc3339(), "note": "Goal seeded via dashboard"}]
            },
            {
                "id": "operational-health",
                "description": "Maintain system health: budget compliance, resource usage, and error rates within thresholds",
                "priority": 3,
                "status": "in_progress",
                "assigned_to": "simard",
                "progress": [{"timestamp": chrono::Utc::now().to_rfc3339(), "note": "Goal seeded via dashboard"}]
            }
        ],
        "backlog": [
            {
                "id": "distributed-sync",
                "description": "Establish hive mind sync with remote Simard instances for cross-agent knowledge sharing",
                "source": "dashboard-seed",
                "score": 0.7
            },
            {
                "id": "meeting-quality",
                "description": "Improve meeting facilitation quality and actionable outcome generation",
                "source": "dashboard-seed",
                "score": 0.6
            }
        ]
    });

    if let Err(e) = std::fs::create_dir_all(&state_root) {
        return Json(
            json!({"status": "error", "error": format!("failed to create state dir: {e}")}),
        );
    }
    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&seed_board).unwrap(),
    ) {
        Ok(()) => {
            Json(json!({"status": "ok", "message": "Seeded 3 active goals and 2 backlog items"}))
        }
        Err(e) => Json(json!({"status": "error", "error": format!("write failed: {e}")})),
    }
}

pub(crate) async fn add_goal(Json(body): Json<Value>) -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let content = std::fs::read_to_string(&goal_path).unwrap_or_default();
    let mut board = serde_json::from_str::<GoalBoard>(&content).unwrap_or_default();

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

    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&board).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok", "id": id})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn remove_goal(Path(id): Path<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let content = std::fs::read_to_string(&goal_path).unwrap_or_default();
    let mut board = serde_json::from_str::<GoalBoard>(&content).unwrap_or_default();

    let before_active = board.active.len();
    let before_backlog = board.backlog.len();
    board.active.retain(|g| g.id != id);
    board.backlog.retain(|g| g.id != id);

    if board.active.len() == before_active && board.backlog.len() == before_backlog {
        return Json(json!({"error": "goal not found"}));
    }

    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&board).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn update_goal_status(
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let content = std::fs::read_to_string(&goal_path).unwrap_or_default();
    let mut board = serde_json::from_str::<GoalBoard>(&content).unwrap_or_default();

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

    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&board).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn promote_backlog_item(Path(id): Path<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let content = std::fs::read_to_string(&goal_path).unwrap_or_default();
    let mut board = serde_json::from_str::<GoalBoard>(&content).unwrap_or_default();

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

    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&board).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn demote_goal(Path(id): Path<String>) -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");
    let content = std::fs::read_to_string(&goal_path).unwrap_or_default();
    let mut board = serde_json::from_str::<GoalBoard>(&content).unwrap_or_default();

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

    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&board).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}
