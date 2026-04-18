use axum::{
    Json, Router,
    extract::Path,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    middleware, response,
    routing::{delete, get, post, put},
};
use serde_json::{Value, json};

use super::auth::{require_auth, try_login};
use crate::agent_registry::{AgentRegistry, FileBackedAgentRegistry};
use crate::build_lock::BuildLock;
use crate::cognitive_memory::{as_f64, as_i64, as_str, CognitiveMemoryOps, NativeCognitiveMemory};
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};
use crate::goals::{GoalRecord, goal_slug};

pub fn build_router() -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/issues", get(issues))
        .route("/api/metrics", get(metrics))
        .route("/api/costs", get(costs))
        .route("/api/budget", get(get_budget).post(set_budget))
        .route("/api/goals", get(goals).post(add_goal))
        .route("/api/goals/seed", post(seed_goals))
        .route("/api/goals/promote/{id}", post(promote_backlog_item))
        .route("/api/goals/{id}", delete(remove_goal))
        .route("/api/goals/{id}/status", put(update_goal_status))
        .route("/api/distributed", get(distributed))
        .route(
            "/api/hosts",
            get(get_hosts).post(add_host).delete(remove_host),
        )
        .route("/api/logs", get(logs))
        .route("/api/processes", get(processes))
        .route(
            "/api/registry",
            get(registry_list)
                .post(registry_register)
                .delete(registry_deregister),
        )
        .route("/api/registry/reap", post(registry_reap))
        .route("/api/build-lock", get(build_lock_status))
        .route("/api/build-lock/release", post(build_lock_force_release))
        .route("/api/memory", get(memory_metrics))
        .route("/api/memory/search", post(memory_search))
        .route("/api/memory/graph", get(memory_graph))
        .route("/api/traces", get(traces))
        .route("/api/activity", get(activity))
        .route("/api/workboard", get(workboard))
        .route("/api/current-work", get(current_work))
        .route("/api/ooda-thinking", get(ooda_thinking))
        .route("/ws/chat", get(ws_chat_handler))
        .route("/api/login", post(login))
        .route("/login", get(login_page))
        .route("/", get(index))
        .layer(middleware::from_fn(require_auth))
}

async fn login(Json(body): Json<Value>) -> response::Response {
    let code = body.get("code").and_then(|v| v.as_str()).unwrap_or("");
    match try_login(code) {
        Some(session_token) => response::Response::builder()
            .status(200)
            .header(
                "set-cookie",
                format!(
                    "simard_session={session_token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400"
                ),
            )
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"ok": true}).to_string(),
            ))
            .unwrap(),
        None => response::Response::builder()
            .status(401)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"ok": false, "error": "invalid code"}).to_string(),
            ))
            .unwrap(),
    }
}

async fn login_page() -> response::Html<String> {
    response::Html(LOGIN_HTML.to_string())
}

async fn status() -> Json<Value> {
    let version = format!(
        "{}.{}",
        env!("CARGO_PKG_VERSION"),
        env!("SIMARD_BUILD_NUMBER")
    );
    let git_hash = env!("SIMARD_GIT_HASH");

    // Real health check: read daemon_health.json
    let health_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
        .join("simard")
        .join("daemon_health.json");

    let daemon_health: Option<serde_json::Value> = std::fs::read_to_string(&health_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let ooda_status = match &daemon_health {
        Some(h) => {
            if let Some(ts) = h.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(health_time) = chrono::DateTime::parse_from_rfc3339(ts) {
                    let age = chrono::Utc::now().signed_duration_since(health_time);
                    // Threshold: cycle interval (300s) + max cycle runtime (~600s).
                    // With the heartbeat at cycle start, age should rarely exceed this.
                    if age.num_seconds() < 900 {
                        "running"
                    } else {
                        "stale"
                    }
                } else {
                    "unknown"
                }
            } else {
                "unknown"
            }
        }
        None => "stopped",
    };

    let disk = disk_usage_pct().await;

    let child_count = std::process::Command::new("pgrep")
        .args(["-f", "-c", "copilot.*Simard|simard.*ooda|cargo.*simard"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let mut status_json = json!({
        "version": version,
        "git_hash": git_hash,
        "ooda_daemon": ooda_status,
        "active_processes": child_count,
        "disk_usage_pct": disk,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    if let Some(h) = daemon_health {
        status_json["daemon_health"] = h;
    }

    Json(status_json)
}

async fn issues() -> Json<Value> {
    let output = tokio::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,labels",
        ])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            match serde_json::from_str::<Value>(&raw) {
                Ok(v) => Json(v),
                Err(_) => Json(json!({"error": "failed to parse gh output"})),
            }
        }
        _ => Json(json!({"error": "failed to run gh issue list"})),
    }
}

async fn metrics() -> Json<Value> {
    let recent = crate::self_metrics::recent_metrics(100).unwrap_or_default();
    let report = crate::self_metrics::daily_report().unwrap_or_default();

    let entries: Vec<Value> = recent
        .iter()
        .map(|e| {
            json!({
                "timestamp": e.timestamp.to_rfc3339(),
                "metric_name": e.metric_name,
                "value": e.value,
                "context": e.context,
            })
        })
        .collect();

    Json(json!({
        "recent": entries,
        "daily_report": report,
    }))
}

async fn costs() -> Json<Value> {
    let daily = crate::cost_tracking::daily_summary()
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .unwrap_or_else(|e| json!({"error": format!("daily: {e}")}));
    let weekly = crate::cost_tracking::weekly_summary()
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .unwrap_or_else(|e| json!({"error": format!("weekly: {e}")}));
    Json(json!({
        "daily": daily,
        "weekly": weekly,
    }))
}

/// Budget config file path.
fn budget_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    std::path::PathBuf::from(home)
        .join(".simard")
        .join("budget.json")
}

async fn get_budget() -> Json<Value> {
    let path = budget_config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    match serde_json::from_str::<Value>(&content) {
        Ok(v) => Json(v),
        Err(_) => Json(json!({
            "daily_budget_usd": std::env::var("SIMARD_DAILY_BUDGET_USD")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(500.0),
            "weekly_budget_usd": std::env::var("SIMARD_WEEKLY_BUDGET_USD")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(2500.0),
        })),
    }
}

async fn set_budget(Json(body): Json<Value>) -> Json<Value> {
    let path = budget_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(
        &path,
        serde_json::to_string_pretty(&body).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn goals() -> Json<Value> {
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

async fn seed_goals() -> Json<Value> {
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


async fn add_goal(Json(body): Json<Value>) -> Json<Value> {
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
        let priority = body
            .get("priority")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as u32;
        board.active.push(ActiveGoal {
            id: id.clone(),
            description: desc,
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
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

async fn remove_goal(Path(id): Path<String>) -> Json<Value> {
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

async fn update_goal_status(Path(id): Path<String>, Json(body): Json<Value>) -> Json<Value> {
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

async fn promote_backlog_item(Path(id): Path<String>) -> Json<Value> {
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
    });

    match std::fs::write(
        &goal_path,
        serde_json::to_string_pretty(&board).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn memory_search(Json(body): Json<Value>) -> Json<Value> {
    let query = body.get("query").and_then(|v| v.as_str()).unwrap_or("");
    if query.is_empty() {
        return Json(json!({"status": "error", "error": "query is required"}));
    }

    // Search through memory_records.json, evidence_records.json for matching content
    let state_root = resolve_state_root();
    let mut results: Vec<Value> = Vec::new();

    for (file, label) in [
        ("memory_records.json", "memory"),
        ("evidence_records.json", "evidence"),
        ("goal_records.json", "goal"),
    ] {
        let path = state_root.join(file);
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(val) = serde_json::from_str::<Value>(&content)
        {
            let search_in = |v: &Value| -> bool {
                let s = serde_json::to_string(v).unwrap_or_default().to_lowercase();
                s.contains(&query.to_lowercase())
            };

            match val {
                Value::Array(arr) => {
                    for item in arr.iter().filter(|i| search_in(i)).take(10) {
                        results.push(json!({"source": label, "data": item}));
                    }
                }
                Value::Object(ref map) => {
                    // For goal board format: search in active and backlog
                    if let Some(Value::Array(active)) = map.get("active") {
                        for item in active.iter().filter(|i| search_in(i)).take(5) {
                            results.push(json!({"source": "active_goal", "data": item}));
                        }
                    }
                    if let Some(Value::Array(backlog)) = map.get("backlog") {
                        for item in backlog.iter().filter(|i| search_in(i)).take(5) {
                            results.push(json!({"source": "backlog_goal", "data": item}));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Json(json!({
        "query": query,
        "result_count": results.len(),
        "results": results,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn memory_graph() -> Json<Value> {
    let state_root = resolve_state_root();
    let mem = match NativeCognitiveMemory::open_read_only(&state_root) {
        Ok(m) => m,
        Err(e) => {
            return Json(json!({
                "nodes": [],
                "edges": [],
                "stats": {},
                "error": format!("Cannot open cognitive memory: {e}"),
            }));
        }
    };

    let stats = mem.get_statistics().unwrap_or_default();
    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();

    let query_rows = |cypher: &str| -> Vec<Vec<lbug::Value>> {
        mem.query(cypher).unwrap_or_default()
    };

    for row in query_rows(
        "MATCH (w:WorkingMemory) RETURN w.id, w.slot_type, w.content, w.task_id, w.relevance LIMIT 100",
    ) {
        if let Some(id) = row.first().and_then(as_str) {
            let content = row.get(2).and_then(as_str).unwrap_or("");
            let label = if content.len() > 60 { format!("{}…", &content[..60]) } else { content.to_string() };
            nodes.push(json!({
                "id": id, "type": "WorkingMemory", "label": label,
                "content": content,
                "task_id": row.get(3).and_then(as_str).unwrap_or(""),
                "relevance": row.get(4).and_then(as_f64).unwrap_or(0.0),
            }));
        }
    }

    for row in query_rows(
        "MATCH (f:Fact) RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags LIMIT 100",
    ) {
        if let Some(id) = row.first().and_then(as_str) {
            let concept = row.get(1).and_then(as_str).unwrap_or("");
            let content = row.get(2).and_then(as_str).unwrap_or("");
            let label = if concept.is_empty() {
                if content.len() > 60 { format!("{}…", &content[..60]) } else { content.to_string() }
            } else { concept.to_string() };
            nodes.push(json!({
                "id": id, "type": "SemanticFact", "label": label,
                "content": content, "confidence": row.get(3).and_then(as_f64).unwrap_or(0.0),
                "source_id": row.get(4).and_then(as_str).unwrap_or(""),
            }));
        }
    }

    for row in query_rows(
        "MATCH (e:Episode) RETURN e.id, e.content, e.source_label, e.temporal_index LIMIT 100",
    ) {
        if let Some(id) = row.first().and_then(as_str) {
            let content = row.get(1).and_then(as_str).unwrap_or("");
            let label = if content.len() > 60 { format!("{}…", &content[..60]) } else { content.to_string() };
            nodes.push(json!({
                "id": id, "type": "EpisodicMemory", "label": label,
                "content": content,
                "temporal_index": row.get(3).and_then(as_i64).unwrap_or(0),
            }));
        }
    }

    for row in query_rows(
        "MATCH (p:Procedure) RETURN p.id, p.name, p.steps, p.prerequisites, p.usage_count LIMIT 100",
    ) {
        if let Some(id) = row.first().and_then(as_str) {
            nodes.push(json!({
                "id": id, "type": "ProceduralMemory",
                "label": row.get(1).and_then(as_str).unwrap_or(""),
                "content": row.get(2).and_then(as_str).unwrap_or(""),
                "usage_count": row.get(4).and_then(as_i64).unwrap_or(0),
            }));
        }
    }

    for row in query_rows(
        "MATCH (p:Prospective) RETURN p.id, p.description, p.trigger_condition, p.action_on_trigger, p.status, p.priority LIMIT 100",
    ) {
        if let Some(id) = row.first().and_then(as_str) {
            nodes.push(json!({
                "id": id, "type": "ProspectiveMemory",
                "label": row.get(1).and_then(as_str).unwrap_or(""),
                "content": row.get(2).and_then(as_str).unwrap_or(""),
                "status": row.get(4).and_then(as_str).unwrap_or("pending"),
            }));
        }
    }

    for row in query_rows("MATCH (s:Sensory) RETURN s.id, s.modality, s.raw_data LIMIT 100") {
        if let Some(id) = row.first().and_then(as_str) {
            let modality = row.get(1).and_then(as_str).unwrap_or("");
            let raw = row.get(2).and_then(as_str).unwrap_or("");
            let label = if raw.len() > 40 {
                format!("[{modality}] {}…", &raw[..40])
            } else {
                format!("[{modality}] {raw}")
            };
            nodes.push(json!({
                "id": id, "type": "SensoryBuffer", "label": label,
                "content": raw, "modality": modality,
            }));
        }
    }

    // Infer edges: link WorkingMemory to nodes sharing the same task_id via source_id
    let working_nodes: Vec<(String, String)> = nodes.iter()
        .filter(|n| n["type"] == "WorkingMemory")
        .filter_map(|n| {
            let id = n["id"].as_str()?.to_string();
            let tid = n["task_id"].as_str()?.to_string();
            if tid.is_empty() { None } else { Some((id, tid)) }
        })
        .collect();
    for wn in &working_nodes {
        for other in &nodes {
            if other["type"] == "WorkingMemory" { continue; }
            if let Some(oid) = other["id"].as_str() {
                if let Some(src) = other["source_id"].as_str() {
                    if !src.is_empty() && src == wn.1 {
                        edges.push(json!({"source": wn.0, "target": oid, "type": "REFERENCES"}));
                    }
                }
            }
        }
    }

    // Link episodes with sequential temporal indices
    let mut episode_ids: Vec<(String, i64)> = nodes.iter()
        .filter(|n| n["type"] == "EpisodicMemory")
        .filter_map(|n| Some((n["id"].as_str()?.to_string(), n["temporal_index"].as_i64().unwrap_or(0))))
        .collect();
    episode_ids.sort_by_key(|e| e.1);
    for pair in episode_ids.windows(2) {
        edges.push(json!({"source": pair[0].0, "target": pair[1].0, "type": "FOLLOWS"}));
    }

    Json(json!({
        "nodes": nodes,
        "edges": edges,
        "stats": {
            "working": stats.working_count,
            "semantic": stats.semantic_count,
            "episodic": stats.episodic_count,
            "procedural": stats.procedural_count,
            "prospective": stats.prospective_count,
            "sensory": stats.sensory_count,
        },
    }))
}

async fn traces() -> Json<Value> {
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
async fn activity() -> Json<Value> {
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

    // --- 3. Open PRs by Simard ---
    let open_prs = run_gh_json(&[
        "pr",
        "list",
        "--author",
        "@me",
        "--state",
        "open",
        "--json",
        "number,title,url,createdAt,headRefName",
    ])
    .await;

    // --- 4. Issues assigned to Simard ---
    let assigned_issues = run_gh_json(&[
        "issue",
        "list",
        "--assignee",
        "@me",
        "--state",
        "open",
        "--json",
        "number,title,url,labels",
    ])
    .await;

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


// ---------------------------------------------------------------------------
// Workboard API — aggregated view of Simard's current mental state
// ---------------------------------------------------------------------------

async fn workboard() -> Json<Value> {
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
    let goal_board = serde_json::from_str::<GoalBoard>(&goal_content).ok();

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
                        crate::goal_curation::GoalProgress::Completed => {
                            ("done".to_string(), 100)
                        }
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
    {
        if !actions.is_empty() {
            recent_actions.push(json!({
                "cycle": cycle_number,
                "action": "current",
                "target": "",
                "result": actions,
                "at": health_timestamp,
            }));
        }
    }

    for report in &recent_reports {
        let cycle_num = report
            .get("cycle_number")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if let Some(summary) = report.get("summary").and_then(|v| v.as_str()) {
            recent_actions.push(json!({
                "cycle": cycle_num,
                "action": "cycle-summary",
                "target": "",
                "result": summary,
                "at": "",
            }));
        } else if let Some(rpt) = report.get("report") {
            let summary_text = rpt
                .get("summary")
                .and_then(|v| v.as_str())
                .or_else(|| rpt.get("actions_taken").and_then(|v| v.as_str()))
                .unwrap_or("");
            if !summary_text.is_empty() {
                recent_actions.push(json!({
                    "cycle": cycle_num,
                    "action": "cycle-summary",
                    "target": "",
                    "result": summary_text,
                    "at": rpt.get("timestamp").and_then(|t| t.as_str()).unwrap_or(""),
                }));
            }
        }
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
        for tag in &["action", "goal", "decision", "episode", "observation", "insight"] {
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

/// Real-time snapshot of what Simard is doing right now.
///
/// Composes data from `daemon_health.json` (cycle/phase), `goal_records.json`
/// (active goals), and the agent registry (spawned engineers).
async fn current_work() -> Json<Value> {
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

fn is_pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Run a `gh` CLI command and parse JSON output, returning a `Value`.
async fn run_gh_json(args: &[&str]) -> Value {
    match tokio::process::Command::new("gh").args(args).output().await {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            serde_json::from_str::<Value>(&raw).unwrap_or(json!([]))
        }
        _ => json!([]),
    }
}

/// Read the most recent N cycle report files from disk.
fn read_recent_cycle_reports(state_root: &std::path::Path, n: usize) -> Vec<Value> {
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

async fn distributed() -> Json<Value> {
    // Query the Simard VM status via azlin connect with a timeout so the
    // dashboard doesn't hang if the bastion is slow.
    //
    // We use `systemd-run --user --pipe` to run the check script in a fresh
    // transient scope.  When azlin runs as a direct child of the daemon's
    // service cgroup, the bastion SSH produces empty stdout (the daemon's
    // inherited pipe/socket FDs or cgroup restrictions interfere with
    // azlin's PTY routing).  Running in a separate scope avoids this.
    let vm_status = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(|| {
            let state_root = std::env::var("SIMARD_STATE_ROOT").unwrap_or_else(|_| {
                format!(
                    "{}/.simard",
                    std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".into())
                )
            });
            let script = format!("{}/bin/check_vm.sh", state_root);
            std::process::Command::new("systemd-run")
                .args(["--user", "--pipe", "--quiet", &script])
                .output()
        }),
    )
    .await;

    let mut vm_info = json!({
        "vm_name": "Simard",
        "resource_group": "rysweet-linux-vm-pool",
        "status": "unknown",
    });

    match vm_status {
        Ok(Ok(Ok(output))) => {
            let raw_stdout = String::from_utf8_lossy(&output.stdout);
            let raw_stderr = String::from_utf8_lossy(&output.stderr);
            // azlin connect --no-tmux routes remote stdout to local stderr
            // when spawned without a TTY (rysweet/azlin#980). Strip ANSI
            // escape codes then search both streams for our KEY=value markers.
            let stdout = strip_ansi_codes(&raw_stdout);
            let stderr = strip_ansi_codes(&raw_stderr);
            let haystack = if stdout.contains("HOSTNAME=") {
                stdout
            } else if stderr.contains("HOSTNAME=") {
                stderr
            } else {
                // Last resort: combine both in case markers are split across streams
                let combined = format!("{}\n{}", stdout, stderr);
                if combined.contains("HOSTNAME=") {
                    combined
                } else {
                    String::new()
                }
            };
            if !haystack.is_empty() {
                vm_info["status"] = json!("reachable");
                for line in haystack.lines() {
                    if let Some((key, val)) = line.split_once('=') {
                        let key = key.trim().to_lowercase();
                        let val = val.trim();
                        match key.as_str() {
                            "hostname" => vm_info["hostname"] = json!(val),
                            "uptime" => vm_info["uptime"] = json!(val),
                            "disk_root" => {
                                vm_info["disk_root_pct"] = json!(val.parse::<u32>().ok());
                            }
                            "disk_data" => {
                                vm_info["disk_data_pct"] = json!(val.parse::<u32>().ok());
                            }
                            "disk_tmp" => vm_info["disk_tmp_pct"] = json!(val.parse::<u32>().ok()),
                            "simard_procs" => {
                                vm_info["simard_processes"] = json!(val.parse::<u32>().ok());
                            }
                            "cargo_procs" => {
                                vm_info["cargo_processes"] = json!(val.parse::<u32>().ok());
                            }
                            "load" => vm_info["load_avg"] = json!(val),
                            "mem_used" => vm_info["memory_mb"] = json!(val),
                            _ => {}
                        }
                    }
                }
            } else {
                vm_info["status"] = json!("unreachable");
                vm_info["debug_hint"] =
                    json!("HOSTNAME= not found in stdout or stderr after ANSI stripping");
            }
        }
        Ok(Ok(Err(e))) => {
            vm_info["status"] = json!("error");
            vm_info["error"] = json!(format!("azlin connect failed: {e}"));
        }
        Ok(Err(e)) => {
            vm_info["status"] = json!("error");
            vm_info["error"] = json!(format!("task join failed: {e}"));
        }
        Err(_) => {
            vm_info["status"] = json!("timeout");
            vm_info["error"] = json!("azlin connect timed out after 30s");
        }
    }

    // Local host info for comparison
    let local_host = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    Json(json!({
        "local": {
            "hostname": local_host,
            "type": "dev-machine",
        },
        "remote_vms": [vm_info],
        "topology": "distributed",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Strip ANSI escape sequences (CSI, OSC, and single-char escapes) so that
/// output from azlin/SSH can be reliably parsed for KEY=value markers.
fn strip_ansi_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    // CSI sequence: consume until a letter or '@'-'~'
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch.is_ascii_alphabetic() || ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next(); // consume ']'
                    // OSC sequence: consume until BEL or ST (\x1b\\)
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    // Single-char escape (e.g. \x1b=, \x1b>)
                    chars.next();
                }
            }
        } else if c == '\r' {
            // Strip carriage returns (common in SSH/PTY output)
            continue;
        } else {
            out.push(c);
        }
    }
    out
}

/// Hosts config file path.
fn hosts_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    std::path::PathBuf::from(home)
        .join(".simard")
        .join("hosts.json")
}

fn load_hosts() -> Vec<Value> {
    let path = hosts_config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_else(|_| "[]".to_string());
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_hosts(hosts: &[Value]) -> std::io::Result<()> {
    let path = hosts_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(hosts).unwrap_or_default(),
    )
}

async fn get_hosts() -> Json<Value> {
    Json(json!({ "hosts": load_hosts() }))
}

async fn add_host(Json(body): Json<Value>) -> Json<Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let rg = body
        .get("resource_group")
        .and_then(|v| v.as_str())
        .unwrap_or("rysweet-linux-vm-pool");
    if name.is_empty() {
        return Json(json!({"error": "name is required"}));
    }
    let mut hosts = load_hosts();
    if hosts
        .iter()
        .any(|h| h.get("name").and_then(|v| v.as_str()) == Some(name))
    {
        return Json(json!({"error": format!("host '{name}' already exists")}));
    }
    hosts.push(json!({
        "name": name,
        "resource_group": rg,
        "added_at": chrono::Utc::now().to_rfc3339(),
    }));
    match save_hosts(&hosts) {
        Ok(_) => Json(json!({"status": "ok", "hosts": hosts})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn remove_host(Json(body): Json<Value>) -> Json<Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut hosts = load_hosts();
    let before = hosts.len();
    hosts.retain(|h| h.get("name").and_then(|v| v.as_str()) != Some(name));
    if hosts.len() == before {
        return Json(json!({"error": format!("host '{name}' not found")}));
    }
    match save_hosts(&hosts) {
        Ok(_) => Json(json!({"status": "ok", "hosts": hosts})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

async fn index() -> axum::response::Html<String> {
    axum::response::Html(INDEX_HTML.to_string())
}

// ---------------------------------------------------------------------------
// WebSocket chat — bridges to Simard's meeting facilitator conversation model
// ---------------------------------------------------------------------------

/// Load the meeting system prompt from disk.
fn load_dashboard_meeting_prompt() -> SimardResult<String> {
    let candidates = [
        // Runtime: next to the binary
        std::env::current_exe().ok().and_then(|p| {
            p.parent()
                .map(|d| d.join("prompt_assets/simard/meeting_system.md"))
        }),
        // Runtime: repo checkout (common on the Simard VM)
        Some(
            std::path::PathBuf::from(
                std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string()),
            )
            .join("src/Simard/prompt_assets/simard/meeting_system.md"),
        ),
        // Build-time: source tree via CARGO_MANIFEST_DIR (dev only)
        Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("prompt_assets/simard/meeting_system.md"),
        ),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            return Ok(content);
        }
    }
    Err(SimardError::PromptNotFound {
        name: "meeting_system.md".into(),
    })
}

/// Open an agent session for the dashboard chat.
/// Uses the same config-driven provider as the CLI meeting REPL
/// (controlled by `SIMARD_LLM_PROVIDER`, defaults to RustyClawd).
fn open_dashboard_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    match crate::session_builder::SessionBuilder::new(crate::identity::OperatingMode::Meeting)
        .node_id("dashboard-chat")
        .address("dashboard-chat://local")
        .adapter_tag("meeting-dashboard")
        .open()
    {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[simard] dashboard chat session failed: {e}");
            None
        }
    }
}

async fn ws_chat_handler(ws: WebSocketUpgrade) -> response::Response {
    ws.on_upgrade(handle_ws_chat)
}

async fn handle_ws_chat(mut socket: WebSocket) {
    use crate::meeting_backend::{MeetingBackend, MeetingCommand, parse_command};

    // Use the full agent session (SessionBuilder) for chat.
    // The lightweight piped-subprocess path is disabled — it spawns
    // `amplihack copilot --subprocess-safe` which hangs indefinitely
    // because the Copilot CLI doesn't support non-interactive piped mode.
    let agent_session: Option<Box<dyn crate::base_types::BaseTypeSession>> =
        tokio::task::spawn_blocking(open_dashboard_agent_session)
            .await
            .ok()
            .flatten();

    let agent = match agent_session {
        Some(full) => {
            eprintln!("[simard] chat using full agent backend");
            full
        }
        None => {
            eprintln!("[simard][ERROR] no chat backend available — agent session failed to open");
            let _ = socket
                .send(Message::Text(
                    json!({"role":"system","content":"No agent backend available. Check SIMARD_LLM_PROVIDER and auth config."})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };

    let system_prompt = match load_dashboard_meeting_prompt() {
        Ok(prompt) => prompt,
        Err(e) => {
            eprintln!("[simard] dashboard chat: {e}");
            let _ = socket
                .send(Message::Text(
                    json!({"role":"error","content": e.to_string()})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };
    let mut backend = MeetingBackend::new_session("Dashboard Chat", agent, None, system_prompt);

    let _ = socket
        .send(Message::Text(
            json!({"role":"system","content":"Connected to Simard. Speak naturally — /help for commands, /close to end."})
                .to_string()
                .into(),
        ))
        .await;

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                let text = text.to_string();
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let cmd = parse_command(trimmed);
                match cmd {
                    MeetingCommand::Close => {
                        // Close runs synchronous LLM call — use spawn_blocking
                        let summary = tokio::task::spawn_blocking(move || backend.close()).await;
                        let recap = match summary {
                            Ok(Ok(s)) => format!(
                                "Meeting closed. {} messages. Summary: {}",
                                s.message_count, s.summary_text
                            ),
                            Ok(Err(e)) => format!("Meeting closed with error: {e}"),
                            Err(e) => format!("Meeting close failed: {e}"),
                        };
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": recap}).to_string().into(),
                            ))
                            .await;
                        break;
                    }
                    MeetingCommand::Help => {
                        let help = "Commands: /status, /template [name], /export, /theme <text>, /recap, /preview, /close, /help. Everything else is natural conversation with Simard.";
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": help}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Status => {
                        let status = backend.status();
                        let info = format!(
                            "Topic: {}\nMessages: {}\nStarted: {}\nOpen: {}",
                            status.topic, status.message_count, status.started_at, status.is_open
                        );
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": info}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Template(name) => {
                        use crate::meeting_backend::persist::{TEMPLATES, find_template};
                        let content = if name.is_empty() {
                            let mut listing = "Available templates:\n".to_string();
                            for t in TEMPLATES {
                                listing.push_str(&format!("  {} — {}\n", t.name, t.description));
                            }
                            listing.push_str("\nUsage: /template <name>");
                            listing
                        } else if let Some(tmpl) = find_template(&name) {
                            tmpl.agenda.to_string()
                        } else {
                            format!(
                                "Unknown template: {name}. Available: standup, 1on1, retro, planning"
                            )
                        };
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Export => {
                        use crate::meeting_backend::persist::write_markdown_export;
                        let content = match write_markdown_export(
                            backend.topic(),
                            backend.started_at(),
                            backend.history(),
                        ) {
                            Ok(path) => format!("Meeting exported to: {}", path.display()),
                            Err(e) => format!("[export error: {e}]"),
                        };
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Theme(theme) => {
                        backend.push_theme(theme.clone());
                        let content = format!("Theme recorded: {theme}");
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": content})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Recap => {
                        let status = backend.status();
                        let themes = backend.explicit_themes();
                        let mut recap = format!(
                            "── Meeting Recap ──\nTopic: {}\nMessages: {}\nStarted: {}",
                            status.topic, status.message_count, status.started_at
                        );
                        if !themes.is_empty() {
                            recap.push_str(&format!("\nThemes: {}", themes.join(", ")));
                        }
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": recap}).to_string().into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Preview => {
                        let status = backend.status();
                        let themes = backend.explicit_themes();
                        let preview = format!(
                            "── Handoff Preview ──\nTopic: {}\nMessages so far: {}\nThemes: {}",
                            status.topic,
                            status.message_count,
                            if themes.is_empty() {
                                "none yet".to_string()
                            } else {
                                themes.join(", ")
                            }
                        );
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": preview})
                                    .to_string()
                                    .into(),
                            ))
                            .await;
                    }
                    MeetingCommand::Conversation(user_text) => {
                        // send_message is synchronous — use spawn_blocking
                        let result = tokio::task::spawn_blocking(move || {
                            let resp = backend.send_message(&user_text);
                            (backend, resp)
                        })
                        .await;
                        match result {
                            Ok((returned_backend, Ok(resp))) => {
                                backend = returned_backend;
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"assistant","content": resp.content})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                            }
                            Ok((returned_backend, Err(e))) => {
                                backend = returned_backend;
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"system","content": format!("[error: {e}]")})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                            }
                            Err(e) => {
                                let _ = socket
                                    .send(Message::Text(
                                        json!({"role":"system","content": format!("[internal error: {e}]")})
                                            .to_string()
                                            .into(),
                                    ))
                                    .await;
                                break;
                            }
                        }
                    }
                }
            }
            Message::Close(_) => {
                // Clean up on disconnect
                let _ = tokio::task::spawn_blocking(move || backend.close()).await;
                break;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Logs endpoint — returns tail of daemon log + OODA transcripts
// ---------------------------------------------------------------------------

async fn logs() -> Json<Value> {
    let state_root = resolve_state_root();

    // Try multiple log sources for daemon output (#414)
    let daemon_log = read_tail("/var/log/simard-daemon.log", 200)
        .or_else(|| {
            let alt_path = state_root.join("simard-daemon.log");
            read_tail(&alt_path.to_string_lossy(), 200)
        })
        .or_else(|| {
            let alt_path = state_root.join("ooda.log");
            read_tail(&alt_path.to_string_lossy(), 200)
        })
        .or_else(|| {
            let alt_path = state_root.join("simard.log");
            read_tail(&alt_path.to_string_lossy(), 200)
        })
        .unwrap_or_default();

    // Try journalctl if no file-based logs found (not a degradation —
    // journalctl is another valid log source, and the UI shows which was used).
    let combined_log = if daemon_log.is_empty() {
        read_journal_logs().await
    } else {
        daemon_log
    };

    let ooda_dir = state_root.join("ooda_transcripts");

    let mut transcripts: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&ooda_dir) {
        let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.path()));
        for entry in files.into_iter().take(10) {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let preview = read_tail(&path.to_string_lossy(), 20).unwrap_or_default();
            transcripts.push(json!({
                "name": name,
                "size_bytes": size,
                "preview_lines": preview,
            }));
        }
    }

    let cost_log = {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
        let ledger = std::path::PathBuf::from(home).join(".simard/costs/ledger.jsonl");
        read_tail(&ledger.to_string_lossy(), 50).unwrap_or_default()
    };

    // Collect recent terminal session transcripts from /tmp (#414)
    let mut terminal_transcripts: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("simard-terminal-shell-")
            })
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.path()));
        for entry in files.into_iter().take(10) {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let preview = read_tail(&path.to_string_lossy(), 20).unwrap_or_default();
            terminal_transcripts.push(json!({
                "name": name,
                "size_bytes": size,
                "preview_lines": preview,
            }));
        }
    }

    Json(json!({
        "daemon_log_lines": combined_log,
        "ooda_transcripts": transcripts,
        "terminal_transcripts": terminal_transcripts,
        "cost_log_lines": cost_log,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

fn read_tail(path: &str, max_lines: usize) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    let start = lines.len().saturating_sub(max_lines);
    Some(lines[start..].to_vec())
}

/// Read recent log entries from systemd journal for simard-related units (#414).
async fn read_journal_logs() -> Vec<String> {
    // Try user-level journal first
    let output = tokio::process::Command::new("journalctl")
        .args([
            "--user",
            "--unit=simard*",
            "--no-pager",
            "-n",
            "200",
            "--output=short-iso",
        ])
        .output()
        .await;

    if let Ok(o) = output
        && o.status.success()
    {
        let text = String::from_utf8_lossy(&o.stdout);
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.contains("No entries"))
            .map(String::from)
            .collect();
        if !lines.is_empty() {
            return lines;
        }
    }

    // Also try system-level journal (broader scope than user-level).
    let output = tokio::process::Command::new("journalctl")
        .args([
            "--unit=simard*",
            "--no-pager",
            "-n",
            "200",
            "--output=short-iso",
        ])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines()
                .filter(|l| !l.contains("No entries"))
                .map(String::from)
                .collect()
        }
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Active processes panel
// ---------------------------------------------------------------------------

async fn processes() -> Json<Value> {
    let output = tokio::process::Command::new("ps")
        .args(["axo", "pid,etime,comm,args"])
        .output()
        .await;

    let mut procs: Vec<Value> = Vec::new();

    if let Ok(o) = output {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines().skip(1) {
            let lower = line.to_lowercase();
            if (lower.contains("simard") || lower.contains("ooda") || lower.contains("copilot"))
                && !lower.contains("ps axo")
            {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    procs.push(json!({
                        "pid": parts[0],
                        "uptime": parts[1],
                        "command": parts[2],
                        "full_args": parts[3..].join(" "),
                    }));
                }
            }
        }
    }

    Json(json!({
        "processes": procs,
        "count": procs.len(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// Agent Registry API (#296)
// ---------------------------------------------------------------------------

async fn registry_list() -> Json<Value> {
    let reg = FileBackedAgentRegistry::new(&resolve_state_root());
    match reg.list() {
        Ok(entries) => {
            let serialized: Vec<Value> = entries
                .iter()
                .filter_map(|e| serde_json::to_value(e).ok())
                .collect();
            Json(json!({
                "agents": serialized,
                "count": serialized.len(),
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }))
        }
        Err(e) => Json(json!({
            "error": e.to_string(),
            "agents": [],
            "count": 0,
        })),
    }
}

async fn registry_register(Json(body): Json<Value>) -> Json<Value> {
    let reg = FileBackedAgentRegistry::new(&resolve_state_root());
    let entry: crate::agent_registry::AgentEntry = match serde_json::from_value(body) {
        Ok(e) => e,
        Err(e) => {
            return Json(json!({"ok": false, "error": format!("invalid entry: {e}")}));
        }
    };
    match reg.register(entry) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

async fn registry_deregister(Json(body): Json<Value>) -> Json<Value> {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    let reg = FileBackedAgentRegistry::new(&resolve_state_root());
    match reg.deregister(id) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

async fn registry_reap() -> Json<Value> {
    let reg = FileBackedAgentRegistry::new(&resolve_state_root());
    match reg.reap_dead() {
        Ok(count) => Json(json!({"ok": true, "reaped": count})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

// ---------------------------------------------------------------------------
// Build Lock API (#337)
// ---------------------------------------------------------------------------

async fn build_lock_status() -> Json<Value> {
    let bl = BuildLock::new(&resolve_state_root());
    Json(json!({
        "locked": bl.is_locked(),
        "holder": bl.current_holder(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn build_lock_force_release() -> Json<Value> {
    let bl = BuildLock::new(&resolve_state_root());
    match bl.force_release() {
        Ok(was_locked) => Json(json!({"ok": true, "was_locked": was_locked})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

// ---------------------------------------------------------------------------
// Memory metrics panel
// ---------------------------------------------------------------------------

async fn memory_metrics() -> Json<Value> {
    let state_root = resolve_state_root();

    let memory_path = state_root.join("memory_records.json");
    let evidence_path = state_root.join("evidence_records.json");
    let goal_path = state_root.join("goal_records.json");
    let handoff_path = state_root.join("latest_handoff.json");

    let memory_info = file_metrics(&memory_path);
    let evidence_info = file_metrics(&evidence_path);
    let goal_info = file_metrics(&goal_path);
    let handoff_info = file_metrics(&handoff_path);

    let fact_count = count_json_records(&memory_path);
    let evidence_count = count_json_records(&evidence_path);
    let goal_count = count_json_records(&goal_path);

    // Query NativeCognitiveMemory (LadybugDB) for live statistics (#419).
    // Capture the error so the dashboard can show *why* data is missing
    // instead of silently returning zeros.
    let native_result =
        NativeCognitiveMemory::open_read_only(&state_root).and_then(|mem| mem.get_statistics());
    let native_error = native_result.as_ref().err().map(|e| e.to_string());
    let native_stats = native_result.ok();

    let last_consolidation = [&memory_path, &evidence_path, &goal_path]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter_map(|m| m.modified().ok())
        .max()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });

    // Use LadybugDB counts when available; JSON file counts are the legacy source.
    let total = native_stats
        .as_ref()
        .map(|s| s.total())
        .unwrap_or(fact_count + evidence_count + goal_count);

    let db_path = state_root.join("cognitive_memory.ladybug");

    Json(json!({
        "state_root": state_root.to_string_lossy(),
        "memory_records": {
            "path": memory_path.to_string_lossy().to_string(),
            "count": fact_count,
            "size_bytes": memory_info.0,
            "modified": memory_info.1,
        },
        "evidence_records": {
            "path": evidence_path.to_string_lossy().to_string(),
            "count": evidence_count,
            "size_bytes": evidence_info.0,
            "modified": evidence_info.1,
        },
        "goal_records": {
            "path": goal_path.to_string_lossy().to_string(),
            "count": goal_count,
            "size_bytes": goal_info.0,
            "modified": goal_info.1,
        },
        "handoff": {
            "path": handoff_path.to_string_lossy().to_string(),
            "size_bytes": handoff_info.0,
            "modified": handoff_info.1,
        },
        "native_memory": native_stats.as_ref().map(|s| json!({
            "sensory": s.sensory_count,
            "working": s.working_count,
            "episodic": s.episodic_count,
            "semantic": s.semantic_count,
            "procedural": s.procedural_count,
            "prospective": s.prospective_count,
            "total": s.total(),
        })),
        "native_memory_error": native_error,
        "native_memory_db_path": db_path.to_string_lossy(),
        "native_memory_db_exists": db_path.exists(),
        "total_facts": total,
        "last_consolidation": last_consolidation,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn ooda_thinking() -> Json<Value> {
    let state_root = resolve_state_root();
    let dir = state_root.join("cycle_reports");
    let mut reports = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        paths.sort_by(|a, b| {
            let num = |p: &std::fs::DirEntry| -> u32 {
                p.file_name()
                    .to_str()
                    .unwrap_or("")
                    .strip_prefix("cycle_")
                    .unwrap_or("")
                    .strip_suffix(".json")
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(0)
            };
            num(b).cmp(&num(a))
        });

        for entry in paths.into_iter().take(20) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                if let Ok(val) = serde_json::from_str::<Value>(&content) {
                    reports.push(val);
                } else {
                    // Legacy one-line summary
                    let cycle_num = entry
                        .file_name()
                        .to_str()
                        .unwrap_or("")
                        .strip_prefix("cycle_")
                        .unwrap_or("")
                        .strip_suffix(".json")
                        .unwrap_or("")
                        .parse::<u32>()
                        .unwrap_or(0);
                    reports.push(json!({
                        "cycle_number": cycle_num,
                        "summary": content.trim(),
                        "legacy": true,
                    }));
                }
            }
        }
    }

    Json(json!({ "reports": reports }))
}

fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}

fn file_metrics(path: &std::path::Path) -> (u64, Option<String>) {
    match std::fs::metadata(path) {
        Ok(m) => {
            let size = m.len();
            let modified = m.modified().ok().map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.to_rfc3339()
            });
            (size, modified)
        }
        Err(_) => (0, None),
    }
}

fn count_json_records(path: &std::path::Path) -> u64 {
    let Ok(content) = std::fs::read_to_string(path) else {
        return 0;
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Array(arr)) => arr.len() as u64,
        Ok(Value::Object(map)) => map.len() as u64,
        _ => 0,
    }
}

async fn disk_usage_pct() -> Option<u8> {
    let output = tokio::process::Command::new("df")
        .args(["--output=pcent", "/home"])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    line.trim().trim_end_matches('%').parse().ok()
}

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard — Login</title>
  <style>
    :root { --bg: #0d1117; --fg: #c9d1d9; --accent: #58a6ff; --card: #161b22; --border: #30363d; }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: var(--bg); color: var(--fg); display: flex; align-items: center; justify-content: center; min-height: 100vh; }
    .login-card { background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 2rem; width: 340px; text-align: center; }
    h1 { color: var(--accent); font-size: 1.3rem; margin-bottom: 0.5rem; }
    p { color: #8b949e; font-size: 0.85rem; margin-bottom: 1.5rem; }
    input { width: 100%; padding: 0.6rem; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); font-size: 1.1rem; text-align: center; letter-spacing: 0.15em; }
    input:focus { outline: none; border-color: var(--accent); }
    button { width: 100%; margin-top: 1rem; padding: 0.6rem; border: none; border-radius: 6px; background: var(--accent); color: #0d1117; font-weight: 600; font-size: 0.95rem; cursor: pointer; }
    button:hover { opacity: 0.9; }
    .error { color: #f85149; margin-top: 0.75rem; font-size: 0.85rem; display: none; }
  </style>
</head>
<body>
  <div class="login-card">
    <h1>🌲 Simard</h1>
    <p>Enter the login code from the server terminal</p>
    <form id="login-form">
      <input id="code" type="text" placeholder="code" autocomplete="off" autofocus maxlength="8">
      <button type="submit">Log in</button>
    </form>
    <div class="error" id="error">Invalid code. Check terminal output.</div>
  </div>



  <script>
    document.getElementById('login-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const code = document.getElementById('code').value;
      const r = await fetch('/api/login', { method: 'POST', headers: {'content-type':'application/json'}, body: JSON.stringify({code}) });
      if (r.ok) { window.location.href = '/'; }
      else { document.getElementById('error').style.display = 'block'; }
    });
  </script>
</body>
</html>
"#;

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard Dashboard v2</title>
  <style>
    :root { --bg:#0d1117; --fg:#c9d1d9; --accent:#58a6ff; --card:#161b22; --border:#30363d; --green:#3fb950; --yellow:#d29922; --red:#f85149; }
    *{margin:0;padding:0;box-sizing:border-box}
    body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:var(--bg);color:var(--fg)}
    header{display:flex;align-items:center;justify-content:space-between;padding:1rem 2rem;border-bottom:1px solid var(--border)}
    header h1{color:var(--accent);font-size:1.3rem}
    .tabs{display:flex;gap:0;border-bottom:1px solid var(--border);padding:0 2rem}
    .tab{padding:.6rem 1.2rem;cursor:pointer;color:#8b949e;border-bottom:2px solid transparent;font-size:.9rem}
    .tab:hover{color:var(--fg)} .tab.active{color:var(--accent);border-bottom-color:var(--accent)}
    .tab-content{display:none;padding:1.5rem 2rem} .tab-content.active{display:block}
    .grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(340px,1fr));gap:1rem}
    .card{background:var(--card);border:1px solid var(--border);border-radius:8px;padding:1.25rem}
    .card h2{color:var(--accent);font-size:1rem;margin-bottom:.75rem;border-bottom:1px solid var(--border);padding-bottom:.5rem}
    .stat{display:flex;justify-content:space-between;padding:.3rem 0}
    .stat .label{color:#8b949e} .stat .value{font-weight:600}
    .ok{color:var(--green)} .warn{color:var(--yellow)} .err{color:var(--red)}
    #issues-list{list-style:none}
    #issues-list li{padding:.3rem 0;border-bottom:1px solid var(--border)}
    #issues-list li:last-child{border-bottom:none}
    .issue-num{color:var(--accent);font-weight:600;margin-right:.5rem}
    .loading{color:#8b949e;font-style:italic}
    .log-box{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.75rem;font-family:'SF Mono','Fira Code',monospace;font-size:.8rem;max-height:500px;overflow-y:auto;white-space:pre-wrap;word-break:break-all;line-height:1.4;color:#8b949e}
    .transcript-item{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .transcript-item h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .proc-table{width:100%;border-collapse:collapse;font-size:.85rem}
    .proc-table th{text-align:left;color:#8b949e;padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table td{padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table tr:last-child td{border-bottom:none}
    #chat-messages{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.75rem;height:400px;overflow-y:auto;font-size:.9rem;margin-bottom:.75rem}
    .chat-msg{margin-bottom:.5rem} .chat-msg .role{font-weight:700;margin-right:.5rem}
    .chat-msg .role.user{color:var(--accent)} .chat-msg .role.system{color:var(--yellow)} .chat-msg .role.assistant{color:var(--green)}
    .typing-dots span{animation:blink 1.4s infinite both;font-size:1.2em}
    .typing-dots span:nth-child(2){animation-delay:.2s}
    .typing-dots span:nth-child(3){animation-delay:.4s}
    @keyframes blink{0%,80%,100%{opacity:0}40%{opacity:1}}
    #chat-send:disabled{opacity:.5;cursor:not-allowed}
    #chat-input-row{display:flex;gap:.5rem}
    #chat-input{flex:1;padding:.5rem;border:1px solid var(--border);border-radius:6px;background:var(--card);color:var(--fg);font-size:.9rem;resize:none;height:42px}
    #chat-input:focus{outline:none;border-color:var(--accent)}
    #chat-send{padding:.5rem 1.2rem;border:none;border-radius:6px;background:var(--accent);color:#0d1117;font-weight:600;cursor:pointer}
    #chat-send:hover{opacity:.9}
    .ws-status{font-size:.8rem;color:#8b949e;margin-bottom:.5rem} .ws-status.connected{color:var(--green)} .ws-status.disconnected{color:var(--red)}
    .mem-file{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .mem-file h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .badge{display:inline-block;padding:.15rem .5rem;border-radius:10px;font-size:.75rem;font-weight:600;background:#1f6feb33;color:var(--accent)}
    .btn{background:var(--accent);color:#0d1117;border:none;border-radius:4px;padding:.2rem .6rem;cursor:pointer;font-size:.8rem;float:right}
    .btn:hover{opacity:.9}
    .thinking-cycle{border:1px solid var(--border);border-radius:8px;padding:1rem;margin-bottom:1rem;background:var(--card)}
    .thinking-cycle.legacy{opacity:0.7}
    .cycle-header{display:flex;align-items:center;gap:.75rem;margin-bottom:.75rem;padding-bottom:.5rem;border-bottom:1px solid var(--border)}
    .cycle-num{font-weight:700;font-size:1rem;color:var(--accent)}
    .cycle-summary-inline{font-size:.85rem;color:#8b949e}
    .cycle-badge{font-size:.7rem;padding:2px 6px;border-radius:4px;background:#21262d;color:#8b949e}
    .phase{margin-bottom:.75rem;padding-left:1rem;border-left:3px solid var(--border)}
    .phase.observe{border-left-color:var(--accent)}
    .phase.orient{border-left-color:var(--yellow)}
    .phase.decide{border-left-color:#a371f7}
    .phase.act{border-left-color:var(--green)}
    .phase-label{font-weight:600;font-size:.9rem;margin-bottom:.3rem}
    .phase-content{font-size:.85rem;color:#c9d1d9}
    .phase-content div{margin-bottom:.2rem}
    .goal-line{padding-left:.5rem;color:#8b949e}
    .priority-line{padding-left:.5rem}
    .urgency{margin-right:.3rem}
    .outcome{padding:.4rem;border-radius:4px;margin-bottom:.3rem}
    .outcome.success{background:rgba(63,185,80,0.1)}
    .outcome.failure{background:rgba(248,81,73,0.1)}
    .outcome-detail{font-size:.8rem;color:#8b949e;margin-top:.2rem;padding-left:1rem;font-family:monospace;white-space:pre-wrap;max-height:100px;overflow-y:auto}
  </style>
</head>
<body>
  <header>
    <h1>🌲 Simard Dashboard</h1>
    <div style="display:flex;align-items:center;gap:1rem">
      <span id="header-version" style="font-size:.75rem;color:#8b949e"></span>
      <a href="https://github.com/rysweet/Simard/releases/latest" target="_blank" style="color:#3fb950;text-decoration:none;font-size:.85rem;border:1px solid #3fb950;padding:.2rem .6rem;border-radius:4px">📦 Releases</a>
      <span id="clock" style="color:#8b949e;font-size:.85rem"></span>
    </div>
  </header>
  <div class="tabs">
    <div class="tab active" data-tab="overview">Overview</div>
    <div class="tab" data-tab="goals">Goals</div>
    <div class="tab" data-tab="traces">Traces</div>
    <div class="tab" data-tab="logs">Logs</div>
    <div class="tab" data-tab="processes">Processes</div>
    <div class="tab" data-tab="memory">Memory</div>
    <div class="tab" data-tab="costs">Costs</div>
    <div class="tab" data-tab="chat">Chat</div>
    <div class="tab" data-tab="workboard">Whiteboard</div>
    <div class="tab" data-tab="thinking">🧠 Thinking</div>
  </div>

  <div class="tab-content active" id="tab-overview">
    <div class="grid">
      <div class="card"><h2>System Status</h2><div id="status"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Open Issues</h2><ul id="issues-list"><li class="loading">Loading…</li></ul></div>
      <div class="card">
        <h2>Cluster Topology <button class="btn" onclick="fetchDistributed()">Refresh</button></h2>
        <div id="cluster-topology"><span class="loading">Loading…</span></div>
      </div>
      <div class="card">
        <h2>Remote VMs</h2>
        <div id="remote-vms"><span class="loading">Loading…</span></div>
      </div>
      <div class="card">
        <h2>Azlin Hosts</h2>
        <div id="hosts-list"><span class="loading">Loading…</span></div>
        <div style="margin-top:1rem;display:flex;gap:0.5rem;align-items:center;flex-wrap:wrap">
          <input id="host-name" placeholder="VM name" style="padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px;width:12rem">
          <input id="host-rg" placeholder="Resource group" value="rysweet-linux-vm-pool" style="padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px;width:16rem">
          <button class="btn" onclick="addHost()">Add Host</button>
          <span id="host-status"></span>
        </div>
      </div>
    </div>
  </div>

  <div class="tab-content" id="tab-goals">
    <div class="card" style="margin-bottom:1rem">
      <h2>Active Goals
        <button class="btn" onclick="fetchGoals()">Refresh</button>
        <button class="btn" onclick="seedGoals()" style="margin-left:.5rem">Seed Default Goals</button>
        <button class="btn" onclick="showAddGoalForm()" style="margin-left:.5rem">+ Add Goal</button>
      </h2>
      <div id="add-goal-form" style="display:none;margin-bottom:1rem;padding:.75rem;background:var(--bg);border:1px solid var(--border);border-radius:6px">
        <div style="display:flex;gap:.5rem;margin-bottom:.5rem">
          <input id="new-goal-desc" placeholder="Goal description" style="flex:1;padding:.4rem;background:var(--card);color:var(--fg);border:1px solid var(--border);border-radius:4px">
          <select id="new-goal-type" style="padding:.4rem;background:var(--card);color:var(--fg);border:1px solid var(--border);border-radius:4px">
            <option value="active">Active</option>
            <option value="backlog">Backlog</option>
          </select>
          <input id="new-goal-priority" type="number" min="1" max="5" value="3" style="width:50px;padding:.4rem;background:var(--card);color:var(--fg);border:1px solid var(--border);border-radius:4px" placeholder="Pri">
        </div>
        <div style="display:flex;gap:.5rem">
          <button class="btn" onclick="submitGoal()">Add</button>
          <button class="btn" onclick="document.getElementById('add-goal-form').style.display='none'" style="background:#21262d">Cancel</button>
        </div>
      </div>
      <div id="goals-active"><span class="loading">Loading…</span></div>
    </div>
    <div class="card">
      <h2>Backlog</h2>
      <div id="goals-backlog"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-traces">
    <div class="card" style="margin-bottom:1rem">
      <h2>OTEL Traces <button class="btn" onclick="fetchTraces()">Refresh</button></h2>
      <div id="otel-status" style="margin-bottom:.75rem"><span class="loading">Loading…</span></div>
      <div id="trace-list" class="log-box" style="max-height:600px;overflow-y:auto"><span class="loading">Loading…</span></div>
    </div>
    <div class="card">
      <h2>Setup</h2>
      <p style="color:#8b949e;font-size:.85rem">To enable full OTEL tracing, set <code>OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317</code> and run an OTEL collector (e.g. Jaeger, Grafana Tempo).</p>
      <p style="color:#8b949e;font-size:.85rem;margin-top:.5rem">For systemd: <code>systemctl --user edit simard-ooda</code> and add the env var in an [Service] override.</p>
    </div>
  </div>

  <div class="tab-content" id="tab-logs">
    <div class="card" style="margin-bottom:1rem">
      <h2>Daemon Log <button class="btn" onclick="fetchLogs()">Refresh</button> <button class="btn" onclick="copyLogContent('daemon-log')" style="margin-left:.3rem">📋 Copy</button></h2>
      <div style="margin-bottom:.5rem;display:flex;gap:.5rem;align-items:center">
        <input id="log-filter" placeholder="Filter logs…" style="flex:1;padding:4px 8px;background:var(--bg);border:1px solid var(--border);color:var(--fg);border-radius:4px;font-size:.85rem">
        <select id="log-level-filter" style="padding:4px;background:var(--bg);border:1px solid var(--border);color:var(--fg);border-radius:4px;font-size:.85rem">
          <option value="">All levels</option>
          <option value="error">Errors</option>
          <option value="warn">Warnings</option>
          <option value="info">Info</option>
        </select>
        <span id="log-line-count" style="color:#8b949e;font-size:.8rem"></span>
      </div>
      <div id="daemon-log" class="log-box"><span class="loading">Loading…</span></div>
    </div>
    <div class="card" style="margin-bottom:1rem">
      <h2>Cost Ledger <button class="btn" onclick="copyLogContent('cost-log-box')">📋 Copy</button></h2>
      <div id="cost-log-box" class="log-box" style="max-height:200px"><span class="loading">Loading…</span></div>
    </div>
    <h2 style="color:var(--accent);font-size:1rem;margin-bottom:.5rem">OODA Transcripts</h2>
    <div id="ooda-transcripts"><span class="loading">Loading…</span></div>
    <h2 style="color:var(--accent);font-size:1rem;margin:.75rem 0 .5rem">Terminal Session Transcripts</h2>
    <div id="terminal-transcripts"><span class="loading">Loading…</span></div>
  </div>

  <div class="tab-content" id="tab-processes">
    <div class="card">
      <h2>Active Simard Processes <button class="btn" onclick="fetchProcesses()">Refresh</button> <span id="proc-auto-refresh" style="font-size:.75rem;color:#8b949e;font-weight:normal;margin-left:.5rem">⟳ auto-refreshing</span></h2>
      <div id="proc-count" style="margin-bottom:.5rem;color:#8b949e;font-size:.85rem"></div>
      <div id="proc-table"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-memory">
    <div style="display:flex;align-items:center;gap:1rem;margin-bottom:1rem">
      <button id="mem-view-graph" class="btn" style="opacity:1" onclick="setMemView('graph')">Graph View</button>
      <button id="mem-view-search" class="btn" style="opacity:.5" onclick="setMemView('search')">Search View</button>
      <span id="mem-graph-stats" style="color:#8b949e;font-size:.8rem;margin-left:auto"></span>
    </div>

    <div id="mem-graph-panel">
      <div class="card" style="margin-bottom:1rem;padding:.75rem">
        <div style="display:flex;gap:1rem;flex-wrap:wrap;align-items:center;font-size:.8rem">
          <label style="color:#f0883e"><input type="checkbox" class="mem-filter" data-type="WorkingMemory" checked> Working</label>
          <label style="color:#58a6ff"><input type="checkbox" class="mem-filter" data-type="SemanticFact" checked> Semantic</label>
          <label style="color:#3fb950"><input type="checkbox" class="mem-filter" data-type="EpisodicMemory" checked> Episodic</label>
          <label style="color:#a371f7"><input type="checkbox" class="mem-filter" data-type="ProceduralMemory" checked> Procedural</label>
          <label style="color:#d29922"><input type="checkbox" class="mem-filter" data-type="ProspectiveMemory" checked> Prospective</label>
          <label style="color:#8b949e"><input type="checkbox" class="mem-filter" data-type="SensoryBuffer" checked> Sensory</label>
          <button class="btn" onclick="fetchMemoryGraph()" style="margin-left:auto">Refresh</button>
        </div>
      </div>
      <div style="display:flex;gap:1rem">
        <div class="card" style="flex:1;padding:0;position:relative;min-height:500px">
          <canvas id="mem-graph-canvas" style="width:100%;height:500px;display:block;cursor:grab"></canvas>
          <div id="mem-graph-tooltip" style="display:none;position:absolute;background:#161b22;border:1px solid #30363d;border-radius:6px;padding:.5rem .75rem;font-size:.8rem;max-width:320px;pointer-events:none;z-index:10;word-break:break-word"></div>
        </div>
        <div id="mem-graph-detail" class="card" style="width:280px;display:none">
          <h2 id="mg-detail-title">Node Details</h2>
          <div id="mg-detail-body"></div>
        </div>
      </div>
    </div>

    <div id="mem-search-panel" style="display:none">
      <div class="grid">
        <div class="card"><h2>Memory Overview</h2><div id="mem-overview"><span class="loading">Loading…</span></div></div>
        <div class="card"><h2>Memory Files</h2><div id="mem-files"><span class="loading">Loading…</span></div></div>
      </div>
      <div class="card" style="margin-top:1rem">
        <h2>Memory Search</h2>
        <div style="display:flex;gap:.5rem;align-items:center;margin-bottom:1rem">
          <input id="mem-search-input" placeholder="Search memories…" style="flex:1;padding:6px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px">
          <button class="btn" onclick="searchMemory()">Search</button>
        </div>
        <div id="mem-search-results"></div>
      </div>
    </div>
  </div>

  <div class="tab-content" id="tab-costs">
    <div class="grid">
      <div class="card"><h2>Daily Costs <button class="btn" onclick="fetchCosts()">Refresh</button></h2><div id="costs-daily"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Weekly Costs</h2><div id="costs-weekly"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Budget Settings</h2>
        <div style="display:flex;gap:1rem;align-items:center;flex-wrap:wrap">
          <label>Daily $<input id="budget-daily" type="number" step="0.01" style="width:8rem;padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px"></label>
          <label>Weekly $<input id="budget-weekly" type="number" step="0.01" style="width:8rem;padding:4px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px"></label>
          <button class="btn" onclick="saveBudget()">Save</button>
          <span id="budget-status"></span>
        </div>
      </div>
    </div>
  </div>

  <div class="tab-content" id="tab-thinking">
    <div class="card">
      <h2>OODA Internal Reasoning <button class="btn" onclick="fetchThinking()">Refresh</button></h2>
      <div id="thinking-timeline"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-chat">
    <div class="card" style="max-width:720px">
      <h2>Meeting Chat</h2>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.75rem;margin-bottom:1rem;font-size:.85rem;color:#8b949e">
        <strong style="color:var(--accent)">💡 Meeting Help:</strong>
        Use this chat or run <code>simard meeting &lt;topic&gt;</code> from the terminal.
        Commands: <code>/close</code> end session, <code>/goals</code> review goals, <code>/status</code> system status.
        Meetings generate handoff documents that the OODA daemon ingests as new goals.
      </div>
      <div class="ws-status disconnected" id="ws-status">● Disconnected <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Reconnect</button></div>
      <div id="chat-messages"></div>
      <div id="chat-input-row">
        <textarea id="chat-input" placeholder="Type a message… (/close to end session)"></textarea>
        <button id="chat-send" onclick="sendChat()">Send</button>
      </div>
    </div>
  </div>

  <div class="tab-content" id="tab-workboard">
    <div id="wb-header" style="display:flex;align-items:center;gap:1.5rem;margin-bottom:1rem;flex-wrap:wrap">
      <div id="wb-cycle-indicator" style="display:flex;align-items:center;gap:.5rem">
        <span id="wb-phase-dot" style="width:12px;height:12px;border-radius:50%;display:inline-block;background:#8b949e"></span>
        <span id="wb-cycle-label" style="font-weight:700;color:var(--accent)">Cycle —</span>
        <span id="wb-phase-label" style="color:#8b949e;font-size:.85rem"></span>
      </div>
      <div style="color:#8b949e;font-size:.85rem"><span id="wb-uptime">—</span> uptime</div>
      <div style="color:#8b949e;font-size:.85rem">Next cycle: <span id="wb-eta" style="color:var(--fg);font-weight:600">—</span></div>
      <button class="btn" onclick="fetchWorkboard()">Refresh</button>
    </div>

    <h3 style="color:var(--accent);margin-bottom:.5rem;font-size:.95rem">Goals</h3>
    <div id="wb-kanban" style="display:grid;grid-template-columns:repeat(4,1fr);gap:.75rem;margin-bottom:1.25rem">
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">Queued</h2><div id="wb-col-queued"></div></div>
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">In Progress</h2><div id="wb-col-inprogress"></div></div>
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">Blocked</h2><div id="wb-col-blocked"></div></div>
      <div class="card" style="min-height:80px"><h2 style="font-size:.85rem">Done</h2><div id="wb-col-done"></div></div>
    </div>

    <div class="grid" style="margin-bottom:1.25rem">
      <div class="card">
        <h2>Active Engineers</h2>
        <div id="wb-engineers"><span style="color:#8b949e">No spawned engineers</span></div>
      </div>
      <div class="card">
        <h2>Recent Actions</h2>
        <div id="wb-actions" style="max-height:300px;overflow-y:auto"><span style="color:#8b949e">No recent actions</span></div>
      </div>
    </div>

    <div class="card" style="margin-bottom:1.25rem">
      <h2 style="cursor:pointer" onclick="document.getElementById('wb-wm-body').style.display=document.getElementById('wb-wm-body').style.display==='none'?'block':'none'">Working Memory <span style="font-weight:normal;color:#8b949e;font-size:.8rem" id="wb-wm-count">0 slots</span> <span style="font-size:.75rem;color:#8b949e">▾</span></h2>
      <div id="wb-wm-body">
        <div id="wb-wm-list" style="font-size:.85rem;color:#8b949e">No active working memory</div>
      </div>
    </div>

    <div class="card" style="margin-bottom:1.25rem">
      <h2 style="cursor:pointer" onclick="document.getElementById('wb-facts-body').style.display=document.getElementById('wb-facts-body').style.display==='none'?'block':'none'">Task Memory <span style="font-weight:normal;color:#8b949e;font-size:.8rem" id="wb-facts-count">0 facts</span> <span style="font-size:.75rem;color:#8b949e">▾</span></h2>
      <div id="wb-facts-body">
        <div id="wb-facts-list" style="font-size:.85rem;color:#8b949e">No facts loaded</div>
      </div>
    </div>

    <div class="card">
      <h2>Cognitive Statistics</h2>
      <div id="wb-cog-stats" style="font-size:.85rem;color:#8b949e">Loading…</div>
    </div>
  </div>

  <div class="tab-content" id="tab-thinking">
    <div class="card">
      <h2>OODA Internal Reasoning <button class="btn" onclick="fetchThinking()">Refresh</button></h2>
      <div id="thinking-timeline"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <script>
    /* --- Helpers --- */
    function fmtB(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';return(b/1048576).toFixed(1)+' MB';}
    function esc(s){if(s==null)return'';const d=document.createElement('div');d.textContent=String(s);return d.innerHTML;}
    function timeAgo(ts){
      if(!ts)return'—';
      const d=new Date(ts);if(isNaN(d))return ts;
      const s=Math.floor((Date.now()-d.getTime())/1000);
      if(s<5)return'just now';if(s<60)return s+'s ago';
      const m=Math.floor(s/60);if(m<60)return m+'m ago';
      const h=Math.floor(m/60);if(h<24)return h+'h ago';
      const days=Math.floor(h/24);return days+'d ago';
    }
    function copyLogContent(id){
      const el=document.getElementById(id);if(!el)return;
      navigator.clipboard.writeText(el.textContent||'').then(
        ()=>{const prev=el.style.borderColor;el.style.borderColor='var(--green)';setTimeout(()=>el.style.borderColor=prev,800);},
        ()=>{}
      );
    }

    /* --- Active tab tracking for auto-refresh --- */
    let activeTab='overview';
    let tabRefreshTimers={};

    function clearTabTimers(){Object.values(tabRefreshTimers).forEach(clearInterval);tabRefreshTimers={};}

    /* --- Tabs --- */
    document.querySelectorAll('.tab').forEach(tab=>{
      tab.addEventListener('click',()=>{
        document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
        document.querySelectorAll('.tab-content').forEach(c=>c.classList.remove('active'));
        tab.classList.add('active');
        document.getElementById('tab-'+tab.dataset.tab).classList.add('active');
        activeTab=tab.dataset.tab;
        clearTabTimers();
        if(tab.dataset.tab==='logs') {fetchLogs();tabRefreshTimers.logs=setInterval(fetchLogs,15000);}
        if(tab.dataset.tab==='processes') {fetchProcessTree();tabRefreshTimers.proc=setInterval(fetchProcessTree,15000);}
        if(tab.dataset.tab==='memory') {fetchMemoryGraph();}

        if(tab.dataset.tab==='goals') fetchGoals();
        if(tab.dataset.tab==='costs') fetchCosts();
        if(tab.dataset.tab==='traces') fetchTraces();
        if(tab.dataset.tab==='chat') initChat();
        if(tab.dataset.tab==='workboard') {fetchWorkboard();tabRefreshTimers.wb=setInterval(fetchWorkboard,30000);}
        if(tab.dataset.tab==='thinking') {fetchThinking();tabRefreshTimers.thinking=setInterval(fetchThinking,30000);}
      });
    });
    setInterval(()=>{document.getElementById('clock').textContent=new Date().toLocaleString()},1000);

    /* --- Status --- */
    async function fetchStatus(){
      try{
        const r=await fetch('/api/status'); const d=await r.json();
        const dc=d.disk_usage_pct>90?'err':d.disk_usage_pct>70?'warn':'ok';
        const oc=d.ooda_daemon==='running'?'ok':(d.ooda_daemon==='stale'?'warn':'err');
        const shortHash=d.git_hash?d.git_hash.substring(0,7):'';
        const versionLink=d.git_hash?`<a href="https://github.com/rysweet/Simard/commit/${d.git_hash}" target="_blank" style="color:#3fb950;text-decoration:none">v${esc(d.version)}</a> (<code>${shortHash}</code>)`:`v${esc(d.version)}`;
        let healthDetail='';
        if(d.daemon_health){
          const dh=d.daemon_health;
          healthDetail=` (cycle #${dh.cycle_number??'?'}`;
          if(dh.timestamp) healthDetail+=`, ${timeAgo(dh.timestamp)}`;
          healthDetail+=')';
        }
        document.getElementById('status').innerHTML=`
          <div class="stat"><span class="label">Version</span><span class="value">${versionLink}</span></div>
          <div class="stat"><span class="label">OODA Daemon</span><span class="value ${oc}">${esc(d.ooda_daemon)}${healthDetail}</span></div>
          <div class="stat"><span class="label">Active Processes</span><span class="value">${d.active_processes??0}</span></div>
          <div class="stat"><span class="label">Disk Usage</span><span class="value ${dc}">${d.disk_usage_pct??'?'}%</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.timestamp)}</span></div>`;
        document.getElementById('header-version').textContent='v'+d.version+' ('+shortHash+')';
      }catch(e){document.getElementById('status').innerHTML='<span class="err">Failed to reach /api/status — is the dashboard server running?</span>';}
    }

    /* --- Issues --- */
    async function fetchIssues(){
      try{
        const r=await fetch('/api/issues'); const data=await r.json();
        if(Array.isArray(data)){
          if(!data.length){document.getElementById('issues-list').innerHTML='<li style="color:#8b949e">No open issues 🎉</li>';return;}
          document.getElementById('issues-list').innerHTML=data.map(i=>{
            const labels=(i.labels||[]).map(l=>`<span class="badge" style="margin-left:.3rem">${esc(l.name||l)}</span>`).join('');
            return`<li><span class="issue-num">#${i.number}</span>${esc(i.title)}${labels}</li>`;
          }).join('');
        }else if(data.error){
          document.getElementById('issues-list').innerHTML=`<li class="warn">${esc(data.error)} — is <code>gh</code> authenticated?</li>`;
        }
      }catch(e){document.getElementById('issues-list').innerHTML='<li class="err">Failed to load issues — check network</li>';}
    }

    /* --- Logs --- */
    let allLogLines=[];
    async function fetchLogs(){
      try{
        const r=await fetch('/api/logs'); const d=await r.json();
        allLogLines=d.daemon_log_lines||[];
        applyLogFilter();
        const tEl=document.getElementById('ooda-transcripts');
        if(d.ooda_transcripts?.length){
          tEl.innerHTML=d.ooda_transcripts.map(t=>`
            <div class="transcript-item">
              <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
              <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
            </div>`).join('');
        }else{tEl.innerHTML='<span style="color:#8b949e">No OODA transcripts found in state root.</span>';}
        // Render cycle reports
        const crEl=document.getElementById('cycle-reports');
        if(d.cycle_reports?.length){
          crEl.innerHTML=d.cycle_reports.map(c=>{
            const num=c.cycle_number;
            const text=c.summary||JSON.stringify(c.report||{});
            return`<div class="transcript-item">
              <h3>Cycle #${num}</h3>
              <div class="log-box" style="max-height:100px">${esc(text)}</div>
            </div>`;
          }).join('');
        }else{crEl.innerHTML='<span style="color:#8b949e">No cycle reports found. Run the OODA daemon to generate cycle data.</span>';}
        const ttEl=document.getElementById('terminal-transcripts');
        if(d.terminal_transcripts?.length){
          ttEl.innerHTML=d.terminal_transcripts.map(t=>`
            <div class="transcript-item">
              <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
              <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
            </div>`).join('');
        }else{ttEl.innerHTML='<span style="color:#8b949e">No terminal session transcripts found.</span>';}
        const costEl=document.getElementById('cost-log-box');
        if(d.cost_log_lines?.length){
          costEl.textContent=d.cost_log_lines.join('\n');
          costEl.scrollTop=costEl.scrollHeight;
        }else{costEl.innerHTML='<span style="color:#8b949e">No cost ledger entries</span>';}
      }catch(e){document.getElementById('daemon-log').textContent='Failed to load logs — check /api/logs endpoint';}
    }
    function applyLogFilter(){
      const filter=(document.getElementById('log-filter')?.value||'').toLowerCase();
      const level=(document.getElementById('log-level-filter')?.value||'').toLowerCase();
      let lines=allLogLines;
      if(filter) lines=lines.filter(l=>l.toLowerCase().includes(filter));
      if(level) lines=lines.filter(l=>l.toLowerCase().includes(level));
      const el=document.getElementById('daemon-log');
      el.textContent=lines.length?lines.join('\n'):'(no matching log lines)';
      el.scrollTop=el.scrollHeight;
      const countEl=document.getElementById('log-line-count');
      if(countEl) countEl.textContent=`${lines.length}/${allLogLines.length} lines`;
    }
    document.getElementById('log-filter')?.addEventListener('input',applyLogFilter);
    document.getElementById('log-level-filter')?.addEventListener('change',applyLogFilter);

    /* --- Process Tree --- */
    function renderTreeNode(node, isLast, depth) {
      if (!node) return '';
      const hasChildren = node.children && node.children.length > 0;
      const toggleCls = hasChildren ? 'proc-toggle' : 'proc-toggle leaf';
      const toggleChar = hasChildren ? '▼' : '·';
      const stateClass = (node.state || 'unknown').replace(/\s+/g, '-');
      const cmdDisplay = esc(node.command || '').length > 80
        ? esc(node.command).substring(0, 77) + '…'
        : esc(node.command || '');
      let html = `<div class="proc-node" data-pid="${node.pid}">
        <div class="proc-row">
          <span class="${toggleCls}" onclick="toggleProcChildren(this)">${toggleChar}</span>
          <span class="proc-pid">${node.pid}</span>
          <span class="proc-state ${stateClass}">${esc(node.state)}</span>
          <span class="proc-cpu">${node.cpu_pct?.toFixed(1) ?? '—'}%</span>
          <span class="proc-mem">${node.memory_mb != null ? node.memory_mb.toFixed(1) + 'M' : '—'}</span>
          <span class="proc-cmd" title="${esc(node.command)}">${cmdDisplay}</span>
        </div>`;
      if (hasChildren) {
        html += '<div class="proc-children">';
        node.children.forEach((child, i) => {
          html += renderTreeNode(child, i === node.children.length - 1, depth + 1);
        });
        html += '</div>';
      }
      html += '</div>';
      return html;
    }

    function toggleProcChildren(el) {
      const node = el.closest('.proc-node');
      const childDiv = node.querySelector(':scope > .proc-children');
      if (!childDiv) return;
      const collapsed = childDiv.classList.toggle('collapsed');
      el.textContent = collapsed ? '▶' : '▼';
    }

    async function fetchProcessTree() {
      try {
        const r = await fetch('/api/process-tree');
        const d = await r.json();
        const container = document.getElementById('proc-tree-container');
        const summary = document.getElementById('proc-tree-summary');
        if (d.root) {
          summary.textContent = `${d.total_processes} process(es) · ${d.total_memory_mb} MB total — updated ${timeAgo(d.timestamp)}`;
          container.innerHTML = '<div class="proc-tree">' + renderTreeNode(d.root, true, 0) + '</div>';
        } else {
          summary.textContent = `Updated ${timeAgo(d.timestamp)}`;
          container.innerHTML = '<span style="color:#8b949e">No process tree available. Is the daemon running?</span>';
        }
      } catch(e) {
        document.getElementById('proc-tree-container').innerHTML = '<span class="err">Failed to load process tree</span>';
      }
    }

    /* --- Memory --- */
    async function fetchMemory(){
      try{
        const r=await fetch('/api/memory'); const d=await r.json();
        let overviewHtml=`
          <div class="stat"><span class="label">Total Facts</span><span class="value">${d.total_facts}</span></div>
          <div class="stat"><span class="label">Last Consolidation</span><span class="value">${d.last_consolidation?timeAgo(d.last_consolidation)+' ('+new Date(d.last_consolidation).toLocaleString()+')':'Never'}</span></div>
          <div class="stat"><span class="label">State Root</span><span class="value" style="font-size:.8rem;word-break:break-all">${esc(d.state_root)}</span></div>`;
        if(d.native_memory){
          const nm=d.native_memory;
          overviewHtml+=`
          <h3 style="color:var(--accent);font-size:.9rem;margin-top:.75rem;border-top:1px solid var(--border);padding-top:.5rem">LadybugDB (Native Memory)</h3>
          <div class="stat"><span class="label">Sensory</span><span class="value">${nm.sensory}</span></div>
          <div class="stat"><span class="label">Working</span><span class="value">${nm.working}</span></div>
          <div class="stat"><span class="label">Episodic</span><span class="value">${nm.episodic}</span></div>
          <div class="stat"><span class="label">Semantic (Facts)</span><span class="value">${nm.semantic}</span></div>
          <div class="stat"><span class="label">Procedural</span><span class="value">${nm.procedural}</span></div>
          <div class="stat"><span class="label">Prospective</span><span class="value">${nm.prospective}</span></div>
          <div class="stat"><span class="label"><strong>Total Native</strong></span><span class="value"><strong>${nm.total}</strong></span></div>`;
        }
        document.getElementById('mem-overview').innerHTML=overviewHtml;
        const files=[
          {key:'memory_records',label:'Memory Records'},
          {key:'evidence_records',label:'Evidence Records'},
          {key:'goal_records',label:'Goal Records'},
          {key:'handoff',label:'Latest Handoff'}];
        document.getElementById('mem-files').innerHTML=files.map(f=>{
          const info=d[f.key]||{};
          const modStr=info.modified?timeAgo(info.modified):'N/A';
          return`<div class="mem-file">
            <h3>${f.label} ${info.count!==undefined?'<span class="badge">'+info.count+' records</span>':''} <span class="badge">${fmtB(info.size_bytes||0)}</span></h3>
            <div class="stat"><span class="label">Modified</span><span class="value">${modStr}</span></div>
          </div>`;}).join('');
      }catch(e){document.getElementById('mem-overview').innerHTML='<span class="err">Failed to load memory data — check state root path</span>';}
    }

    /* --- Distributed --- */
    async function fetchDistributed(){
      document.getElementById('cluster-topology').innerHTML='<span class="loading">Querying remote VMs… (this may take 10-30s)</span>';
      try{
        const r=await fetch('/api/distributed'); const d=await r.json();
        document.getElementById('cluster-topology').innerHTML=`
          <div class="stat"><span class="label">Topology</span><span class="value">${esc(d.topology)}</span></div>
          <div class="stat"><span class="label">Local Host</span><span class="value">${esc(d.local?.hostname||'?')}</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.timestamp)}</span></div>`;
        if(d.remote_vms?.length){
          document.getElementById('remote-vms').innerHTML=d.remote_vms.map(vm=>{
            const sc=vm.status==='reachable'?'ok':(vm.status==='unreachable'?'err':'warn');
            return`<div style="border:1px solid var(--border);border-radius:6px;padding:1rem;margin-bottom:.75rem">
              <h3 style="margin:0 0 .5rem 0;color:var(--accent)">${esc(vm.vm_name)} <span class="${sc}" style="font-size:.85rem">${esc(vm.status)}</span></h3>
              ${vm.hostname?`<div class="stat"><span class="label">Hostname</span><span class="value">${esc(vm.hostname)}</span></div>`:''}
              ${vm.uptime?`<div class="stat"><span class="label">Uptime</span><span class="value">${esc(vm.uptime)}</span></div>`:''}
              ${vm.load_avg?`<div class="stat"><span class="label">Load</span><span class="value">${esc(vm.load_avg)}</span></div>`:''}
              ${vm.memory_mb?`<div class="stat"><span class="label">Memory</span><span class="value">${esc(vm.memory_mb)} MB</span></div>`:''}
              ${vm.disk_root_pct!=null?`<div class="stat"><span class="label">Root Disk</span><span class="value ${vm.disk_root_pct>90?'err':vm.disk_root_pct>70?'warn':'ok'}">${vm.disk_root_pct}%</span></div>`:''}
              ${vm.disk_data_pct!=null?`<div class="stat"><span class="label">Data Disk</span><span class="value">${vm.disk_data_pct}%</span></div>`:''}
              ${vm.disk_tmp_pct!=null?`<div class="stat"><span class="label">Tmp Disk</span><span class="value">${vm.disk_tmp_pct}%</span></div>`:''}
              ${vm.simard_processes!=null?`<div class="stat"><span class="label">Simard Processes</span><span class="value">${vm.simard_processes}</span></div>`:''}
              ${vm.cargo_processes!=null?`<div class="stat"><span class="label">Cargo Processes</span><span class="value">${vm.cargo_processes}</span></div>`:''}
              ${vm.error?`<div class="stat"><span class="label">Error</span><span class="value err">${esc(vm.error)}</span></div>`:''}
            </div>`;}).join('');
        }else{document.getElementById('remote-vms').innerHTML='<span style="color:#8b949e">No remote VMs configured. Add hosts below.</span>';}
      }catch(e){document.getElementById('cluster-topology').innerHTML='<span class="err">Failed to query distributed status — check network and azlin</span>';}
    }
    async function fetchHosts(){
      try{
        const r=await fetch('/api/hosts');const d=await r.json();
        const el=document.getElementById('hosts-list');
        if(!d.hosts?.length){el.innerHTML='<span style="color:#8b949e">No hosts configured. Add a VM name below.</span>';return;}
        el.innerHTML=d.hosts.map(h=>{
          const name=esc(h.name||'');
          return`<div style="display:flex;align-items:center;gap:0.5rem;padding:4px 0;border-bottom:1px solid var(--border)">
            <span style="flex:1"><strong>${name}</strong> <span style="color:#8b949e">(${esc(h.resource_group||'default')})</span> <span style="color:#8b949e;font-size:.75rem">${timeAgo(h.added_at)}</span></span>
            <button class="btn" style="padding:2px 8px;font-size:.8rem" data-host="${name}">Remove</button>
          </div>`;
        }).join('');
        el.querySelectorAll('button[data-host]').forEach(btn=>{
          btn.addEventListener('click',()=>removeHost(btn.dataset.host));
        });
      }catch(e){}
    }
    async function addHost(){
      const name=document.getElementById('host-name').value.trim();
      const rg=document.getElementById('host-rg').value.trim();
      if(!name){document.getElementById('host-status').textContent='Name required';return;}
      try{
        const r=await fetch('/api/hosts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name,resource_group:rg})});
        const d=await r.json();
        document.getElementById('host-status').textContent=d.status==='ok'?'Added ✓':'Error: '+(d.error||'');
        document.getElementById('host-name').value='';
        fetchHosts();
        setTimeout(()=>document.getElementById('host-status').textContent='',3000);
      }catch(e){document.getElementById('host-status').textContent='Network error';}
    }
    async function removeHost(name){
      if(!confirm('Remove host "'+name+'"?'))return;
      await fetch('/api/hosts',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({name})});
      fetchHosts();
    }
    fetchHosts();

    /* --- Goals --- */
    async function fetchGoals(){
      try{
        const r=await fetch('/api/goals'); const d=await r.json();
        if(d.active?.length){
          document.getElementById('goals-active').innerHTML=`<table class="proc-table">
            <tr><th>Priority</th><th>ID</th><th>Description</th><th>Status</th><th>Assigned</th><th>Actions</th></tr>
            ${d.active.map(g=>`<tr>
              <td style="text-align:center">${g.priority??'—'}</td>
              <td><code>${esc(g.id)}</code></td>
              <td>${esc(g.description)}</td>
              <td>${esc(g.status)}</td>
              <td>${g.assigned_to?esc(g.assigned_to):'—'}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="removeGoal('${esc(g.id)}')">✕</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="updateGoalStatus('${esc(g.id)}')">Status</button>
              </td>
            </tr>`).join('')}
          </table>
          <div style="margin-top:.5rem;color:#8b949e;font-size:.8rem">${d.active_count} active goal(s)</div>`;
        }else{document.getElementById('goals-active').innerHTML='<span style="color:#8b949e">No active goals. Use "Seed Default Goals" or run the OODA daemon to generate goals from meetings.</span>';}
        if(d.backlog?.length){
          document.getElementById('goals-backlog').innerHTML=`<table class="proc-table">
            <tr><th>ID</th><th>Description</th><th>Source</th><th>Score</th><th>Actions</th></tr>
            ${d.backlog.map(b=>`<tr>
              <td><code>${esc(b.id)}</code></td>
              <td>${esc(b.description)}</td>
              <td>${esc(b.source||'')}</td>
              <td>${b.score??'—'}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="promoteGoal('${esc(b.id)}')">▲ Promote</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="removeGoal('${esc(b.id)}')">✕</button>
              </td>
            </tr>`).join('')}
          </table>`;
        }else{document.getElementById('goals-backlog').innerHTML='<span style="color:#8b949e">No backlog items</span>';}
      }catch(e){document.getElementById('goals-active').innerHTML='<span class="err">Failed to load goals — check state root</span>';}
    }

    async function seedGoals(){
      if(!confirm('Seed default goals? This only works if no active goals exist.'))return;
      try{
        const r=await fetch('/api/goals/seed',{method:'POST'});
        const d=await r.json();
        if(d.status==='ok'||d.status==='already_seeded'){
          fetchGoals();
        }else{
          alert('Seed failed: '+(d.error||'unknown'));
        }
      }catch(e){alert('Seed failed: '+e);}
    }

    function showAddGoalForm(){document.getElementById('add-goal-form').style.display='block';document.getElementById('new-goal-desc').focus();}

    async function submitGoal(){
      const desc=document.getElementById('new-goal-desc').value.trim();
      if(!desc){alert('Description required');return;}
      const type=document.getElementById('new-goal-type').value;
      const priority=parseInt(document.getElementById('new-goal-priority').value)||3;
      try{
        const r=await fetch('/api/goals',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({description:desc,type:type,priority:priority})});
        const d=await r.json();
        if(d.status==='ok'){document.getElementById('add-goal-form').style.display='none';document.getElementById('new-goal-desc').value='';fetchGoals();}
        else{alert(d.error||'Failed');}
      }catch(e){alert('Error: '+e);}
    }

    async function removeGoal(id){
      if(!confirm('Remove goal "'+id+'"?'))return;
      try{
        const r=await fetch('/api/goals/'+encodeURIComponent(id),{method:'DELETE'});
        const d=await r.json();
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function promoteGoal(id){
      try{
        const r=await fetch('/api/goals/promote/'+encodeURIComponent(id),{method:'POST'});
        const d=await r.json();
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function updateGoalStatus(id){
      const status=prompt('New status (not-started, in-progress, blocked, completed):');
      if(!status)return;
      try{
        const r=await fetch('/api/goals/'+encodeURIComponent(id)+'/status',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({status:status})});
        const d=await r.json();
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    /* --- Traces --- */
    async function fetchTraces(){
      try{
        const r=await fetch('/api/traces'); const d=await r.json();
        const status=d.otel_enabled
          ?`<span class="ok">OTEL enabled</span> → <code>${esc(d.otel_endpoint||'')}</code>`
          :'<span class="warn">OTEL not configured</span> — set OTEL_EXPORTER_OTLP_ENDPOINT to enable';
        document.getElementById('otel-status').innerHTML=`
          <div class="stat"><span class="label">OTEL Status</span><span class="value">${status}</span></div>
          <div class="stat"><span class="label">Collected Entries</span><span class="value">${d.span_count}</span></div>`;
        if(d.spans?.length){
          document.getElementById('trace-list').innerHTML=d.spans.map(s=>{
            const data=s.data;
            const ts=data.timestamp||data.__REALTIME_TIMESTAMP||data._SOURCE_REALTIME_TIMESTAMP||'';
            const msg=data.MESSAGE||data.message||data.description||data.model||JSON.stringify(data).substring(0,200);
            return`<div style="border-bottom:1px solid var(--border);padding:4px 0;font-size:.82rem">
              <span style="color:#8b949e">[${esc(s.source)}]</span>
              ${ts?'<span style="color:var(--accent);margin:0 .5rem">'+esc(String(ts).substring(0,19))+'</span>':''}
              <span>${esc(String(msg))}</span>
            </div>`;
          }).join('');
        }else{document.getElementById('trace-list').innerHTML='<span style="color:#8b949e">No trace data yet. Run the OODA daemon or make API calls to generate traces.</span>';}
      }catch(e){document.getElementById('trace-list').innerHTML='<span class="err">Failed to load traces — check /api/traces</span>';}
    }

    /* --- Memory Search --- */
    async function searchMemory(){
      const q=document.getElementById('mem-search-input').value.trim();
      if(!q){document.getElementById('mem-search-results').innerHTML='<span class="warn">Enter a search term</span>';return;}
      document.getElementById('mem-search-results').innerHTML='<span class="loading">Searching…</span>';
      try{
        const r=await fetch('/api/memory/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:q})});
        const d=await r.json();
        if(d.results?.length){
          document.getElementById('mem-search-results').innerHTML=`
            <p style="color:#8b949e;font-size:.85rem">${d.result_count} result(s) for "${esc(d.query)}"</p>
            ${d.results.map(sr=>`<div style="border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem">
              <span class="badge">${esc(sr.source)}</span>
              <pre style="margin:.5rem 0 0;white-space:pre-wrap;font-size:.8rem;color:var(--fg)">${esc(JSON.stringify(sr.data,null,2).substring(0,500))}</pre>
            </div>`).join('')}`;
        }else{
          document.getElementById('mem-search-results').innerHTML=`<span style="color:#8b949e">No results for "${esc(q)}" — try broader terms</span>`;
        }
      }catch(e){document.getElementById('mem-search-results').innerHTML='<span class="err">Search failed — check /api/memory/search</span>';}
    }
    document.getElementById('mem-search-input')?.addEventListener('keypress',e=>{if(e.key==='Enter')searchMemory();});

    /* --- Memory Graph Visualization --- */
    let mgNodes=[],mgEdges=[],mgFiltered=[],mgFilteredEdges=[];
    let mgDrag=null,mgPinned=null;
    let mgOffX=0,mgOffY=0,mgScale=1,mgPanX=0,mgPanY=0;
    const mgColors={WorkingMemory:'#f0883e',SemanticFact:'#58a6ff',EpisodicMemory:'#3fb950',ProceduralMemory:'#a371f7',ProspectiveMemory:'#d29922',SensoryBuffer:'#8b949e'};

    function setMemView(v){
      document.getElementById('mem-graph-panel').style.display=v==='graph'?'block':'none';
      document.getElementById('mem-search-panel').style.display=v==='search'?'block':'none';
      document.getElementById('mem-view-graph').style.opacity=v==='graph'?'1':'.5';
      document.getElementById('mem-view-search').style.opacity=v==='search'?'1':'.5';
      if(v==='graph') fetchMemoryGraph();
      if(v==='search') fetchMemory();
    }

    function mgApplyFilters(){
      const checks={};
      document.querySelectorAll('.mem-filter').forEach(cb=>{checks[cb.dataset.type]=cb.checked;});
      mgFiltered=mgNodes.filter(n=>checks[n.type]!==false);
      const ids=new Set(mgFiltered.map(n=>n.id));
      mgFilteredEdges=mgEdges.filter(e=>ids.has(e.source)&&ids.has(e.target));
      mgRender();
    }
    document.querySelectorAll('.mem-filter').forEach(cb=>cb.addEventListener('change',mgApplyFilters));

    async function fetchMemoryGraph(){
      try{
        const r=await fetch('/api/memory/graph');const d=await r.json();
        if(d.error){document.getElementById('mem-graph-stats').textContent='Error: '+d.error;return;}
        const s=d.stats||{};
        document.getElementById('mem-graph-stats').textContent=
          'W:'+(s.working||0)+' S:'+(s.semantic||0)+' E:'+(s.episodic||0)+' P:'+(s.procedural||0)+' Pr:'+(s.prospective||0)+' Se:'+(s.sensory||0);
        mgNodes=(d.nodes||[]);mgEdges=(d.edges||[]);
        mgInitLayout();mgApplyFilters();mgSimulate();
      }catch(e){document.getElementById('mem-graph-stats').textContent='Load failed';}
    }

    function mgInitLayout(){
      const canvas=document.getElementById('mem-graph-canvas');
      const w=canvas.clientWidth||800,h=canvas.clientHeight||500;
      mgPanX=0;mgPanY=0;mgScale=1;
      const n=mgNodes.length||1;
      mgNodes.forEach((nd,i)=>{
        const angle=(2*Math.PI*i)/n;
        const radius=Math.min(w,h)*0.3;
        nd.x=w/2+radius*Math.cos(angle);
        nd.y=h/2+radius*Math.sin(angle);
        nd.vx=0;nd.vy=0;nd.pinned=false;
      });
    }

    function mgSimulate(){
      const canvas=document.getElementById('mem-graph-canvas');
      const dt=0.3,repulsion=800,springLen=100,springK=0.02,gravity=0.01,damping=0.85;
      const cx=(canvas.clientWidth||800)/2,cy=(canvas.clientHeight||500)/2;
      for(let iter=0;iter<120;iter++){
        for(let i=0;i<mgFiltered.length;i++){
          if(mgFiltered[i].pinned)continue;
          let fx=0,fy=0;
          for(let j=0;j<mgFiltered.length;j++){
            if(i===j)continue;
            let dx=mgFiltered[i].x-mgFiltered[j].x,dy=mgFiltered[i].y-mgFiltered[j].y;
            let dist=Math.sqrt(dx*dx+dy*dy)||1;
            let f=repulsion/(dist*dist);
            fx+=f*dx/dist;fy+=f*dy/dist;
          }
          fx+=(cx-mgFiltered[i].x)*gravity;
          fy+=(cy-mgFiltered[i].y)*gravity;
          mgFiltered[i].vx=(mgFiltered[i].vx+fx*dt)*damping;
          mgFiltered[i].vy=(mgFiltered[i].vy+fy*dt)*damping;
          mgFiltered[i].x+=mgFiltered[i].vx*dt;
          mgFiltered[i].y+=mgFiltered[i].vy*dt;
        }
        const nodeMap={};mgFiltered.forEach(n=>{nodeMap[n.id]=n;});
        mgFilteredEdges.forEach(e=>{
          const a=nodeMap[e.source],b=nodeMap[e.target];
          if(!a||!b)return;
          let dx=b.x-a.x,dy=b.y-a.y;
          let dist=Math.sqrt(dx*dx+dy*dy)||1;
          let f=(dist-springLen)*springK;
          let fx2=f*dx/dist,fy2=f*dy/dist;
          if(!a.pinned){a.vx+=fx2*dt;a.vy+=fy2*dt;}
          if(!b.pinned){b.vx-=fx2*dt;b.vy-=fy2*dt;}
        });
      }
      mgRender();
    }

    function mgRender(){
      const canvas=document.getElementById('mem-graph-canvas');
      if(!canvas)return;
      canvas.width=canvas.clientWidth*(window.devicePixelRatio||1);
      canvas.height=canvas.clientHeight*(window.devicePixelRatio||1);
      const ctx=canvas.getContext('2d');
      const dpr=window.devicePixelRatio||1;
      ctx.scale(dpr,dpr);
      ctx.clearRect(0,0,canvas.clientWidth,canvas.clientHeight);
      ctx.save();ctx.translate(mgPanX,mgPanY);ctx.scale(mgScale,mgScale);
      const nodeMap={};mgFiltered.forEach(n=>{nodeMap[n.id]=n;});
      mgFilteredEdges.forEach(e=>{
        const a=nodeMap[e.source],b=nodeMap[e.target];
        if(!a||!b)return;
        ctx.beginPath();ctx.moveTo(a.x,a.y);ctx.lineTo(b.x,b.y);
        ctx.strokeStyle='#30363d';ctx.lineWidth=1;ctx.stroke();
      });
      const r=8;
      mgFiltered.forEach(n=>{
        ctx.beginPath();ctx.arc(n.x,n.y,n===mgPinned?r+3:r,0,Math.PI*2);
        ctx.fillStyle=mgColors[n.type]||'#8b949e';
        if(n===mgPinned){ctx.lineWidth=2;ctx.strokeStyle='#fff';ctx.stroke();}
        ctx.fill();
        const lbl=n.label||'';
        if(lbl.length>0&&mgScale>0.5){
          ctx.fillStyle='#c9d1d9';ctx.font='10px sans-serif';ctx.textAlign='center';
          ctx.fillText(lbl.substring(0,30),n.x,n.y-r-4);
        }
      });
      ctx.restore();
    }

    (function(){
      const mgCanvas=document.getElementById('mem-graph-canvas');
      if(!mgCanvas)return;
      function mgHitTest(mx,my){
        const x=(mx-mgPanX)/mgScale,y=(my-mgPanY)/mgScale;
        for(const n of mgFiltered){if((n.x-x)**2+(n.y-y)**2<144)return n;}
        return null;
      }
      mgCanvas.addEventListener('mousemove',function(e){
        const rect=mgCanvas.getBoundingClientRect();
        const mx=e.clientX-rect.left,my=e.clientY-rect.top;
        if(mgDrag){mgDrag.x=(mx-mgOffX-mgPanX)/mgScale;mgDrag.y=(my-mgOffY-mgPanY)/mgScale;mgRender();return;}
        const node=mgHitTest(mx,my);
        const tip=document.getElementById('mem-graph-tooltip');
        if(node){
          mgCanvas.style.cursor='pointer';tip.style.display='block';
          tip.style.left=Math.min(mx+12,mgCanvas.clientWidth-330)+'px';tip.style.top=(my+12)+'px';
          tip.innerHTML='<strong style="color:'+(mgColors[node.type]||'#ccc')+'">'+esc(node.type)+'</strong><br>'+esc((node.content||'').substring(0,200));
        }else{mgCanvas.style.cursor='grab';tip.style.display='none';}
      });
      mgCanvas.addEventListener('mousedown',function(e){
        const rect=mgCanvas.getBoundingClientRect();const mx=e.clientX-rect.left,my=e.clientY-rect.top;
        const node=mgHitTest(mx,my);
        if(node){mgDrag=node;mgCanvas.style.cursor='grabbing';mgOffX=mx-node.x*mgScale-mgPanX;mgOffY=my-node.y*mgScale-mgPanY;}
        else{
          const startPX=mgPanX,startPY=mgPanY,sx=e.clientX,sy=e.clientY;
          function onMove(ev){mgPanX=startPX+(ev.clientX-sx);mgPanY=startPY+(ev.clientY-sy);mgRender();}
          function onUp(){window.removeEventListener('mousemove',onMove);window.removeEventListener('mouseup',onUp);}
          window.addEventListener('mousemove',onMove);window.addEventListener('mouseup',onUp);
        }
      });
      mgCanvas.addEventListener('mouseup',function(){mgDrag=null;mgCanvas.style.cursor='grab';});
      mgCanvas.addEventListener('click',function(e){
        const rect=mgCanvas.getBoundingClientRect();const node=mgHitTest(e.clientX-rect.left,e.clientY-rect.top);
        if(node){
          mgPinned=node;node.pinned=true;
          document.getElementById('mem-graph-detail').style.display='block';
          document.getElementById('mg-detail-title').textContent=node.type;
          document.getElementById('mg-detail-body').innerHTML=
            '<div class="stat"><span class="label">ID</span><span class="value" style="font-size:.75rem;word-break:break-all">'+esc(node.id)+'</span></div>'+
            '<div class="stat"><span class="label">Label</span><span class="value">'+esc(node.label)+'</span></div>'+
            '<div style="margin-top:.5rem;font-size:.8rem;color:#c9d1d9;white-space:pre-wrap;max-height:300px;overflow-y:auto">'+esc(node.content||'')+'</div>';
          mgRender();
        }else{
          if(mgPinned){mgPinned.pinned=false;mgPinned=null;}
          document.getElementById('mem-graph-detail').style.display='none';mgRender();
        }
      });
      mgCanvas.addEventListener('wheel',function(e){
        e.preventDefault();const rect=mgCanvas.getBoundingClientRect();
        const mx=e.clientX-rect.left,my=e.clientY-rect.top;
        const factor=e.deltaY<0?1.1:0.9;
        mgPanX=mx-(mx-mgPanX)*factor;mgPanY=my-(my-mgPanY)*factor;
        mgScale*=factor;mgRender();
      },{passive:false});
    })();

    /* --- Costs --- */
    function fmtLabel(k){
      const map={
        'period':'Period','entry_count':'API Calls',
        'total_prompt_tokens':'Prompt Tokens','total_completion_tokens':'Completion Tokens',
        'total_cost_usd':'Estimated Cost'};
      return map[k]||k.replace(/_/g,' ').replace(/\b\w/g,c=>c.toUpperCase());
    }
    async function fetchCosts(){
      try{
        const r=await fetch('/api/costs'); const d=await r.json();
        function renderSummary(s){
          if(!s||s.error) return `<span class="err">${esc(s?.error||'No cost data — is cost tracking configured?')}</span>`;
          return Object.entries(s).map(([k,v])=>{
            if(v==null)return'';
            if(typeof v==='object')return`<div class="stat"><span class="label">${esc(fmtLabel(k))}</span><span class="value" style="font-size:.8rem">${esc(JSON.stringify(v))}</span></div>`;
            const isCost=k.toLowerCase().includes('cost_usd');
            const isTokens=k.toLowerCase().includes('token');
            let fmt;
            if(typeof v==='number'){
              if(isCost) fmt='$'+v.toFixed(4);
              else if(isTokens) fmt=v.toLocaleString()+' tokens';
              else fmt=v.toLocaleString();
            }else{fmt=String(v);}
            return `<div class="stat"><span class="label">${esc(fmtLabel(k))}</span><span class="value">${fmt}</span></div>`;
          }).join('');
        }
        document.getElementById('costs-daily').innerHTML=renderSummary(d.daily);
        document.getElementById('costs-weekly').innerHTML=renderSummary(d.weekly);
      }catch(e){document.getElementById('costs-daily').innerHTML='<span class="err">Failed to load cost data</span>';}
    }
    async function fetchBudget(){
      try{
        const r=await fetch('/api/budget');const d=await r.json();
        document.getElementById('budget-daily').value=d.daily_budget_usd||500;
        document.getElementById('budget-weekly').value=d.weekly_budget_usd||2500;
      }catch(e){}
    }
    async function saveBudget(){
      const daily=parseFloat(document.getElementById('budget-daily').value)||500;
      const weekly=parseFloat(document.getElementById('budget-weekly').value)||2500;
      try{
        const r=await fetch('/api/budget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({daily_budget_usd:daily,weekly_budget_usd:weekly})});
        const d=await r.json();
        const el=document.getElementById('budget-status');
        el.textContent=d.status==='ok'?'✓ Saved':'Error: '+(d.error||'unknown');
        el.style.color=d.status==='ok'?'var(--green)':'var(--red)';
        setTimeout(()=>{el.textContent='';el.style.color='';},3000);
      }catch(e){document.getElementById('budget-status').textContent='Network error';}
    }
    fetchBudget();

    /* --- Chat --- */
    let ws=null,chatInit=false;
    function initChat(){
      if(ws){try{ws.close();}catch(e){}}
      chatInit=true;
      const proto=location.protocol==='https:'?'wss:':'ws:';
      ws=new WebSocket(`${proto}//${location.host}/ws/chat`);
      const st=document.getElementById('ws-status');
      st.innerHTML='<span style="color:var(--yellow)">● Connecting…</span>';
      ws.onopen=()=>{st.innerHTML='<span style="color:var(--green)">● Connected</span>';};
      ws.onclose=()=>{
        st.innerHTML='<span style="color:var(--red)">● Disconnected</span> <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Reconnect</button>';
        chatInit=false;removeTypingIndicator();setChatBusy(false);
      };
      ws.onerror=()=>{
        st.innerHTML='<span style="color:var(--red)">● Error</span> <button class="btn" onclick="initChat()" style="font-size:.75rem;padding:.1rem .4rem;margin-left:.5rem">Retry</button>';
        removeTypingIndicator();setChatBusy(false);
      };
      ws.onmessage=ev=>{removeTypingIndicator();setChatBusy(false);try{const m=JSON.parse(ev.data);appendMsg(m.role||'system',m.content||ev.data);}catch(ex){appendMsg('system',ev.data);}};
    }
    function sendChat(){
      const inp=document.getElementById('chat-input'); const txt=inp.value.trim();
      if(!txt) return;
      if(!ws||ws.readyState!==WebSocket.OPEN){
        appendMsg('system','Not connected. Click Reconnect to establish a session.');
        return;
      }
      appendMsg('user',txt); ws.send(txt); inp.value='';
      showTypingIndicator(); setChatBusy(true);
    }
    function showTypingIndicator(){
      removeTypingIndicator();
      const el=document.getElementById('chat-messages');
      const div=document.createElement('div');
      div.id='typing-indicator';
      div.className='chat-msg';
      div.innerHTML='<span class="role assistant">simard:</span> <span class="typing-dots"><span>.</span><span>.</span><span>.</span></span>';
      el.appendChild(div);
      el.scrollTop=el.scrollHeight;
    }
    function removeTypingIndicator(){
      const ind=document.getElementById('typing-indicator');
      if(ind) ind.remove();
    }
    function setChatBusy(busy){
      document.getElementById('chat-send').disabled=busy;
      document.getElementById('chat-input').disabled=busy;
    }
    function appendMsg(role,content){
      const el=document.getElementById('chat-messages');
      const div=document.createElement('div');
      div.className='chat-msg';
      const roleSpan=document.createElement('span');
      roleSpan.className='role '+role;
      roleSpan.textContent=role+':';
      div.appendChild(roleSpan);
      div.appendChild(document.createTextNode(' '+content));
      el.appendChild(div);
      el.scrollTop=el.scrollHeight;
    }
    document.getElementById('chat-input').addEventListener('keydown',e=>{
      if(e.key==='Enter'&&!e.shiftKey){e.preventDefault();sendChat();}
    });


    /* --- Workboard --- */
    const phaseColors={act:'var(--green)',orient:'var(--yellow)',observe:'var(--accent)',decide:'#a371f7',sleep:'#8b949e',unknown:'#8b949e'};
    function fmtDuration(s){if(s<60)return s+'s';const m=Math.floor(s/60);if(m<60)return m+'m '+s%60+'s';const h=Math.floor(m/60);return h+'h '+m%60+'m';}
    function wbGoalCard(g){
      const pct=g.progress_pct||0;
      const barColor=g.status==='done'?'var(--green)':g.status.startsWith('blocked')?'var(--red)':'var(--accent)';
      return`<div style="background:var(--bg);border:1px solid var(--border);border-radius:6px;padding:.6rem;margin-bottom:.5rem">
        <div style="font-weight:600;font-size:.85rem;margin-bottom:.3rem">${esc(g.name)}</div>
        <div style="font-size:.75rem;color:#8b949e;margin-bottom:.4rem">${esc(g.description||'')}</div>
        <div style="background:#21262d;border-radius:3px;height:6px;margin-bottom:.3rem">
          <div style="background:${barColor};height:100%;border-radius:3px;width:${pct}%;transition:width .3s"></div>
        </div>
        <div style="font-size:.7rem;color:#8b949e">${pct}% complete${g.assigned_to?' · '+esc(g.assigned_to):''}</div>
      </div>`;
    }
    async function fetchWorkboard(){
      try{
        const r=await fetch('/api/workboard'); const d=await r.json();
        // Header
        const phase=d.cycle?.phase||'unknown';
        document.getElementById('wb-phase-dot').style.background=phaseColors[phase]||phaseColors.unknown;
        document.getElementById('wb-cycle-label').textContent='Cycle #'+(d.cycle?.number||'—');
        document.getElementById('wb-phase-label').textContent=phase;
        document.getElementById('wb-uptime').textContent=fmtDuration(d.uptime_seconds||0);
        document.getElementById('wb-eta').textContent=d.next_cycle_eta_seconds>0?fmtDuration(d.next_cycle_eta_seconds):'now';
        // Kanban columns
        const cols={queued:[],in_progress:[],blocked:[],done:[]};
        (d.goals||[]).forEach(g=>{
          if(g.status==='done') cols.done.push(g);
          else if(g.status==='queued') cols.queued.push(g);
          else if(g.status.startsWith('blocked')) cols.blocked.push(g);
          else cols.in_progress.push(g);
        });
        document.getElementById('wb-col-queued').innerHTML=cols.queued.length?cols.queued.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        document.getElementById('wb-col-inprogress').innerHTML=cols.in_progress.length?cols.in_progress.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        document.getElementById('wb-col-blocked').innerHTML=cols.blocked.length?cols.blocked.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        document.getElementById('wb-col-done').innerHTML=cols.done.length?cols.done.map(wbGoalCard).join(''):'<span style="color:#8b949e;font-size:.8rem">—</span>';
        // Engineers
        if(d.spawned_engineers?.length){
          document.getElementById('wb-engineers').innerHTML=d.spawned_engineers.map(e=>{
            const sc=e.alive?'ok':'err';
            return`<div style="display:flex;align-items:center;gap:.75rem;padding:.4rem 0;border-bottom:1px solid var(--border)">
              <span class="${sc}" style="font-weight:600">PID ${e.pid}</span>
              <span style="flex:1">${esc(e.task)}</span>
              <span class="${sc}" style="font-size:.8rem">${e.alive?'alive':'exited'}</span>
              <span style="color:#8b949e;font-size:.75rem">${timeAgo(e.started_at)}</span>
            </div>`;
          }).join('');
        }else{document.getElementById('wb-engineers').innerHTML='<span style="color:#8b949e;font-size:.85rem">No spawned engineers</span>';}
        // Recent actions timeline
        if(d.recent_actions?.length){
          document.getElementById('wb-actions').innerHTML=d.recent_actions.map(a=>{
            const isCurrent=a.action==='current';
            return`<div style="display:flex;gap:.5rem;padding:.35rem 0;border-bottom:1px solid var(--border);font-size:.85rem">
              <span style="color:var(--accent);min-width:2.5rem;font-weight:600">#${a.cycle}</span>
              <span style="min-width:5rem;color:${isCurrent?'var(--green)':'#8b949e'}">${esc(a.action)}</span>
              <span style="flex:1">${esc(a.result)}</span>
              ${a.at?'<span style="color:#8b949e;font-size:.75rem">'+timeAgo(a.at)+'</span>':''}
            </div>`;
          }).join('');
        }else{document.getElementById('wb-actions').innerHTML='<span style="color:#8b949e;font-size:.85rem">No recent actions</span>';}
        // Task memory (rich facts)
        const tm=d.task_memory||{};
        document.getElementById('wb-facts-count').textContent=(tm.facts_count||0)+' facts';
        if(tm.recent_facts?.length){
          document.getElementById('wb-facts-list').innerHTML=tm.recent_facts.map(f=>{
            const conf=typeof f.confidence==='number'?(' <span style="color:#8b949e;font-size:.75rem">('+Math.round(f.confidence*100)+'%)</span>'):'';
            const tags=(f.tags||[]).map(t=>'<span style="background:var(--border);padding:0 .3rem;border-radius:3px;font-size:.7rem;margin-left:.3rem">'+esc(t)+'</span>').join('');
            return'<div style="padding:.25rem 0;border-bottom:1px solid var(--border)"><strong style="color:var(--accent);font-size:.8rem">'+esc(f.concept||'')+'</strong>'+conf+tags+'<div>'+esc(f.content||'')+'</div></div>';
          }).join('');
        }else{document.getElementById('wb-facts-list').innerHTML='<span style="color:#8b949e">No recent facts in memory</span>';}
        // Working memory
        const wm=d.working_memory||[];
        document.getElementById('wb-wm-count').textContent=wm.length+' slots';
        if(wm.length){
          document.getElementById('wb-wm-list').innerHTML=wm.map(s=>{
            return'<div style="padding:.25rem 0;border-bottom:1px solid var(--border)"><span style="color:var(--accent);font-weight:600;font-size:.8rem">'+esc(s.slot_type)+'</span> <span style="color:#8b949e;font-size:.75rem">['+esc(s.task_id)+'] rel='+((s.relevance||0).toFixed(2))+'</span><div>'+esc(s.content)+'</div></div>';
          }).join('');
        }else{document.getElementById('wb-wm-list').innerHTML='<span style="color:#8b949e">No active working memory</span>';}
        // Cognitive statistics
        const cs=d.cognitive_statistics;
        if(cs){
          document.getElementById('wb-cog-stats').innerHTML=[
            ['Sensory',cs.sensory_count],['Working',cs.working_count],['Episodic',cs.episodic_count],
            ['Semantic',cs.semantic_count],['Procedural',cs.procedural_count],['Prospective',cs.prospective_count],['Total',cs.total]
          ].map(([k,v])=>'<span style="margin-right:1rem"><strong>'+k+':</strong> '+(v||0)+'</span>').join('');
        }else{document.getElementById('wb-cog-stats').innerHTML='<span style="color:#8b949e">No cognitive memory available</span>';}
      }catch(e){document.getElementById('wb-engineers').innerHTML='<span class="err">Failed to load workboard data</span>';}
    }

    /* --- Thinking --- */
    async function fetchThinking(){
      try{
        const r=await fetch('/api/ooda-thinking');
        const d=await r.json();
        const el=document.getElementById('thinking-timeline');
        if(!d.reports?.length){el.innerHTML='<span style="color:#8b949e">No cycle reports yet. The OODA daemon generates these during autonomous work.</span>';return;}
        el.innerHTML=d.reports.map(rpt=>{
          if(rpt.legacy){
            return `<div class="thinking-cycle legacy">
              <div class="cycle-header"><span class="cycle-num">Cycle #${rpt.cycle_number}</span><span class="cycle-badge">legacy</span></div>
              <div class="cycle-summary">${esc(rpt.summary)}</div>
            </div>`;
          }
          const phases=[];
          if(rpt.observation){
            const obs=rpt.observation;
            phases.push(`<div class="phase observe">
              <div class="phase-label">👁 Observe</div>
              <div class="phase-content">
                <div>${obs.goal_count} goals tracked</div>
                ${obs.goals?.map(g=>`<div class="goal-line">• ${esc(g.id)}: ${esc(g.progress)}</div>`).join('')||''}
                ${obs.gym_health?`<div>Gym: ${(obs.gym_health.pass_rate*100).toFixed(0)}% pass rate (${obs.gym_health.scenario_count} scenarios)</div>`:''}
                ${obs.environment?`<div>Env: ${obs.environment.open_issues} issues, ${obs.environment.recent_commits} recent commits${obs.environment.git_status?'':' (clean)'}</div>`:''}
              </div>
            </div>`);
          }
          if(rpt.priorities?.length){
            phases.push(`<div class="phase orient">
              <div class="phase-label">🧭 Orient</div>
              <div class="phase-content">
                ${rpt.priorities.map(p=>`<div class="priority-line">
                  <span class="urgency" style="color:${p.urgency>0.7?'var(--red)':p.urgency>0.4?'var(--yellow)':'var(--green)'}">●</span>
                  <strong>${esc(p.goal_id)}</strong> (urgency: ${p.urgency.toFixed(2)}) — ${esc(p.reason)}
                </div>`).join('')}
              </div>
            </div>`);
          }
          if(rpt.planned_actions?.length){
            phases.push(`<div class="phase decide">
              <div class="phase-label">🎯 Decide</div>
              <div class="phase-content">
                ${rpt.planned_actions.map(a=>`<div>→ <code>${esc(a.kind)}</code> ${a.goal_id?'['+esc(a.goal_id)+']':''} ${esc(a.description)}</div>`).join('')}
              </div>
            </div>`);
          }
          if(rpt.outcomes?.length){
            phases.push(`<div class="phase act">
              <div class="phase-label">⚡ Act</div>
              <div class="phase-content">
                ${rpt.outcomes.map(o=>`<div class="outcome ${o.success?'success':'failure'}">
                  ${o.success?'✅':'❌'} <code>${esc(o.action_kind)}</code> — ${esc(o.action_description)}
                  <div class="outcome-detail">${esc((o.detail||'').substring(0,300))}${(o.detail||'').length>300?'…':''}</div>
                </div>`).join('')}
              </div>
            </div>`);
          }
          return `<div class="thinking-cycle">
            <div class="cycle-header">
              <span class="cycle-num">Cycle #${rpt.cycle_number}</span>
              <span class="cycle-summary-inline">${esc(rpt.summary||'')}</span>
            </div>
            <div class="cycle-phases">${phases.join('')}</div>
          </div>`;
        }).join('');
      }catch(e){document.getElementById('thinking-timeline').innerHTML='<span class="err">Failed to load: '+esc(e.toString())+'</span>';}
    }

    /* --- Init --- */
    fetchStatus(); fetchIssues(); fetchDistributed();
    setInterval(fetchStatus,30000);
    setInterval(fetchIssues,120000);
  </script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_router_creates_valid_router() {
        let router = build_router();
        // Verify the router can be constructed without panicking.
        // Axum routers are opaque, but construction succeeding validates
        // that all route paths, handlers, and middleware are well-formed.
        let _ = router;
    }

    #[test]
    fn login_html_contains_form() {
        assert!(LOGIN_HTML.contains("<form"));
        assert!(LOGIN_HTML.contains("login-form"));
        assert!(LOGIN_HTML.contains("/api/login"));
    }

    #[test]
    fn index_html_contains_dashboard_structure() {
        assert!(INDEX_HTML.contains("Simard Dashboard"));
        assert!(INDEX_HTML.contains("/api/status"));
        assert!(INDEX_HTML.contains("/api/workboard"));
        assert!(INDEX_HTML.contains("Whiteboard"));
        assert!(INDEX_HTML.contains("/api/issues"));
        assert!(INDEX_HTML.contains("fetchStatus"));
        assert!(INDEX_HTML.contains("mem-graph-canvas"));
        assert!(INDEX_HTML.contains("fetchMemoryGraph"));
    }

    #[test]
    fn login_html_has_code_input() {
        assert!(LOGIN_HTML.contains(r#"type="text""#));
        assert!(LOGIN_HTML.contains("maxlength"));
    }

    #[test]
    fn read_recent_cycle_reports_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let reports = read_recent_cycle_reports(dir.path(), 5);
        assert!(reports.is_empty());
    }

    #[test]
    fn read_recent_cycle_reports_returns_sorted_and_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        for i in 1..=15 {
            std::fs::write(
                cycle_dir.join(format!("cycle_{i}.json")),
                format!("Cycle {i}: 1 action, 1 succeeded"),
            )
            .unwrap();
        }

        let reports = read_recent_cycle_reports(dir.path(), 5);
        assert_eq!(reports.len(), 5);
        // Should be sorted descending by cycle number
        assert_eq!(reports[0]["cycle_number"], 15);
        assert_eq!(reports[4]["cycle_number"], 11);
    }

    #[test]
    fn read_recent_cycle_reports_parses_json_content() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        std::fs::write(
            cycle_dir.join("cycle_1.json"),
            r#"{"actions": 3, "succeeded": 2}"#,
        )
        .unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 5);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["cycle_number"], 1);
        // JSON content should be nested under "report"
        assert!(reports[0].get("report").is_some());
        assert_eq!(reports[0]["report"]["actions"], 3);
    }

    #[test]
    fn read_recent_cycle_reports_deduplicates_across_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Create both candidate directories with overlapping cycle numbers
        let dir_a = dir.path().join("cycle_reports");
        let dir_b = dir.path().join("state").join("cycle_reports");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        std::fs::write(dir_a.join("cycle_5.json"), "from dir_a").unwrap();
        std::fs::write(dir_b.join("cycle_5.json"), "from dir_b").unwrap();
        std::fs::write(dir_b.join("cycle_6.json"), "unique to dir_b").unwrap();

        let reports = read_recent_cycle_reports(dir.path(), 10);
        // Should have 2 unique cycle numbers (5 and 6), not 3
        assert_eq!(reports.len(), 2);
    }

    #[tokio::test]
    async fn run_gh_json_returns_empty_array_on_failure() {
        // gh is unlikely to succeed without auth in test; verify graceful handling
        let result = run_gh_json(&["pr", "list", "--json", "number"]).await;
        assert!(result.is_array());
    }
}
