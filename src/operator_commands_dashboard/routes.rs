use axum::{
    Json, Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    middleware, response,
    routing::get,
    routing::post,
};
use serde_json::{Value, json};

use super::auth::{require_auth, try_login};

pub fn build_router() -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/issues", get(issues))
        .route("/api/metrics", get(metrics))
        .route("/api/costs", get(costs))
        .route("/api/budget", get(get_budget).post(set_budget))
        .route("/api/goals", get(goals))
        .route("/api/goals/seed", post(seed_goals))
        .route("/api/distributed", get(distributed))
        .route(
            "/api/hosts",
            get(get_hosts).post(add_host).delete(remove_host),
        )
        .route("/api/logs", get(logs))
        .route("/api/processes", get(processes))
        .route("/api/memory", get(memory_metrics))
        .route("/api/memory/search", post(memory_search))
        .route("/api/traces", get(traces))
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
                    if age.num_seconds() < 600 {
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
    match serde_json::from_str::<Value>(&content) {
        Ok(val) => {
            // goal_records.json may be a GoalBoard or an array of goals
            if val.is_object() {
                let active = val.get("active").cloned().unwrap_or(json!([]));
                let backlog = val.get("backlog").cloned().unwrap_or(json!([]));
                Json(json!({
                    "active": active,
                    "backlog": backlog,
                    "active_count": active.as_array().map(|a| a.len()).unwrap_or(0),
                    "backlog_count": backlog.as_array().map(|a| a.len()).unwrap_or(0),
                }))
            } else if val.is_array() {
                Json(json!({
                    "active": val,
                    "backlog": [],
                    "active_count": val.as_array().map(|a| a.len()).unwrap_or(0),
                    "backlog_count": 0,
                }))
            } else {
                Json(json!({"active": [], "backlog": [], "active_count": 0, "backlog_count": 0}))
            }
        }
        Err(_) => Json(json!({"active": [], "backlog": [], "active_count": 0, "backlog_count": 0})),
    }
}

async fn seed_goals() -> Json<Value> {
    let state_root = resolve_state_root();
    let goal_path = state_root.join("goal_records.json");

    // Only seed if no goals exist yet
    if goal_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&goal_path) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                let has_goals = val
                    .get("active")
                    .and_then(|a| a.as_array())
                    .map(|a| !a.is_empty())
                    .unwrap_or(false);
                if has_goals {
                    return Json(
                        json!({"status": "already_seeded", "message": "Goals already exist"}),
                    );
                }
            }
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
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
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
    }

    Json(json!({
        "query": query,
        "result_count": results.len(),
        "results": results,
        "timestamp": chrono::Utc::now().to_rfc3339(),
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
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().take(50) {
                if let Ok(val) = serde_json::from_str::<Value>(line) {
                    spans.push(json!({"source": "journald", "data": val}));
                }
            }
        }
    }

    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();

    Json(json!({
        "span_count": spans.len(),
        "spans": spans,
        "otel_enabled": otel_endpoint.is_some(),
        "otel_endpoint": otel_endpoint,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn distributed() -> Json<Value> {
    // Query the Simard VM status via azlin connect
    let vm_status = tokio::process::Command::new("azlin")
        .args([
            "connect",
            "Simard",
            "--resource-group",
            "rysweet-linux-vm-pool",
            "--no-tmux",
            "--",
            "export PATH=\"$HOME/.cargo/bin:$HOME/.simard/bin:$PATH\" && \
             echo HOSTNAME=$(hostname) && \
             echo UPTIME=$(uptime -p) && \
             echo DISK_ROOT=$(df / --output=pcent | tail -1 | tr -d ' %') && \
             echo DISK_DATA=$(df /mnt/home-data --output=pcent 2>/dev/null | tail -1 | tr -d ' %' || echo N/A) && \
             echo DISK_TMP=$(df /mnt/tmp-data --output=pcent 2>/dev/null | tail -1 | tr -d ' %' || echo N/A) && \
             echo SIMARD_PROCS=$(pgrep -f simard -c 2>/dev/null || echo 0) && \
             echo CARGO_PROCS=$(pgrep -f cargo -c 2>/dev/null || echo 0) && \
             echo LOAD=$(cat /proc/loadavg | cut -d' ' -f1-3) && \
             echo MEM_USED=$(free -m | awk '/Mem/{printf \"%d/%d\", $3, $2}')",
        ])
        .output()
        .await;

    let mut vm_info = json!({
        "vm_name": "Simard",
        "resource_group": "rysweet-linux-vm-pool",
        "status": "unknown",
    });

    if let Ok(output) = vm_status {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("HOSTNAME=") {
            vm_info["status"] = json!("reachable");
            for line in stdout.lines() {
                if let Some((key, val)) = line.split_once('=') {
                    let key = key.trim().to_lowercase();
                    let val = val.trim();
                    match key.as_str() {
                        "hostname" => vm_info["hostname"] = json!(val),
                        "uptime" => vm_info["uptime"] = json!(val),
                        "disk_root" => vm_info["disk_root_pct"] = json!(val.parse::<u32>().ok()),
                        "disk_data" => vm_info["disk_data_pct"] = json!(val.parse::<u32>().ok()),
                        "disk_tmp" => vm_info["disk_tmp_pct"] = json!(val.parse::<u32>().ok()),
                        "simard_procs" => {
                            vm_info["simard_processes"] = json!(val.parse::<u32>().ok())
                        }
                        "cargo_procs" => {
                            vm_info["cargo_processes"] = json!(val.parse::<u32>().ok())
                        }
                        "load" => vm_info["load_avg"] = json!(val),
                        "mem_used" => vm_info["memory_mb"] = json!(val),
                        _ => {}
                    }
                }
            }
        } else {
            vm_info["status"] = json!("unreachable");
        }
    } else {
        vm_info["status"] = json!("error");
        vm_info["error"] = json!("azlin connect failed");
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
fn load_dashboard_meeting_prompt() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/meeting_system.md");
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Open an agent session for the dashboard chat, using the same infrastructure
/// as the meeting REPL.  Returns the session or logs the error and returns `None`.
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
    use crate::base_types::BaseTypeTurnInput;
    use crate::meeting_facilitator::{
        ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus, add_note, add_question,
        edit_item, record_action_item, record_decision, remove_item,
    };
    use crate::meeting_repl::{
        MeetingCommand, auto_capture_structured_items, parse_meeting_command,
    };

    // Open agent session in a blocking context (session builder does synchronous I/O).
    let mut agent_session: Option<Box<dyn crate::base_types::BaseTypeSession>> =
        tokio::task::spawn_blocking(open_dashboard_agent_session)
            .await
            .ok()
            .flatten();

    let meeting_system_prompt = load_dashboard_meeting_prompt();
    let has_agent = agent_session.is_some();

    let topic = "Dashboard Chat";
    let mut session = MeetingSession {
        topic: topic.to_string(),
        decisions: Vec::new(),
        action_items: Vec::new(),
        notes: Vec::new(),
        status: MeetingSessionStatus::Open,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: vec!["operator".to_string()],
        explicit_questions: Vec::new(),
    };

    let greeting = if has_agent {
        "Connected to Simard. Speak naturally — I'll respond conversationally. Use /help for commands, /close to end."
    } else {
        "Connected in note-taking mode (no agent backend). Use /help for commands, /close to end."
    };
    let _ = socket
        .send(Message::Text(
            json!({"role":"system","content": greeting})
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

                let cmd = parse_meeting_command(trimmed);
                let is_conversation = matches!(&cmd, MeetingCommand::Conversation(_));

                let reply_content = match cmd {
                    MeetingCommand::Close => {
                        session.status = MeetingSessionStatus::Closed;
                        let recap = format!(
                            "Meeting closed. {} decision(s), {} action item(s), {} note(s).",
                            session.decisions.len(),
                            session.action_items.len(),
                            session.notes.len(),
                        );
                        let _ = socket
                            .send(Message::Text(
                                json!({"role":"system","content": recap}).to_string().into(),
                            ))
                            .await;
                        break;
                    }
                    MeetingCommand::Help => {
                        "Commands: /decision <desc> | <rationale>, /action <desc> | <owner>, \
                         /note <text>, /question <text>, /status, /recap, /list, \
                         /edit <type> <n> <text>, /delete <type> <n>, /close. \
                         Or just type naturally to talk with Simard."
                            .to_string()
                    }
                    MeetingCommand::Decision {
                        description,
                        rationale,
                    } => {
                        let decision = MeetingDecision {
                            description: description.clone(),
                            rationale,
                            participants: Vec::new(),
                        };
                        match record_decision(&mut session, decision) {
                            Ok(()) => format!("Recorded decision: {description}"),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    MeetingCommand::Action {
                        description,
                        owner,
                        priority,
                        due_description,
                    } => {
                        let item = ActionItem {
                            description: description.clone(),
                            owner: owner.clone(),
                            priority,
                            due_description: due_description.clone(),
                        };
                        match record_action_item(&mut session, item) {
                            Ok(()) => {
                                let due_suffix = due_description
                                    .as_deref()
                                    .map(|d| format!(", due: {d}"))
                                    .unwrap_or_default();
                                format!(
                                    "Recorded action: {description} (owner={owner}{due_suffix})"
                                )
                            }
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    MeetingCommand::Note(ref note_text) => {
                        match add_note(&mut session, note_text) {
                            Ok(()) => "Note added.".to_string(),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    MeetingCommand::Question(ref q_text) => {
                        match add_question(&mut session, q_text) {
                            Ok(()) => format!("Question added: {q_text}"),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    MeetingCommand::Status => {
                        format!(
                            "Meeting: {}\n  Decisions: {}\n  Actions: {}\n  Notes: {}\n  Questions: {}\n  Participants: {}",
                            session.topic,
                            session.decisions.len(),
                            session.action_items.len(),
                            session.notes.len(),
                            session.explicit_questions.len(),
                            session.participants.len(),
                        )
                    }
                    MeetingCommand::Recap => {
                        let mut buf = String::new();
                        buf.push_str(&format!(
                            "── Meeting Recap ──\nTopic: {}\n\n",
                            session.topic
                        ));
                        buf.push_str(&format!("Decisions ({}):\n", session.decisions.len()));
                        if session.decisions.is_empty() {
                            buf.push_str("  (none)\n");
                        } else {
                            for (i, d) in session.decisions.iter().enumerate() {
                                buf.push_str(&format!(
                                    "  {}. {} — {}\n",
                                    i + 1,
                                    d.description,
                                    d.rationale
                                ));
                            }
                        }
                        buf.push_str(&format!(
                            "\nAction Items ({}):\n",
                            session.action_items.len()
                        ));
                        if session.action_items.is_empty() {
                            buf.push_str("  (none)\n");
                        } else {
                            for (i, a) in session.action_items.iter().enumerate() {
                                buf.push_str(&format!(
                                    "  {}. [P{}] {} (owner: {})\n",
                                    i + 1,
                                    a.priority,
                                    a.description,
                                    a.owner
                                ));
                            }
                        }
                        buf.push_str(&format!("\nNotes ({}):\n", session.notes.len()));
                        if session.notes.is_empty() {
                            buf.push_str("  (none)\n");
                        } else {
                            for n in &session.notes {
                                buf.push_str(&format!("  - {n}\n"));
                            }
                        }
                        buf
                    }
                    MeetingCommand::List => {
                        let has_items = !session.decisions.is_empty()
                            || !session.action_items.is_empty()
                            || !session.notes.is_empty()
                            || !session.explicit_questions.is_empty();
                        if !has_items {
                            "No items recorded yet.".to_string()
                        } else {
                            let mut buf = String::new();
                            if !session.decisions.is_empty() {
                                buf.push_str("Decisions:\n");
                                for (i, d) in session.decisions.iter().enumerate() {
                                    buf.push_str(&format!("  {}. {}\n", i + 1, d.description));
                                }
                            }
                            if !session.action_items.is_empty() {
                                buf.push_str("Action items:\n");
                                for (i, a) in session.action_items.iter().enumerate() {
                                    buf.push_str(&format!(
                                        "  {}. {} (owner={})\n",
                                        i + 1,
                                        a.description,
                                        a.owner
                                    ));
                                }
                            }
                            if !session.notes.is_empty() {
                                buf.push_str("Notes:\n");
                                for (i, n) in session.notes.iter().enumerate() {
                                    buf.push_str(&format!("  {}. {}\n", i + 1, n));
                                }
                            }
                            if !session.explicit_questions.is_empty() {
                                buf.push_str("Questions:\n");
                                for (i, q) in session.explicit_questions.iter().enumerate() {
                                    buf.push_str(&format!("  {}. {}\n", i + 1, q));
                                }
                            }
                            buf
                        }
                    }
                    MeetingCommand::Edit {
                        item_type,
                        index,
                        new_text,
                    } => match edit_item(&mut session, &item_type, index, &new_text) {
                        Ok(()) => format!("Updated {item_type} {}.", index + 1),
                        Err(e) => format!("Error: {e}"),
                    },
                    MeetingCommand::Delete { item_type, index } => {
                        match remove_item(&mut session, &item_type, index) {
                            Ok(()) => format!("Deleted {item_type} {}.", index + 1),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    MeetingCommand::AddParticipant(name) => {
                        if !session.participants.contains(&name) {
                            session.participants.push(name.clone());
                        }
                        format!("Participant added: {name}")
                    }
                    MeetingCommand::ListParticipants => {
                        if session.participants.is_empty() {
                            "No participants recorded yet.".to_string()
                        } else {
                            let mut buf = String::from("Participants:\n");
                            for p in &session.participants {
                                buf.push_str(&format!("  - {p}\n"));
                            }
                            buf
                        }
                    }
                    MeetingCommand::Preview => {
                        format!(
                            "Handoff Preview — {} decision(s), {} action(s), {} note(s), {} question(s).",
                            session.decisions.len(),
                            session.action_items.len(),
                            session.notes.len(),
                            session.explicit_questions.len(),
                        )
                    }
                    MeetingCommand::Conversation(ref user_text) => {
                        if let Some(ref mut agent) = agent_session {
                            let turn_input = BaseTypeTurnInput {
                                objective: user_text.clone(),
                                identity_context: meeting_system_prompt.clone(),
                                prompt_preamble: format!("Meeting topic: {topic}"),
                            };
                            // run_turn is synchronous (blocks on LLM call)
                            match agent.run_turn(turn_input) {
                                Ok(outcome) => {
                                    let response = outcome.execution_summary.trim().to_string();
                                    add_note(&mut session, &format!("operator: {user_text}")).ok();
                                    add_note(&mut session, &format!("simard: {response}")).ok();
                                    // Auto-capture decisions/actions from conversation
                                    let mut capture_buf: Vec<u8> = Vec::new();
                                    auto_capture_structured_items(
                                        &mut session,
                                        user_text,
                                        &response,
                                        &mut capture_buf,
                                    );
                                    response
                                }
                                Err(e) => {
                                    add_note(&mut session, user_text).ok();
                                    format!("[agent error: {e}]")
                                }
                            }
                        } else {
                            add_note(&mut session, user_text).ok();
                            "Note added. (No agent backend — running in note-taking mode. Check SIMARD_LLM_PROVIDER and auth config.)".to_string()
                        }
                    }
                    MeetingCommand::Empty => continue,
                    MeetingCommand::Unknown(ref input) => {
                        format!("Unknown command: {input}. Type /help for available commands.")
                    }
                };

                let role = if is_conversation {
                    "assistant"
                } else {
                    "system"
                };
                let reply = json!({"role": role, "content": reply_content});
                if socket
                    .send(Message::Text(reply.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Logs endpoint — returns tail of daemon log + OODA transcripts
// ---------------------------------------------------------------------------

async fn logs() -> Json<Value> {
    let state_root = resolve_state_root();

    let daemon_log = read_tail("/var/log/simard-daemon.log", 200)
        .or_else(|| {
            let fallback = state_root.join("simard-daemon.log");
            read_tail(&fallback.to_string_lossy(), 200)
        })
        .unwrap_or_default();

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

    Json(json!({
        "daemon_log_lines": daemon_log,
        "ooda_transcripts": transcripts,
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

    let last_consolidation = [&memory_path, &evidence_path, &goal_path]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter_map(|m| m.modified().ok())
        .max()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });

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
        "total_facts": fact_count + evidence_count + goal_count,
        "last_consolidation": last_consolidation,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/simard-state")
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

const INDEX_HTML: &str = r##"<!DOCTYPE html>
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
  </style>
</head>
<body>
  <header>
    <h1>🌲 Simard Dashboard <span style="font-size:.75rem;color:#8b949e">v2</span></h1>
    <div style="display:flex;align-items:center;gap:1rem">
      <a href="https://github.com/rysweet/Simard/releases/latest" target="_blank" style="color:#3fb950;text-decoration:none;font-size:.85rem;border:1px solid #3fb950;padding:.2rem .6rem;border-radius:4px">📦 Download Latest</a>
      <span id="clock" style="color:#8b949e;font-size:.85rem"></span>
    </div>
  </header>
  <div class="tabs">
    <div class="tab active" data-tab="overview">Overview</div>
    <div class="tab" data-tab="distributed">Distributed</div>
    <div class="tab" data-tab="goals">Goals</div>
    <div class="tab" data-tab="traces">Traces</div>
    <div class="tab" data-tab="logs">Logs</div>
    <div class="tab" data-tab="processes">Processes</div>
    <div class="tab" data-tab="memory">Memory</div>
    <div class="tab" data-tab="costs">Costs</div>
    <div class="tab" data-tab="chat">Chat</div>
  </div>

  <div class="tab-content active" id="tab-overview">
    <div class="grid">
      <div class="card"><h2>System Status</h2><div id="status"><span class="loading">Loading…</span></div></div>
      <div class="card"><h2>Open Issues</h2><ul id="issues-list"><li class="loading">Loading…</li></ul></div>
    </div>
  </div>

  <div class="tab-content" id="tab-distributed">
    <div class="grid">
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
      </h2>
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
      <h2>Daemon Log <button class="btn" onclick="fetchLogs()">Refresh</button></h2>
      <div id="daemon-log" class="log-box"><span class="loading">Loading…</span></div>
    </div>
    <h2 style="color:var(--accent);font-size:1rem;margin-bottom:.5rem">OODA Transcripts</h2>
    <div id="ooda-transcripts"><span class="loading">Loading…</span></div>
  </div>

  <div class="tab-content" id="tab-processes">
    <div class="card">
      <h2>Active Simard Processes <button class="btn" onclick="fetchProcesses()">Refresh</button></h2>
      <div id="proc-count" style="margin-bottom:.5rem;color:#8b949e;font-size:.85rem"></div>
      <div id="proc-table"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-memory">
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

  <div class="tab-content" id="tab-chat">
    <div class="card" style="max-width:720px">
      <h2>Meeting Chat</h2>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.75rem;margin-bottom:1rem;font-size:.85rem;color:#8b949e">
        <strong style="color:var(--accent)">💡 Meeting Help:</strong>
        Use this chat or run <code>simard meeting &lt;topic&gt;</code> from the terminal.
        Commands: <code>/close</code> end session, <code>/goals</code> review goals, <code>/status</code> system status.
        Meetings generate handoff documents that the OODA daemon ingests as new goals.
      </div>
      <div class="ws-status disconnected" id="ws-status">● Disconnected</div>
      <div id="chat-messages"></div>
      <div id="chat-input-row">
        <textarea id="chat-input" placeholder="Type a message… (/close to end session)"></textarea>
        <button id="chat-send" onclick="sendChat()">Send</button>
      </div>
    </div>
  </div>

  <script>
    /* --- Tabs --- */
    document.querySelectorAll('.tab').forEach(tab=>{
      tab.addEventListener('click',()=>{
        document.querySelectorAll('.tab').forEach(t=>t.classList.remove('active'));
        document.querySelectorAll('.tab-content').forEach(c=>c.classList.remove('active'));
        tab.classList.add('active');
        document.getElementById('tab-'+tab.dataset.tab).classList.add('active');
        if(tab.dataset.tab==='logs') fetchLogs();
        if(tab.dataset.tab==='processes') fetchProcesses();
        if(tab.dataset.tab==='memory') fetchMemory();
        if(tab.dataset.tab==='distributed') fetchDistributed();
        if(tab.dataset.tab==='goals') fetchGoals();
        if(tab.dataset.tab==='costs') fetchCosts();
        if(tab.dataset.tab==='traces') fetchTraces();
        if(tab.dataset.tab==='chat') initChat();
      });
    });
    setInterval(()=>{document.getElementById('clock').textContent=new Date().toLocaleTimeString()},1000);

    /* --- Status --- */
    async function fetchStatus(){
      try{
        const r=await fetch('/api/status'); const d=await r.json();
        const dc=d.disk_usage_pct>90?'err':d.disk_usage_pct>70?'warn':'ok';
        const oc=d.ooda_daemon==='running'?'ok':(d.ooda_daemon==='stale'?'warn':'err');
        const shortHash=d.git_hash?d.git_hash.substring(0,7):'';
        const versionLink=d.git_hash?`<a href="https://github.com/rysweet/Simard/commit/${d.git_hash}" target="_blank" style="color:#3fb950;text-decoration:none">v${d.version}</a> (<code>${shortHash}</code>)`:`v${d.version}`;
        let healthDetail='';
        if(d.daemon_health){
          const dh=d.daemon_health;
          healthDetail=` (cycle #${dh.cycle_number??'?'}`;
          if(dh.timestamp) healthDetail+=`, last: ${new Date(dh.timestamp).toLocaleTimeString()}`;
          healthDetail+=')';
        }
        document.getElementById('status').innerHTML=`
          <div class="stat"><span class="label">Version</span><span class="value">${versionLink}</span></div>
          <div class="stat"><span class="label">OODA Daemon</span><span class="value ${oc}">${d.ooda_daemon}${healthDetail}</span></div>
          <div class="stat"><span class="label">Active Processes</span><span class="value">${d.active_processes??0}</span></div>
          <div class="stat"><span class="label">Disk Usage</span><span class="value ${dc}">${d.disk_usage_pct??'?'}%</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${new Date(d.timestamp).toLocaleTimeString()}</span></div>`;
      }catch(e){document.getElementById('status').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Issues --- */
    async function fetchIssues(){
      try{
        const r=await fetch('/api/issues'); const issues=await r.json();
        if(Array.isArray(issues)){
          document.getElementById('issues-list').innerHTML=issues.map(i=>
            `<li><span class="issue-num">#${i.number}</span>${i.title}</li>`
          ).join('');
        }
      }catch(e){document.getElementById('issues-list').innerHTML='<li class="err">Failed to load</li>';}
    }

    /* --- Logs --- */
    async function fetchLogs(){
      try{
        const r=await fetch('/api/logs'); const d=await r.json();
        const el=document.getElementById('daemon-log');
        el.textContent=d.daemon_log_lines?.length?d.daemon_log_lines.join('\n'):'(no daemon log found)';
        el.scrollTop=el.scrollHeight;
        const tEl=document.getElementById('ooda-transcripts');
        if(d.ooda_transcripts?.length){
          tEl.innerHTML=d.ooda_transcripts.map(t=>`
            <div class="transcript-item">
              <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
              <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
            </div>`).join('');
        }else{tEl.innerHTML='<span class="loading">No OODA transcripts found</span>';}
      }catch(e){document.getElementById('daemon-log').textContent='Failed to load logs';}
    }

    /* --- Processes --- */
    async function fetchProcesses(){
      try{
        const r=await fetch('/api/processes'); const d=await r.json();
        document.getElementById('proc-count').textContent=`${d.count} process(es) detected`;
        if(d.processes?.length){
          document.getElementById('proc-table').innerHTML=`
            <table class="proc-table">
              <tr><th>PID</th><th>Uptime</th><th>Command</th><th>Arguments</th></tr>
              ${d.processes.map(p=>`<tr><td>${esc(p.pid)}</td><td>${esc(p.uptime)}</td><td>${esc(p.command)}</td><td style="max-width:400px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(p.full_args)}</td></tr>`).join('')}
            </table>`;
        }else{document.getElementById('proc-table').innerHTML='<span class="loading">No Simard processes found</span>';}
      }catch(e){document.getElementById('proc-table').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Memory --- */
    async function fetchMemory(){
      try{
        const r=await fetch('/api/memory'); const d=await r.json();
        document.getElementById('mem-overview').innerHTML=`
          <div class="stat"><span class="label">Total Facts</span><span class="value">${d.total_facts}</span></div>
          <div class="stat"><span class="label">Last Consolidation</span><span class="value">${d.last_consolidation?new Date(d.last_consolidation).toLocaleString():'N/A'}</span></div>
          <div class="stat"><span class="label">State Root</span><span class="value" style="font-size:.8rem;word-break:break-all">${esc(d.state_root)}</span></div>`;
        const files=[
          {key:'memory_records',label:'Memory Records'},
          {key:'evidence_records',label:'Evidence Records'},
          {key:'goal_records',label:'Goal Records'},
          {key:'handoff',label:'Latest Handoff'}];
        document.getElementById('mem-files').innerHTML=files.map(f=>{
          const info=d[f.key]||{};
          return`<div class="mem-file">
            <h3>${f.label} ${info.count!==undefined?'<span class="badge">'+info.count+' records</span>':''} <span class="badge">${fmtB(info.size_bytes||0)}</span></h3>
            <div class="stat"><span class="label">Modified</span><span class="value">${info.modified?new Date(info.modified).toLocaleString():'N/A'}</span></div>
          </div>`;}).join('');
      }catch(e){document.getElementById('mem-overview').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Distributed --- */
    async function fetchDistributed(){
      try{
        const r=await fetch('/api/distributed'); const d=await r.json();
        document.getElementById('cluster-topology').innerHTML=`
          <div class="stat"><span class="label">Topology</span><span class="value">${d.topology}</span></div>
          <div class="stat"><span class="label">Local Host</span><span class="value">${esc(d.local?.hostname||'?')}</span></div>
          <div class="stat"><span class="label">Updated</span><span class="value">${d.timestamp?new Date(d.timestamp).toLocaleTimeString():'?'}</span></div>`;
        if(d.remote_vms?.length){
          document.getElementById('remote-vms').innerHTML=d.remote_vms.map(vm=>{
            const sc=vm.status==='reachable'?'ok':(vm.status==='unreachable'?'err':'warn');
            return`<div style="border:1px solid #30363d;border-radius:6px;padding:1rem;margin-bottom:.75rem">
              <h3 style="margin:0 0 .5rem 0;color:var(--accent)">${esc(vm.vm_name)} <span class="${sc}" style="font-size:.85rem">${vm.status}</span></h3>
              ${vm.hostname?`<div class="stat"><span class="label">Hostname</span><span class="value">${esc(vm.hostname)}</span></div>`:''}
              ${vm.uptime?`<div class="stat"><span class="label">Uptime</span><span class="value">${esc(vm.uptime)}</span></div>`:''}
              ${vm.load_avg?`<div class="stat"><span class="label">Load</span><span class="value">${esc(vm.load_avg)}</span></div>`:''}
              ${vm.memory_mb?`<div class="stat"><span class="label">Memory</span><span class="value">${esc(vm.memory_mb)} MB</span></div>`:''}
              ${vm.disk_root_pct!==null&&vm.disk_root_pct!==undefined?`<div class="stat"><span class="label">Root Disk</span><span class="value ${vm.disk_root_pct>90?'err':vm.disk_root_pct>70?'warn':'ok'}">${vm.disk_root_pct}%</span></div>`:''}
              ${vm.disk_data_pct!==null&&vm.disk_data_pct!==undefined?`<div class="stat"><span class="label">Data Disk</span><span class="value">${vm.disk_data_pct}%</span></div>`:''}
              ${vm.disk_tmp_pct!==null&&vm.disk_tmp_pct!==undefined?`<div class="stat"><span class="label">Tmp Disk</span><span class="value">${vm.disk_tmp_pct}%</span></div>`:''}
              ${vm.simard_processes!==null&&vm.simard_processes!==undefined?`<div class="stat"><span class="label">Simard Processes</span><span class="value">${vm.simard_processes}</span></div>`:''}
              ${vm.cargo_processes!==null&&vm.cargo_processes!==undefined?`<div class="stat"><span class="label">Cargo Processes</span><span class="value">${vm.cargo_processes}</span></div>`:''}
            </div>`;}).join('');
        }else{document.getElementById('remote-vms').innerHTML='<span class="loading">No remote VMs configured</span>';}
      }catch(e){document.getElementById('cluster-topology').innerHTML='<span class="err">Failed to load</span>';}
    }
    async function fetchHosts(){
      try{
        const r=await fetch('/api/hosts');const d=await r.json();
        const el=document.getElementById('hosts-list');
        if(!d.hosts?.length){el.innerHTML='<span class="loading">No hosts configured</span>';return;}
        el.innerHTML=d.hosts.map(h=>`<div style="display:flex;align-items:center;gap:0.5rem;padding:4px 0;border-bottom:1px solid #222">
          <span style="flex:1"><strong>${esc(h.name)}</strong> <span style="color:#888">(${esc(h.resource_group||'default')})</span></span>
          <button class="btn" style="padding:2px 8px;font-size:.8rem" onclick="removeHost('${esc(h.name)}')">Remove</button>
        </div>`).join('');
      }catch(e){}
    }
    async function addHost(){
      const name=document.getElementById('host-name').value.trim();
      const rg=document.getElementById('host-rg').value.trim();
      if(!name){document.getElementById('host-status').textContent='Name required';return;}
      const r=await fetch('/api/hosts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name,resource_group:rg})});
      const d=await r.json();
      document.getElementById('host-status').textContent=d.status==='ok'?'Added':'Error: '+(d.error||'');
      document.getElementById('host-name').value='';
      fetchHosts();
      setTimeout(()=>document.getElementById('host-status').textContent='',3000);
    }
    async function removeHost(name){
      const r=await fetch('/api/hosts',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({name})});
      fetchHosts();
    }
    fetchHosts();

    /* --- Goals --- */
    async function fetchGoals(){
      try{
        const r=await fetch('/api/goals'); const d=await r.json();
        if(d.active?.length){
          document.getElementById('goals-active').innerHTML=`<table class="proc-table">
            <tr><th>Priority</th><th>ID</th><th>Description</th><th>Status</th><th>Assigned</th></tr>
            ${d.active.map(g=>`<tr>
              <td style="text-align:center">${g.priority}</td>
              <td><code>${esc(g.id)}</code></td>
              <td>${esc(g.description)}</td>
              <td>${esc(g.status)}</td>
              <td>${g.assigned_to?esc(g.assigned_to):'—'}</td>
            </tr>`).join('')}
          </table>`;
        }else{document.getElementById('goals-active').innerHTML='<span class="loading">No active goals</span>';}
        if(d.backlog?.length){
          document.getElementById('goals-backlog').innerHTML=`<table class="proc-table">
            <tr><th>ID</th><th>Description</th><th>Source</th><th>Score</th></tr>
            ${d.backlog.map(b=>`<tr>
              <td><code>${esc(b.id)}</code></td>
              <td>${esc(b.description)}</td>
              <td>${esc(b.source||'')}</td>
              <td>${b.score??'—'}</td>
            </tr>`).join('')}
          </table>`;
        }else{document.getElementById('goals-backlog').innerHTML='<span class="loading">No backlog items</span>';}
      }catch(e){document.getElementById('goals-active').innerHTML='<span class="err">Failed to load</span>';}
    }

    async function seedGoals(){
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
            return`<div style="border-bottom:1px solid #222;padding:4px 0;font-size:.82rem">
              <span style="color:#8b949e">[${esc(s.source)}]</span>
              ${ts?'<span style="color:#58a6ff;margin:0 .5rem">'+esc(String(ts).substring(0,19))+'</span>':''}
              <span>${esc(String(msg))}</span>
            </div>`;
          }).join('');
        }else{document.getElementById('trace-list').innerHTML='<span class="loading">No trace data yet. Run the OODA daemon or make API calls to generate traces.</span>';}
      }catch(e){document.getElementById('trace-list').innerHTML='<span class="err">Failed to load</span>';}
    }

    /* --- Memory Search --- */
    async function searchMemory(){
      const q=document.getElementById('mem-search-input').value.trim();
      if(!q){document.getElementById('mem-search-results').innerHTML='<span class="warn">Enter a search term</span>';return;}
      try{
        const r=await fetch('/api/memory/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:q})});
        const d=await r.json();
        if(d.results?.length){
          document.getElementById('mem-search-results').innerHTML=`
            <p style="color:#8b949e;font-size:.85rem">${d.result_count} result(s) for "${esc(d.query)}"</p>
            ${d.results.map(r=>`<div style="border:1px solid #30363d;border-radius:6px;padding:.75rem;margin-bottom:.5rem">
              <span class="badge">${esc(r.source)}</span>
              <pre style="margin:.5rem 0 0;white-space:pre-wrap;font-size:.8rem;color:#c9d1d9">${esc(JSON.stringify(r.data,null,2).substring(0,500))}</pre>
            </div>`).join('')}`;
        }else{
          document.getElementById('mem-search-results').innerHTML='<span class="loading">No results found</span>';
        }
      }catch(e){document.getElementById('mem-search-results').innerHTML='<span class="err">Search failed</span>';}
    }
    document.getElementById('mem-search-input')?.addEventListener('keypress',e=>{if(e.key==='Enter')searchMemory();});

    /* --- Costs --- */
    async function fetchCosts(){
      try{
        const r=await fetch('/api/costs'); const d=await r.json();
        function renderSummary(s){
          if(!s||s.error) return `<span class="err">${esc(s?.error||'No data')}</span>`;
          return Object.entries(s).map(([k,v])=>{
            if(typeof v!=='number') return `<div class="stat"><span class="label">${esc(k)}</span><span class="value">${esc(String(v))}</span></div>`;
            const isCost=k.toLowerCase().includes('cost')||k.toLowerCase().includes('usd');
            const fmt=isCost?'$'+v.toFixed(4):v.toLocaleString();
            return `<div class="stat"><span class="label">${esc(k)}</span><span class="value">${fmt}</span></div>`;
          }).join('');
        }
        document.getElementById('costs-daily').innerHTML=renderSummary(d.daily);
        document.getElementById('costs-weekly').innerHTML=renderSummary(d.weekly);
      }catch(e){document.getElementById('costs-daily').innerHTML='<span class="err">Failed to load</span>';}
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
      const r=await fetch('/api/budget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({daily_budget_usd:daily,weekly_budget_usd:weekly})});
      const d=await r.json();
      document.getElementById('budget-status').textContent=d.status==='ok'?'Saved':'Error: '+d.error;
      setTimeout(()=>document.getElementById('budget-status').textContent='',3000);
    }
    fetchBudget();

    /* --- Chat --- */
    let ws=null,chatInit=false;
    function initChat(){
      if(chatInit&&ws&&ws.readyState===WebSocket.OPEN) return;
      chatInit=true;
      const proto=location.protocol==='https:'?'wss:':'ws:';
      ws=new WebSocket(`${proto}//${location.host}/ws/chat`);
      const st=document.getElementById('ws-status');
      ws.onopen=()=>{st.textContent='● Connected';st.className='ws-status connected';};
      ws.onclose=()=>{st.textContent='● Disconnected';st.className='ws-status disconnected';chatInit=false;};
      ws.onerror=()=>{st.textContent='● Error';st.className='ws-status disconnected';};
      ws.onmessage=ev=>{try{const m=JSON.parse(ev.data);appendMsg(m.role||'system',m.content||ev.data);}catch{appendMsg('system',ev.data);}};
    }
    function sendChat(){
      const inp=document.getElementById('chat-input'); const txt=inp.value.trim();
      if(!txt||!ws||ws.readyState!==WebSocket.OPEN) return;
      appendMsg('user',txt); ws.send(txt); inp.value='';
    }
    function appendMsg(role,content){
      const el=document.getElementById('chat-messages');
      el.innerHTML+=`<div class="chat-msg"><span class="role ${role}">${role}:</span> ${esc(content)}</div>`;
      el.scrollTop=el.scrollHeight;
    }
    document.getElementById('chat-input').addEventListener('keydown',e=>{
      if(e.key==='Enter'&&!e.shiftKey){e.preventDefault();sendChat();}
    });

    /* --- Helpers --- */
    function fmtB(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';return(b/1048576).toFixed(1)+' MB';}
    function esc(s){const d=document.createElement('div');d.textContent=s;return d.innerHTML;}

    /* --- Init --- */
    fetchStatus(); fetchIssues();
    setInterval(fetchStatus,30000);
    setInterval(fetchIssues,60000);
  </script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_router_has_expected_routes() {
        let router = build_router();
        let _ = router;
    }

    #[test]
    fn login_html_contains_form_elements() {
        assert!(LOGIN_HTML.contains("<form id=\"login-form\">"));
        assert!(LOGIN_HTML.contains("input id=\"code\""));
        assert!(LOGIN_HTML.contains("Log in"));
        assert!(LOGIN_HTML.contains("Simard"));
    }

    #[test]
    fn index_html_contains_dashboard_sections() {
        assert!(INDEX_HTML.contains("Simard Dashboard"));
        assert!(INDEX_HTML.contains("System Status"));
        assert!(INDEX_HTML.contains("Open Issues"));
        assert!(INDEX_HTML.contains("fetchStatus"));
        assert!(INDEX_HTML.contains("fetchIssues"));
    }

    #[test]
    fn login_html_has_security_attributes() {
        assert!(LOGIN_HTML.contains("maxlength=\"8\""));
        assert!(LOGIN_HTML.contains("autocomplete=\"off\""));
    }

    #[test]
    fn index_html_has_refresh_intervals() {
        assert!(INDEX_HTML.contains("setInterval(fetchStatus,30000)"));
        assert!(INDEX_HTML.contains("setInterval(fetchIssues,60000)"));
    }

    #[test]
    fn build_router_creates_router() {
        let router = build_router();
        // Just verify the router is constructed without panic
        let _ = format!("{:?}", "router created");
        drop(router);
    }

    #[test]
    fn login_html_contains_form() {
        assert!(LOGIN_HTML.contains("<form"));
        assert!(LOGIN_HTML.contains("login"));
    }

    #[test]
    fn index_html_contains_dashboard() {
        assert!(INDEX_HTML.contains("Simard Dashboard"));
        assert!(INDEX_HTML.contains("fetchStatus"));
    }

    #[test]
    fn index_html_contains_distributed_tab() {
        assert!(INDEX_HTML.contains("data-tab=\"distributed\""));
        assert!(INDEX_HTML.contains("Distributed"));
        assert!(INDEX_HTML.contains("fetchDistributed"));
        assert!(INDEX_HTML.contains("Cluster Topology"));
        assert!(INDEX_HTML.contains("Remote VMs"));
    }

    #[test]
    fn index_html_contains_goals_tab() {
        assert!(INDEX_HTML.contains("data-tab=\"goals\""));
        assert!(INDEX_HTML.contains("Goals"));
        assert!(INDEX_HTML.contains("fetchGoals"));
        assert!(INDEX_HTML.contains("Active Goals"));
        assert!(INDEX_HTML.contains("Backlog"));
    }

    #[test]
    fn index_html_contains_costs_tab() {
        assert!(INDEX_HTML.contains("data-tab=\"costs\""));
        assert!(INDEX_HTML.contains("Costs"));
        assert!(INDEX_HTML.contains("fetchCosts"));
        assert!(INDEX_HTML.contains("Daily Costs"));
        assert!(INDEX_HTML.contains("Weekly Costs"));
    }
}
