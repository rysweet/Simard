use axum::{
    Json, Router,
    extract::Path,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    middleware, response,
    routing::{delete, get, post, put},
};
use serde_json::{Value, json};

use super::auth::{login, login_page, require_auth};
use super::monitoring::{costs, get_budget, metrics, set_budget};
use super::activity::{activity, traces};
use super::workboard::workboard;
use super::current_work::current_work;
use super::distributed::{distributed, strip_ansi_codes, vacate_vm};
use super::tmux::{azlin_tmux_sessions, ws_tmux_attach_handler};
use super::chat::ws_chat_handler;
use super::hosts::{add_host, get_hosts, host_entry_name, is_local_host, load_hosts, remove_host, save_hosts, tag_local_membership};
use super::memory::{build_agent_graph, classify_agent_layer, memory_graph, memory_search};
use super::goals::{add_goal, demote_goal, goals, promote_backlog_item, remove_goal, seed_goals, update_goal_status};
use crate::agent_registry::{AgentRegistry, FileBackedAgentRegistry};
use crate::build_lock::BuildLock;
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory, as_f64, as_i64, as_str};
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
        .route("/api/goals/demote/{id}", post(demote_goal))
        .route("/api/goals/{id}", delete(remove_goal))
        .route("/api/goals/{id}/status", put(update_goal_status))
        .route("/api/distributed", get(distributed))
        .route("/api/vm/vacate", post(vacate_vm))
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
        .route("/api/agent-graph", get(agent_graph))
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
        .route("/api/subagent-sessions", get(subagent_sessions))
        .route("/ws/chat", get(ws_chat_handler))
        .route(WS_AGENT_LOG_ROUTE, get(ws_agent_log_handler))
        .route("/api/azlin/tmux-sessions", get(azlin_tmux_sessions))
        .route(
            "/ws/tmux_attach/{host}/{session}",
            get(ws_tmux_attach_handler),
        )
        .route("/api/login", post(login))
        .route("/login", get(login_page))
        .route("/", get(index))
        .layer(middleware::from_fn(require_auth))
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








pub(crate) fn is_pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Run a `gh` CLI command and parse JSON output, returning a `Value`.
pub(crate) async fn run_gh_json(args: &[&str]) -> Value {
    match tokio::process::Command::new("gh").args(args).output().await {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            serde_json::from_str::<Value>(&raw).unwrap_or(json!([]))
        }
        _ => json!([]),
    }
}

/// Read the most recent N cycle report files from disk.
/// Truncates `s` to at most `max` Unicode characters, appending `…` if the
/// string was shortened. Pure helper; no allocation when no truncation needed.
pub(crate) fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}





/// Vacate a remote VM: stop Simard processes and export memory snapshot.
///
/// Steps:
/// 1. Connect via azlin and stop simard-ooda service
/// 2. Kill any remaining simard/cargo processes
/// 3. Export cognitive memory snapshot (if available)
/// 4. Remove from configured hosts

/// Strip ANSI escape sequences (CSI, OSC, and single-char escapes) so that
/// output from azlin/SSH can be reliably parsed for KEY=value markers.


async fn index() -> axum::response::Html<String> {
    axum::response::Html(INDEX_HTML.to_string())
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

    // Collect cycle reports for the logs tab
    let mut cycle_reports: Vec<Value> = Vec::new();
    let cycle_dir = state_root.join("cycle_reports");
    if let Ok(entries) = std::fs::read_dir(&cycle_dir) {
        let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        files.sort_by_key(|e| e.path());
        for entry in files.into_iter() {
            let path = entry.path();
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(report) = serde_json::from_str::<Value>(&content)
            {
                cycle_reports.push(report);
            }
        }
    }

    Json(json!({
        "daemon_log_lines": combined_log,
        "ooda_transcripts": transcripts,
        "terminal_transcripts": terminal_transcripts,
        "cost_log_lines": cost_log,
        "cycle_reports": cycle_reports,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

pub(crate) fn read_tail(path: &str, max_lines: usize) -> Option<Vec<String>> {
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
        .args(["axo", "pid,ppid,etime,comm,args"])
        .output()
        .await;

    let mut procs: Vec<Value> = Vec::new();
    let mut root_pid: Option<String> = None;

    if let Ok(o) = output {
        let text = String::from_utf8_lossy(&o.stdout);

        // Phase 1: Parse every process into a row.
        struct PsRow {
            pid: String,
            ppid: String,
            etime: String,
            comm: String,
            full_args: String,
        }
        let mut all_rows: Vec<PsRow> = Vec::new();
        // Map parent-pid -> indices of direct children for fast descendant walk.
        let mut children_map: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();

        for line in text.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                let idx = all_rows.len();
                let row = PsRow {
                    pid: parts[0].to_string(),
                    ppid: parts[1].to_string(),
                    etime: parts[2].to_string(),
                    comm: parts[3].to_string(),
                    full_args: parts[4..].join(" "),
                };
                children_map.entry(row.ppid.clone()).or_default().push(idx);
                all_rows.push(row);
            }
        }

        // Phase 2: Locate the OODA daemon – the process whose args contain
        // "simard" AND "ooda" AND "run" (i.e. `simard ooda run`).
        let mut ooda_idx: Option<usize> = None;
        for (i, row) in all_rows.iter().enumerate() {
            let lower = row.full_args.to_lowercase();
            if lower.contains("simard") && lower.contains("ooda") && lower.contains("run") {
                ooda_idx = Some(i);
                break;
            }
        }

        // Phase 3: BFS from the OODA daemon to collect it + all descendants.
        if let Some(start) = ooda_idx {
            let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
            let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
            queue.push_back(start);
            visited.insert(start);
            root_pid = Some(all_rows[start].pid.clone());

            while let Some(idx) = queue.pop_front() {
                let row = &all_rows[idx];
                let is_root = idx == start;
                procs.push(json!({
                    "pid": row.pid,
                    "ppid": row.ppid,
                    "uptime": row.etime,
                    "command": row.comm,
                    "full_args": row.full_args,
                    "is_ooda_root": is_root,
                }));

                if let Some(kids) = children_map.get(&row.pid) {
                    for &child_idx in kids {
                        if visited.insert(child_idx) {
                            queue.push_back(child_idx);
                        }
                    }
                }
            }
        }
    }

    Json(json!({
        "processes": procs,
        "count": procs.len(),
        "root_pid": root_pid,
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
// Agent Graph API (#951)
// Returns force-directed-friendly topology: OODA -> engineers -> sessions.
// Pure builder is unit-tested; HTTP handler sources live data from the
// existing FileBackedAgentRegistry.
// ---------------------------------------------------------------------------



async fn agent_graph() -> Json<Value> {
    let reg = FileBackedAgentRegistry::new(&resolve_state_root());
    match reg.list() {
        Ok(entries) => Json(build_agent_graph(&entries)),
        Err(e) => Json(json!({
            "error": e.to_string(),
            "nodes": [],
            "edges": [],
        })),
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

pub(crate) fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}

// ---------------------------------------------------------------------------
// Issue #947 — Agent terminal widget: WS endpoint, sanitizer, and tail loop.
// ---------------------------------------------------------------------------

/// WebSocket route path for tailing per-agent stdout/stderr logs.
///
/// Registered inside the `require_auth` middleware scope by `build_router`.
pub(crate) const WS_AGENT_LOG_ROUTE: &str = "/ws/agent_log/{agent_name}";

/// Validate `agent_name` against allow-list `^[A-Za-z0-9_-]{1,64}$`.
///
/// This is the sole defense against path traversal (INV-7): any byte that is
/// not in the allow-list (including `/`, `\`, `.`, NUL, control chars, and
/// non-ASCII) causes rejection with `None`. No filesystem-side canonicalization
/// is performed — the regex shape is sufficient to keep names confined to a
/// single path component within `agent_logs/`.
pub(crate) fn sanitize_agent_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return None;
    }
    for &b in bytes {
        let ok = b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
        if !ok {
            return None;
        }
    }
    Some(name.to_string())
}

/// Build the per-agent log file path: `<state_root>/agent_logs/<name>.log`.
///
/// Caller is responsible for sanitizing `name` first via
/// [`sanitize_agent_name`]. Combined with the allow-list, the resulting path
/// is guaranteed to be a direct child of `<state_root>/agent_logs/`.
pub(crate) fn agent_log_path(state_root: &std::path::Path, name: &str) -> std::path::PathBuf {
    state_root.join("agent_logs").join(format!("{name}.log"))
}

async fn ws_agent_log_handler(
    Path(agent_name): Path<String>,
    ws: WebSocketUpgrade,
) -> response::Response {
    let Some(safe) = sanitize_agent_name(&agent_name) else {
        return response::Response::builder()
            .status(400)
            .header("content-type", "text/plain; charset=utf-8")
            .body(axum::body::Body::from(
                "invalid agent_name: must match ^[A-Za-z0-9_-]{1,64}$",
            ))
            .unwrap();
    };
    let path = agent_log_path(&resolve_state_root(), &safe);
    ws.on_upgrade(move |socket| handle_agent_log_ws(socket, path))
}

/// Maximum number of lines sent during the initial backfill.
const AGENT_LOG_BACKFILL_LINES: usize = 200;
/// Maximum bytes read per polling tick (DoS bound on burst writes).
const AGENT_LOG_MAX_TICK_BYTES: u64 = 1_048_576; // 1 MiB
/// Polling interval for new bytes appended to the log.
const AGENT_LOG_TICK_MS: u64 = 200;
/// Maximum time to wait for the log file to appear before giving up.
const AGENT_LOG_WAIT_TIMEOUT_MS: u64 = 30_000;

async fn handle_agent_log_ws(mut socket: WebSocket, path: std::path::PathBuf) {
    use std::io::SeekFrom;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio::time::{Duration, sleep};

    // Phase 1: wait for the log file to appear (supervisor may not have
    // spawned the agent yet). Poll every tick up to the timeout.
    let waited_ms = wait_for_file(&path).await;
    if waited_ms.is_none() {
        let _ = socket
            .send(Message::Text(
                "[simard] no log file for this agent yet (timed out waiting). The agent may not be running.\n"
                    .to_string()
                    .into(),
            ))
            .await;
        let _ = socket.send(Message::Close(None)).await;
        return;
    }

    // Phase 2: backfill the last N lines using the existing helper, so the
    // viewer immediately sees recent context.
    let path_str = path.to_string_lossy().to_string();
    let backfill = read_tail(&path_str, AGENT_LOG_BACKFILL_LINES).unwrap_or_default();
    for line in backfill {
        if socket
            .send(Message::Text(format!("{line}\n").into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Phase 3: stream new appends. Open the file and seek to its current end
    // so we don't double-deliver the backfill lines.
    let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("[simard] could not open log: {e}\n").into(),
                ))
                .await;
            return;
        }
    };
    let mut pos = file.seek(SeekFrom::End(0)).await.unwrap_or(0);
    // Buffer trailing partial line until we see its newline.
    let mut partial: Vec<u8> = Vec::new();

    loop {
        // If the client sent anything (typically a close), drain it.
        if let Ok(maybe_msg) = tokio::time::timeout(Duration::from_millis(1), socket.recv()).await {
            match maybe_msg {
                Some(Ok(Message::Close(_))) | None => return,
                Some(Err(_)) => return,
                _ => {} // ignore other inbound frames (server→client only)
            }
        }

        // Detect truncation/rotation: if file shrinks below our position,
        // reset to start and drop any partial line buffered.
        let len = match tokio::fs::metadata(&path).await {
            Ok(m) => m.len(),
            Err(_) => {
                // Transient stat failure — try again next tick.
                sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
                continue;
            }
        };
        if len < pos {
            partial.clear();
            pos = 0;
            let _ = socket
                .send(Message::Text(
                    "[simard] log file truncated; resetting tail position\n"
                        .to_string()
                        .into(),
                ))
                .await;
        }

        let available = len.saturating_sub(pos);
        if available > 0 {
            let to_read = available.min(AGENT_LOG_MAX_TICK_BYTES);
            if file.seek(SeekFrom::Start(pos)).await.is_err() {
                sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
                continue;
            }
            let mut buf = vec![0u8; to_read as usize];
            match file.read_exact(&mut buf).await {
                Ok(_) => {
                    pos += to_read;
                    partial.extend_from_slice(&buf);
                    // Emit one frame per complete line.
                    while let Some(nl) = partial.iter().position(|&b| b == b'\n') {
                        let line_bytes = partial.drain(..=nl).collect::<Vec<u8>>();
                        // Strip trailing \n (and \r if present) for the frame;
                        // the client adds its own line break via writeln.
                        let mut line = String::from_utf8_lossy(&line_bytes).into_owned();
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        if line.ends_with('\r') {
                            line.pop();
                        }
                        if socket.send(Message::Text(line.into())).await.is_err() {
                            return;
                        }
                    }
                }
                Err(_) => {
                    sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
                    continue;
                }
            }
        } else {
            sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
        }
    }
}

/// Poll for `path` to exist. Returns `Some(elapsed_ms)` on success or `None`
/// if the timeout is reached.
async fn wait_for_file(path: &std::path::Path) -> Option<u64> {
    use tokio::time::{Duration, Instant, sleep};
    let start = Instant::now();
    loop {
        if tokio::fs::metadata(path).await.is_ok() {
            return Some(start.elapsed().as_millis() as u64);
        }
        if start.elapsed() >= Duration::from_millis(AGENT_LOG_WAIT_TIMEOUT_MS) {
            return None;
        }
        sleep(Duration::from_millis(AGENT_LOG_TICK_MS)).await;
    }
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


/// WS-2: list live and recently-ended subagent tmux sessions.
///
/// Returns `{ live: [...], recently_ended: [...] }` sorted by `created_at`
/// descending. The dashboard polls this every 5s to populate the Subagent
/// Sessions card and to drive Attach deep-links in the Recent Actions feed.
async fn subagent_sessions() -> Json<Value> {
    let reg = crate::subagent_sessions::load();
    let mut live: Vec<&crate::subagent_sessions::SubagentSession> = reg
        .sessions
        .iter()
        .filter(|s| s.ended_at.is_none())
        .collect();
    let mut ended: Vec<&crate::subagent_sessions::SubagentSession> = reg
        .sessions
        .iter()
        .filter(|s| s.ended_at.is_some())
        .collect();
    live.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    ended.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    Json(json!({
        "live": live,
        "recently_ended": ended,
    }))
}

pub(crate) const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard Dashboard v2</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.min.css">
  <script src="https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.min.js"></script>
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
    .loading{color:#8b949e;font-style:italic;display:inline-flex;align-items:center;gap:.5rem}
    .loading::before{content:'';width:1rem;height:1rem;border:2px solid #30363d;border-top-color:#58a6ff;border-radius:50%;animation:spin .8s linear infinite;flex-shrink:0}
    @keyframes spin{to{transform:rotate(360deg)}}
    .log-box{background:#010409;border:1px solid var(--border);border-radius:6px;padding:.75rem;font-family:'SF Mono','Fira Code',monospace;font-size:.8rem;max-height:500px;overflow-y:auto;white-space:pre-wrap;word-break:break-all;line-height:1.4;color:#8b949e}
    .transcript-item{background:var(--card);border:1px solid var(--border);border-radius:6px;padding:.75rem;margin-bottom:.5rem}
    .transcript-item h3{font-size:.85rem;color:var(--accent);margin-bottom:.4rem}
    .proc-table{width:100%;border-collapse:collapse;font-size:.85rem}
    .proc-table th{text-align:left;color:#8b949e;padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table td{padding:.4rem .6rem;border-bottom:1px solid var(--border)}
    .proc-table tr:last-child td{border-bottom:none}
    .proc-tree .proc-row{display:flex;gap:.5rem;align-items:baseline;padding:.25rem .5rem;border-bottom:1px solid var(--border);font-family:monospace;font-size:.82rem}
    .proc-tree .proc-row:hover{background:rgba(88,166,255,0.05)}
    .proc-tree .proc-pid{color:var(--accent);min-width:4rem;font-weight:600}
    .proc-tree .proc-uptime{min-width:6rem}
    .proc-tree .proc-kids.collapsed{display:none}
    .proc-tree .proc-kids{border-left:1px solid #30363d;margin-left:8px}
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
      <a href="https://github.com/rysweet/Simard" target="_blank" style="color:#8b949e;text-decoration:none;font-size:.85rem;padding:.2rem .4rem" title="Source on GitHub">⟨/⟩ Source</a>
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
    <div class="tab" data-tab="terminal">Terminal</div>
  </div>

  <div class="tab-content active" id="tab-overview">
    <div class="card" style="margin-bottom:1rem;border:1px solid #238636;background:linear-gradient(135deg,#0d1117,#0f1a12)">
      <h2 style="color:#3fb950;margin-bottom:.75rem">🤖 Simard — Autonomous Agent</h2>
      <div id="agent-live-status"><span class="loading">Loading agent status…</span></div>
    </div>
    <div class="grid">
      <div class="card">
        <h2>Recent Actions <button class="btn" onclick="fetchStatus()" style="font-size:.75rem">Refresh</button></h2>
        <div id="recent-actions-list"><span class="loading">Loading…</span></div>
      </div>
      <div class="card">
        <h2>Open PRs</h2>
        <div id="open-prs-list"><span class="loading">Loading…</span></div>
      </div>
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
    <div class="card" style="margin-bottom:1rem">
      <h2>Cycle Reports</h2>
      <div id="cycle-reports"><span class="loading">Loading…</span></div>
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
    <div class="card" style="margin-top:1rem">
      <h2>Process Tree <button class="btn" onclick="fetchProcessTree()">Refresh</button></h2>
      <div id="proc-tree-summary" style="margin-bottom:.5rem;color:#8b949e;font-size:.85rem"></div>
      <div id="proc-tree-container"><span class="loading">Loading…</span></div>
    </div>
  </div>

  <div class="tab-content" id="tab-memory">
    <div style="display:flex;align-items:center;gap:1rem;margin-bottom:1rem">
      <h2 style="margin:0">Memory</h2>
      <span id="mem-graph-stats" style="color:#8b949e;font-size:.8rem;margin-left:auto"></span>
      <button class="btn" onclick="fetchMemoryGraph()" style="font-size:.75rem">Refresh Graph</button>
    </div>

    <div id="mem-graph-panel">
      <div class="card" style="margin-bottom:.5rem;padding:.5rem .75rem">
        <div style="display:flex;gap:1rem;flex-wrap:wrap;align-items:center;font-size:.8rem">
          <label style="color:#f0883e"><input type="checkbox" class="mem-filter" data-type="WorkingMemory" checked> Working</label>
          <label style="color:#58a6ff"><input type="checkbox" class="mem-filter" data-type="SemanticFact" checked> Semantic</label>
          <label style="color:#3fb950"><input type="checkbox" class="mem-filter" data-type="EpisodicMemory" checked> Episodic</label>
          <label style="color:#a371f7"><input type="checkbox" class="mem-filter" data-type="ProceduralMemory" checked> Procedural</label>
          <label style="color:#d29922"><input type="checkbox" class="mem-filter" data-type="ProspectiveMemory" checked> Prospective</label>
          <label style="color:#8b949e"><input type="checkbox" class="mem-filter" data-type="SensoryBuffer" checked> Sensory</label>
        </div>
      </div>
      <div style="display:flex;gap:1rem">
        <div class="card" style="flex:1;padding:0;position:relative;min-height:60vh">
          <canvas id="mem-graph-canvas" style="width:100%;height:60vh;display:block;cursor:grab"></canvas>
          <div id="mem-graph-tooltip" style="display:none;position:absolute;background:#161b22;border:1px solid #30363d;border-radius:6px;padding:.5rem .75rem;font-size:.8rem;max-width:320px;pointer-events:none;z-index:10;word-break:break-word"></div>
        </div>
        <div id="mem-graph-detail" class="card" style="width:280px;display:none">
          <h2 id="mg-detail-title">Node Details</h2>
          <div id="mg-detail-body"></div>
        </div>
      </div>
    </div>

    <div style="display:flex;gap:1rem;margin-top:1rem">
      <div class="card" style="flex:1">
        <h2>Memory Search</h2>
        <div style="display:flex;gap:.5rem;align-items:center;margin-bottom:.75rem">
          <input id="mem-search-input" placeholder="Search memories…" style="flex:1;padding:6px;background:#1a1a2e;border:1px solid #333;color:#e0e0e0;border-radius:4px">
          <button class="btn" onclick="searchMemory()">Search</button>
        </div>
        <div id="mem-search-results"></div>
      </div>
      <div class="card" style="flex:1"><h2>Memory Overview</h2><div id="mem-overview"><span class="loading">Loading…</span></div></div>
      <div class="card" style="flex:1"><h2>Memory Files</h2><div id="mem-files"><span class="loading">Loading…</span></div></div>
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

  <div class="tab-content" id="tab-terminal">
    <div class="card" style="max-width:980px">
      <h2>Agent Terminal</h2>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.6rem;margin-bottom:.75rem;font-size:.8rem;color:#8b949e">
        Stream the live stdout/stderr of a running subordinate agent. The viewer
        reconnects each time you click <strong>Connect</strong>; close the WS
        with <strong>Disconnect</strong>.
      </div>
      <div style="display:flex;gap:.5rem;align-items:center;flex-wrap:wrap;margin-bottom:.75rem">
        <label for="agent-log-name" style="color:#8b949e;font-size:.85rem">Agent name</label>
        <input id="agent-log-name" type="text" placeholder="e.g. planner" maxlength="64"
               style="padding:.35rem .5rem;background:var(--bg);border:1px solid var(--border);border-radius:4px;color:var(--fg);font-family:monospace;min-width:14rem">
        <button class="btn" id="agent-log-connect" onclick="connectAgentLog()">Connect</button>
        <button class="btn" id="agent-log-disconnect" onclick="disconnectAgentLog()">Disconnect</button>
        <span id="agent-log-status" style="color:#8b949e;font-size:.85rem">Not connected</span>
      </div>
      <div id="xterm-host" style="height:60vh;background:#000;border:1px solid var(--border);border-radius:6px;padding:.25rem"></div>
    </div>
    <div class="card" style="max-width:980px" id="subagent-sessions">
      <h2>Subagent Sessions</h2>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.6rem;margin-bottom:.75rem;font-size:.8rem;color:#8b949e">
        Live and recently-ended engineer subprocesses tracked via tmux.
        Click <strong>Attach</strong> to copy the <code>tmux attach</code>
        command for the corresponding <code>simard-engineer-&lt;id&gt;</code>
        session.
      </div>
      <div id="subagent-sessions-list">
        <span style="color:#8b949e;font-size:.85rem">Loading…</span>
      </div>
    </div>

    <section id="azlin-sessions-panel" class="card" style="max-width:980px;margin-top:1rem">
      <div style="display:flex;justify-content:space-between;align-items:center;flex-wrap:wrap;gap:.5rem">
        <h2 style="margin:0">Azlin Tmux Sessions</h2>
        <div style="display:flex;gap:.5rem;align-items:center;font-size:.85rem;color:#8b949e">
          <span>Last refreshed:</span>
          <span id="tmux-last-refreshed" data-testid="tmux-last-refreshed">—</span>
          <button class="btn" data-testid="tmux-refresh" onclick="fetchTmuxSessions()">Refresh</button>
        </div>
      </div>
      <div style="background:#1a1a2e;border:1px solid #333;border-radius:6px;padding:.6rem;margin-top:.6rem;font-size:.8rem;color:#8b949e">
        Per-host listing of <code>tmux list-sessions</code> across configured azlin hosts.
        Click <strong>Open</strong> to attach a session into the terminal viewer above.
        Auto-refreshes every 10 s while this tab is active.
      </div>
      <div id="tmux-sessions-body" style="margin-top:.6rem">
        <div style="color:#8b949e;font-size:.85rem">Loading…</div>
      </div>
    </section>
  </div>

  <script>
    /* --- Helpers --- */
    function fmtB(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';return(b/1048576).toFixed(1)+' MB';}
    function esc(s){if(s==null)return'';const d=document.createElement('div');d.textContent=String(s);return d.innerHTML;}
    async function apiFetch(url,opts){
      const r=await fetch(url,opts);
      if(r.status===401){window.location.href='/login';throw new Error('Session expired — redirecting to login');}
      if(!r.ok){const t=await r.text();throw new Error(t||('HTTP '+r.status));}
      const text=await r.text();
      if(!text)return {};
      return JSON.parse(text);
    }
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

    /* --- WS-2: Subagent tmux session registry (cached client-side) --- */
    let subagentSessionsCache={live:[],recently_ended:[],byId:{}};
    function rebuildSubagentIndex(){
      const idx={};
      for(const s of (subagentSessionsCache.live||[])){idx[s.agent_id]=s;}
      for(const s of (subagentSessionsCache.recently_ended||[])){if(!idx[s.agent_id])idx[s.agent_id]=s;}
      subagentSessionsCache.byId=idx;
    }
    async function fetchSubagentSessions(){
      try{
        const d=await apiFetch('/api/subagent-sessions');
        subagentSessionsCache.live=d.live||[];
        subagentSessionsCache.recently_ended=d.recently_ended||[];
        rebuildSubagentIndex();
        renderSubagentSessions();
      }catch(e){
        const el=document.getElementById('subagent-sessions-list');
        if(el) el.innerHTML='<span class="err">Failed to load subagent sessions: '+esc(e.message||e)+'</span>';
      }
    }
    function attachCommandFor(s){
      if(s.host && s.host!=='local'){
        return 'ssh '+s.host+' -t tmux attach -t '+s.session_name;
      }
      return 'tmux attach -t '+s.session_name;
    }
    function renderSubagentSessions(){
      const el=document.getElementById('subagent-sessions-list');
      if(!el) return;
      const live=subagentSessionsCache.live||[];
      const ended=subagentSessionsCache.recently_ended||[];
      if(!live.length && !ended.length){
        el.innerHTML='<span style="color:#8b949e;font-size:.85rem">No subagent sessions tracked yet.</span>';
        return;
      }
      const row=(s,status)=>{
        const cmd=attachCommandFor(s);
        return '<div style="display:flex;gap:.5rem;align-items:baseline;padding:.35rem 0;border-bottom:1px solid var(--border);font-size:.85rem">'
          +'<code style="min-width:14rem">'+esc(s.agent_id)+'</code>'
          +'<span style="color:#8b949e;min-width:8rem">'+esc(s.goal_id||'')+'</span>'
          +'<span class="'+(status==='live'?'ok':'warn')+'" style="min-width:5rem">'+status+'</span>'
          +'<span style="flex:1;color:#8b949e;font-size:.75rem">pid '+s.pid+' · '+esc(s.host||'local')+'</span>'
          +'<button class="btn attach-btn" data-cmd="'+esc(cmd)+'" onclick="copyAttachCmd(this)">Attach →</button>'
          +'</div>';
      };
      el.innerHTML=live.map(s=>row(s,'live')).join('')+ended.map(s=>row(s,'ended')).join('');
    }
    function copyAttachCmd(btn){
      const cmd=btn.getAttribute('data-cmd')||'';
      navigator.clipboard.writeText(cmd).then(()=>{
        const prev=btn.textContent;btn.textContent='Copied!';
        setTimeout(()=>{btn.textContent=prev;},900);
      },()=>{});
    }
    /* Shared renderer for Recent Actions outcome.detail strings.
       Detects agent='engineer-...' references and, when a matching tmux
       session is in the registry cache, swaps the literal substring for an
       inline Attach button. Returns an HTML string (caller already escaped
       the detail). */
    function renderActionDetail(detail){
      const safe=esc(detail||'');
      const re=/agent='(engineer-[A-Za-z0-9_-]+)'/;
      const m=safe.match(re);
      if(!m) return safe;
      const agentId=m[1];
      const session=subagentSessionsCache.byId[agentId];
      if(!session) return safe;
      const cmd=attachCommandFor(session);
      const btn=' <button class="btn attach-btn" data-cmd="'+esc(cmd)+'" onclick="copyAttachCmd(this)" style="font-size:.7rem;padding:.05rem .35rem;margin-left:.25rem">Attach →</button>';
      return safe.replace(m[0], m[0]+btn);
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
        if(tab.dataset.tab==='memory') {fetchMemoryGraph();fetchMemory();}

        if(tab.dataset.tab==='goals') fetchGoals();
        if(tab.dataset.tab==='costs') fetchCosts();
        if(tab.dataset.tab==='traces') fetchTraces();
        if(tab.dataset.tab==='chat') initChat();
        if(tab.dataset.tab==='workboard') {fetchWorkboard();tabRefreshTimers.wb=setInterval(fetchWorkboard,30000);}
        if(tab.dataset.tab==='thinking') {fetchThinking();tabRefreshTimers.thinking=setInterval(fetchThinking,30000);}
        if(tab.dataset.tab==='terminal') {initAgentLogTerminal();fetchSubagentSessions();tabRefreshTimers.subagent=setInterval(fetchSubagentSessions,5000);fetchTmuxSessions();tabRefreshTimers.tmux=setInterval(fetchTmuxSessions,10000);}
      });
    });
    setInterval(()=>{document.getElementById('clock').textContent=new Date().toLocaleString()},1000);

    /* --- Status --- */
    async function fetchStatus(){
      try{
        const d=await apiFetch('/api/status');
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

    async function fetchAgentOverview(){
      try{
        const d=await apiFetch('/api/activity');
        const el=document.getElementById('agent-live-status');
        const daemon=d.daemon||{};
        const isRunning=daemon.status==='healthy';
        const heartbeat=daemon.last_heartbeat?timeAgo(daemon.last_heartbeat):'never';
        const cycle=daemon.current_cycle||'?';

        // Staleness check: if heartbeat is >10 min old, daemon may be hung
        let isStale=false;
        if(isRunning && daemon.last_heartbeat){
          const hbAge=Date.now()-new Date(daemon.last_heartbeat).getTime();
          isStale=hbAge>10*60*1000;
        }

        // Extract actual actions from the most recent structured cycle report
        let latestActions=[];
        const cycles=d.recent_cycles||[];
        for(const c of cycles){
          const rpt=c.report||{};
          if(rpt.outcomes?.length){
            latestActions=rpt.outcomes;
            break;
          }
        }

        // Find what the agent is currently working on from latest priorities
        let currentFocus='';
        for(const c of cycles){
          const rpt=c.report||{};
          if(rpt.priorities?.length){
            const top=rpt.priorities[0];
            currentFocus=`<strong>${esc(top.goal_id)}</strong> — ${esc(top.reason)} <span style="color:${top.urgency>0.7?'var(--red)':top.urgency>0.4?'var(--yellow)':'var(--green)'}">urgency ${top.urgency.toFixed(2)}</span>`;
            break;
          }
        }

        el.innerHTML=`
          <div style="display:flex;gap:2rem;flex-wrap:wrap;align-items:center;margin-bottom:.75rem">
            <div><span style="font-size:1.5rem;${isRunning&&!isStale?'':'filter:grayscale(1)'}">${isRunning?(isStale?'🟡':'🟢'):'🔴'}</span> <strong style="font-size:1.1rem">${isRunning?(isStale?'Agent Stale':'OODA Loop Active'):'Agent Stopped'}</strong></div>
            <div style="color:#8b949e">Cycle <strong style="color:var(--fg)">#${cycle}</strong> · Last heartbeat <strong style="color:var(--fg)">${heartbeat}</strong>${isStale?' <span style="color:var(--yellow)">(>10 min ago)</span>':''}</div>
          </div>
          ${currentFocus?`<div style="margin-bottom:.75rem"><span style="color:#8b949e">🎯 Top Priority:</span> ${currentFocus}</div>`:''}
          ${latestActions.length?`
            <div style="font-size:.85rem">
              <div style="color:#8b949e;margin-bottom:.3rem;font-weight:600">Last Cycle Actions:</div>
              ${latestActions.map(o=>`
                <div style="padding:.2rem 0;display:flex;gap:.5rem;align-items:baseline">
                  <span>${o.success?'✅':'❌'}</span>
                  <code style="color:var(--accent)">${esc(o.action_kind||'')}</code>
                  <span>${esc(o.action_description||'')}</span>
                  ${o.detail?'<span style="color:#8b949e;font-size:.8rem;max-width:400px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:inline-block">'+esc(o.detail.substring(0,120))+'</span>':''}
                </div>`).join('')}
            </div>`:'<div style="color:#8b949e">No recent actions recorded.</div>'}`;

        // Open PRs
        const prs=d.open_prs||[];
        const prEl=document.getElementById('open-prs-list');
        if(prs.length){
          prEl.innerHTML=prs.slice(0,8).map(pr=>`
            <div style="padding:.3rem 0;border-bottom:1px solid var(--border);font-size:.85rem;display:flex;gap:.5rem;align-items:baseline">
              <a href="${esc(pr.url)}" target="_blank" style="color:var(--accent);text-decoration:none;min-width:3rem">#${pr.number}</a>
              <span style="flex:1">${esc(pr.title)}</span>
              <span style="color:#8b949e;font-size:.75rem">${timeAgo(pr.createdAt)}</span>
            </div>`).join('')+
            (prs.length>8?`<div style="color:#8b949e;font-size:.8rem;margin-top:.3rem">+ ${prs.length-8} more</div>`:'');
        }else{
          prEl.innerHTML='<span style="color:#8b949e">No open PRs</span>';
        }

        // Recent actions from cycle outcomes
        const actEl=document.getElementById('recent-actions-list');
        let allActions=[];
        for(const c of cycles.slice(0,5)){
          const rpt=c.report||{};
          const num=rpt.cycle_number||c.cycle_number||'?';
          for(const o of (rpt.outcomes||[])){
            allActions.push({cycle:num,...o});
          }
        }
        if(allActions.length){
          actEl.innerHTML=allActions.slice(0,15).map(a=>`
            <div style="padding:.25rem 0;border-bottom:1px solid var(--border);font-size:.85rem;display:flex;gap:.5rem;align-items:baseline">
              <span style="color:var(--accent);min-width:2rem;font-weight:600">#${a.cycle}</span>
              <span>${a.success?'✅':'❌'}</span>
              <code>${esc(a.action_kind||'')}</code>
              <span style="flex:1">${renderActionDetail((function(){var arr=Array.from(a.detail||'');var d=arr.length>200?arr.slice(0,200).join('')+'…':arr.join('');return d||a.action_description||'';})())}</span>
            </div>`).join('');
        }else{
          actEl.innerHTML='<span style="color:#8b949e">No structured action history yet. The OODA daemon records actions each cycle.</span>';
        }
      }catch(e){
        console.warn('fetchAgentOverview failed:', e);
        const el=document.getElementById('agent-live-status');
        if(el) el.innerHTML='<span class="err">Failed to load agent status</span>';
      }
    }

    /* --- Issues --- */
    async function fetchIssues(){
      try{
        const data=await apiFetch('/api/issues');
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
        const d=await apiFetch('/api/logs');
        allLogLines=d.daemon_log_lines||[];
        applyLogFilter();
        // Issue #928: guard each element access so a missing target on the
        // current tab does not abort the whole fetchLogs and leave every
        // panel stuck on "Loading…".
        const tEl=document.getElementById('ooda-transcripts');
        if(tEl){
          if(d.ooda_transcripts?.length){
            tEl.innerHTML=d.ooda_transcripts.map(t=>`
              <div class="transcript-item">
                <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
                <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
              </div>`).join('');
          }else{tEl.innerHTML='<span style="color:#8b949e">No OODA transcripts found in state root.</span>';}
        }
        // Render cycle reports
        const crEl=document.getElementById('cycle-reports');
        if(crEl){
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
        }
        const ttEl=document.getElementById('terminal-transcripts');
        if(ttEl){
          if(d.terminal_transcripts?.length){
            ttEl.innerHTML=d.terminal_transcripts.map(t=>`
              <div class="transcript-item">
                <h3>${esc(t.name)} <span class="badge">${fmtB(t.size_bytes)}</span></h3>
                <div class="log-box" style="max-height:200px">${esc((t.preview_lines||[]).join('\n'))||'(empty)'}</div>
              </div>`).join('');
          }else{ttEl.innerHTML='<span style="color:#8b949e">No terminal session transcripts found.</span>';}
        }
        const costEl=document.getElementById('cost-log-box');
        if(costEl){
          if(d.cost_log_lines?.length){
            costEl.textContent=d.cost_log_lines.join('\n');
            costEl.scrollTop=costEl.scrollHeight;
          }else{costEl.innerHTML='<span style="color:#8b949e">No cost ledger entries</span>';}
        }
      }catch(e){const dl=document.getElementById('daemon-log'); if(dl){dl.textContent='Failed to load logs — check /api/logs endpoint';}}
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
        const d=await apiFetch('/api/processes');
        const container = document.getElementById('proc-tree-container');
        const summary = document.getElementById('proc-tree-summary');
        if (!container) return;
        const procs = d.processes || [];
        if (procs.length) {
          const rootLabel = d.root_pid ? ` — OODA daemon PID ${d.root_pid}` : '';
          if (summary) summary.textContent = `${procs.length} process(es)${rootLabel} — updated ${timeAgo(d.timestamp)}`;
          // Build tree from flat list using ppid
          const byPid = {};
          procs.forEach(p => { byPid[p.pid] = { ...p, children: [] }; });
          const roots = [];
          // The OODA root's ppid won't be in our set, so it becomes a root.
          // Any other process whose ppid isn't in our set is also a root,
          // but with the descendant-walk backend this should only be the daemon.
          procs.forEach(p => {
            const node = byPid[p.pid];
            if (p.ppid && byPid[p.ppid]) {
              byPid[p.ppid].children.push(node);
            } else {
              roots.push(node);
            }
          });
          function renderNode(n, depth) {
            const indent = depth * 20;
            const hasKids = n.children.length > 0;
            const toggle = hasKids
              ? `<span class="proc-toggle" onclick="this.parentElement.parentElement.querySelector('.proc-kids').classList.toggle('collapsed');this.textContent=this.textContent==='▼'?'▶':'▼'" style="cursor:pointer;user-select:none;width:1em;display:inline-block">▼</span>`
              : `<span style="width:1em;display:inline-block;color:#484f58">·</span>`;
            const isRoot = n.is_ooda_root === true;
            const label = isRoot ? '🤖 Simard OODA Daemon' : '';
            const cmd = esc(n.full_args || n.command || '');
            const cmdShort = cmd.length > 90 ? cmd.substring(0,87)+'…' : cmd;
            const rootBadge = isRoot ? `<span style="background:#238636;color:#fff;padding:1px 6px;border-radius:4px;font-size:.75rem;margin-right:4px">${label}</span>` : '';
            let html = `<div class="proc-row" style="padding-left:${indent}px">
              ${toggle}
              <span class="proc-pid">${esc(n.pid)}</span>
              ${rootBadge}
              <span class="proc-uptime" style="color:#8b949e;font-size:.8rem;min-width:80px">${esc(n.uptime||'')}</span>
              <span class="proc-cmd" title="${cmd}" style="color:#c9d1d9">${cmdShort}</span>
            </div>`;
            if (hasKids) {
              html += '<div class="proc-kids">';
              n.children.forEach(c => { html += renderNode(c, depth+1); });
              html += '</div>';
            }
            return html;
          }
          container.innerHTML = '<div class="proc-tree">' + roots.map(r => renderNode(r, 0)).join('') + '</div>';
        } else {
          if (summary) summary.textContent = d.timestamp ? `Updated ${timeAgo(d.timestamp)}` : '';
          container.innerHTML = '<span style="color:#8b949e">No Simard-related processes found. Is the daemon running?</span>';
        }
      } catch(e) {
        const c = document.getElementById('proc-tree-container');
        if (c) c.innerHTML = '<span class="err">Failed to load process tree: ' + esc(e.toString()) + '</span>';
      }
    }

    /* --- Memory --- */
    async function fetchMemory(){
      try{
        const d=await apiFetch('/api/memory');
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
        const d=await apiFetch('/api/distributed');
        const eb=d.event_bus;
        const emDash='\u2014';
        const fmtTs=v=>(v==null?emDash:v);
        const fmtRate=v=>(v==null?'0':(Math.round(v*100)/100).toString());
        let ebBlock='';
        if(eb){
          const topics=eb.topics||{};
          const rows=Object.keys(topics).sort().map(name=>{
            const t=topics[name]||{};
            return `<li data-testid="event-bus-topic-${esc(name)}">${esc(name)}: ${t.subscribers||0} subs, ${fmtRate(t.events_per_min)}/min, last ${esc(fmtTs(t.last_event_timestamp))}</li>`;
          }).join('');
          ebBlock=`
          <div class="event-bus-stats" style="margin-top:1rem;padding-top:.75rem;border-top:1px solid var(--border)">
            <h3 style="margin:0 0 .5rem 0;color:var(--accent);font-size:1rem">Event Bus</h3>
            <div class="stat" data-testid="event-bus-total-subscribers"><span class="label">Subscribers</span><span class="value">${eb.total_subscribers||0}</span></div>
            <div class="stat" data-testid="event-bus-events-per-min"><span class="label">Events/min</span><span class="value">${fmtRate(eb.events_per_min)}</span></div>
            <div class="stat" data-testid="event-bus-last-event"><span class="label">Last event</span><span class="value">${esc(fmtTs(eb.last_event_timestamp))}</span></div>
            <ul style="margin:.5rem 0 0 1rem;padding:0;font-size:.85rem;color:#8b949e">${rows}</ul>
          </div>`;
        }
        document.getElementById('cluster-topology').innerHTML=`
          <div class="stat"><span class="label">Topology</span><span class="value">${esc(d.topology)}</span></div>
          <div class="stat"><span class="label">Local Host</span><span class="value">${esc(d.local?.hostname||'?')}</span></div>
          <div class="stat"><span class="label">Memory Sync</span><span class="value">${esc(d.hive_mind?.protocol||'DHT+bloom gossip')}</span></div>
          <div class="stat"><span class="label">Hive Status</span><span class="value ${d.hive_mind?.status==='active'?'ok':'warn'}">${esc(d.hive_mind?.status||'standalone')}</span></div>
          ${d.hive_mind?.peers!=null?`<div class="stat"><span class="label">Peers</span><span class="value">${d.hive_mind.peers}</span></div>`:''}
          ${d.hive_mind?.facts_shared!=null?`<div class="stat"><span class="label">Facts Shared</span><span class="value">${d.hive_mind.facts_shared}</span></div>`:''}
          <div class="stat"><span class="label">Updated</span><span class="value">${timeAgo(d.timestamp)}</span></div>${ebBlock}`;
        if(d.remote_vms?.length){
          document.getElementById('remote-vms').innerHTML=d.remote_vms.map(vm=>{
            const sc=vm.status==='reachable'?'ok':(vm.status==='unreachable'?'err':'warn');
            const hasWorkloads=(vm.simard_processes||0)>0||(vm.cargo_processes||0)>0;
            return`<div style="border:1px solid var(--border);border-radius:6px;padding:1rem;margin-bottom:.75rem">
              <div style="display:flex;justify-content:space-between;align-items:center">
                <h3 style="margin:0 0 .5rem 0;color:var(--accent)">${esc(vm.vm_name)} <span class="${sc}" style="font-size:.85rem">${esc(vm.status)}</span></h3>
                <div style="display:flex;gap:.5rem">
                  ${hasWorkloads?`<button class="btn" style="font-size:.75rem;padding:2px 8px" onclick="vacateVM('${esc(vm.vm_name)}')">🚚 Vacate</button>`:''}
                  <button class="btn" style="font-size:.75rem;padding:2px 8px;color:#f85149" onclick="removeVM('${esc(vm.vm_name)}')">✕ Remove</button>
                </div>
              </div>
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
    async function vacateVM(vmName){
      if(!confirm(`Vacate "${vmName}"? This will:\n1. Stop all Simard processes on the VM\n2. Export cognitive memory snapshot\n3. Transfer workloads to this host\n\nProceed?`))return;
      const el=document.getElementById('remote-vms');
      const origHtml=el.innerHTML;
      el.innerHTML=`<span class="loading">Vacating ${esc(vmName)}… stopping processes and exporting memory</span>`;
      try{
        const d=await apiFetch('/api/vm/vacate',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({vm_name:vmName})});
        if(d.status==='ok'){
          el.innerHTML=`<div class="ok" style="padding:1rem">✓ ${esc(vmName)} vacated. ${d.message||''}</div>`;
          setTimeout(fetchDistributed,3000);
        }else{
          el.innerHTML=origHtml;
          alert('Vacate failed: '+(d.error||'unknown error'));
        }
      }catch(e){el.innerHTML=origHtml;alert('Vacate error: '+e);}
    }
    async function removeVM(vmName){
      if(!confirm(`Remove "${vmName}" from the cluster? This only removes it from the dashboard — it does not deallocate the Azure VM.`))return;
      try{
        await apiFetch('/api/hosts',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:vmName})});
        fetchDistributed();
        fetchHosts();
      }catch(e){alert('Remove error: '+e);}
    }
    async function fetchHosts(){
      try{
        const d=await apiFetch('/api/hosts');
        const el=document.getElementById('hosts-list');
        let html='';

        // Discovered VMs from azlin
        const discovered=d.discovered||[];
        const configuredNames=new Set((d.hosts||[]).map(h=>h.name));
        if(discovered.length){
          html+=`<div style="margin-bottom:.75rem"><div style="font-weight:600;font-size:.85rem;margin-bottom:.4rem;color:var(--accent)">Available VMs (${discovered.length})</div>`;
          html+=`<table class="proc-table"><tr><th>Name</th><th>Location</th><th>Resource Group</th><th>Status</th><th></th></tr>`;
          html+=discovered.map(vm=>{
            const name=esc(vm.name||vm.Name||'');
            const loc=esc(vm.location||vm.Location||'');
            const rg=esc(vm.resourceGroup||vm.resource_group||vm.ResourceGroup||'');
            const isConfigured=configuredNames.has(vm.name||vm.Name||'');
            return`<tr>
              <td><strong>${name}</strong></td>
              <td>${loc}</td>
              <td style="font-size:.8rem;color:#8b949e">${rg}</td>
              <td>${isConfigured?'<span class="ok">configured</span>':'<span style="color:#8b949e">available</span>'}${vm.is_local?' <span class="ok">joined</span>':''}</td>
              <td>${!isConfigured?`<button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="quickAddHost('${name}','${rg}')">+ Add</button>`:''}</td>
            </tr>`;
          }).join('');
          html+=`</table></div>`;
        }

        // Configured hosts
        if(d.hosts?.length){
          html+=`<div style="margin-top:.5rem"><div style="font-weight:600;font-size:.85rem;margin-bottom:.4rem">Configured Hosts (${d.hosts.length})</div>`;
          html+=d.hosts.map(h=>{
            const name=esc(h.name||'');
            return`<div style="display:flex;align-items:center;gap:0.5rem;padding:4px 0;border-bottom:1px solid var(--border)">
              <span style="flex:1"><strong>${name}</strong> <span style="color:#8b949e">(${esc(h.resource_group||'default')})</span> ${h.is_local?'<span class="ok">joined</span> ':''}<span style="color:#8b949e;font-size:.75rem">${timeAgo(h.added_at)}</span></span>
              <button class="btn" style="padding:2px 8px;font-size:.8rem" data-host="${name}">Remove</button>
            </div>`;
          }).join('');
          html+=`</div>`;
        }

        if(!html){html='<span style="color:#8b949e">No hosts discovered or configured. Ensure azlin is installed, or add a VM name below.</span>';}
        el.innerHTML=html;
        el.querySelectorAll('button[data-host]').forEach(btn=>{
          btn.addEventListener('click',()=>removeHost(btn.dataset.host));
        });
      }catch(e){document.getElementById('hosts-list').innerHTML='<span class="err">Failed to load hosts</span>';}
    }
    function quickAddHost(name,rg){
      apiFetch('/api/hosts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:name,resource_group:rg||'rysweet-linux-vm-pool'})})
        .then(d=>{if(d.status==='ok'){fetchHosts();fetchDistributed();}else alert(d.error||'Failed');}).catch(e=>alert('Error: '+e));
    }
    async function addHost(){
      const name=document.getElementById('host-name').value.trim();
      const rg=document.getElementById('host-rg').value.trim();
      if(!name){document.getElementById('host-status').textContent='Name required';return;}
      try{
        const d=await apiFetch('/api/hosts',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name,resource_group:rg})});
        document.getElementById('host-status').textContent=d.status==='ok'?'Added ✓':'Error: '+(d.error||'');
        document.getElementById('host-name').value='';
        fetchHosts();
        fetchDistributed();
        setTimeout(()=>document.getElementById('host-status').textContent='',3000);
      }catch(e){document.getElementById('host-status').textContent='Network error';}
    }
    async function removeHost(name){
      if(!confirm('Remove host "'+name+'"?'))return;
      await apiFetch('/api/hosts',{method:'DELETE',headers:{'Content-Type':'application/json'},body:JSON.stringify({name})});
      fetchHosts();
      fetchDistributed();
    }
    fetchHosts();

    /* --- Goals --- */
    async function fetchGoals(){
      try{
        const d=await apiFetch('/api/goals');
        if(d.active?.length){
          document.getElementById('goals-active').innerHTML=`<table class="proc-table">
            <tr><th>Priority</th><th>ID</th><th>Description</th><th>Status</th><th>Current Activity</th><th>Actions</th></tr>
            ${d.active.map(g=>{
              let wipHtml='—';
              if(g.current_activity||g.wip_refs?.length){
                let parts=[];
                if(g.current_activity) parts.push('<div style="font-size:.8rem">'+esc(g.current_activity)+'</div>');
                if(g.wip_refs?.length) parts.push(g.wip_refs.map(r=>{
                  const icon=r.kind==='pr'?'🔀':r.kind==='issue'?'🐛':r.kind==='branch'?'🌿':r.kind==='session'?'💻':'📌';
                  return r.url?'<a href="'+esc(r.url)+'" target="_blank" style="color:var(--accent);text-decoration:none;font-size:.8rem">'+icon+' '+esc(r.label)+'</a>':'<span style="font-size:.8rem">'+icon+' '+esc(r.label)+'</span>';
                }).join('<br>'));
                wipHtml=parts.join('');
              }
              return `<tr>
              <td style="text-align:center">${g.priority??'—'}</td>
              <td><code>${esc(g.id)}</code></td>
              <td>${esc(g.description)}</td>
              <td>${esc(g.status)}</td>
              <td>${wipHtml}</td>
              <td>
                <button class="btn" style="font-size:.7rem;padding:2px 6px" onclick="demoteGoal('${esc(g.id)}')">▼ Backlog</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px" onclick="updateGoalStatus('${esc(g.id)}')">Status</button>
                <button class="btn" style="font-size:.7rem;padding:2px 6px;margin-left:4px;color:#f85149" onclick="removeGoal('${esc(g.id)}')">✕</button>
              </td>
            </tr>`;}).join('')}
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
        const d=await apiFetch('/api/goals/seed',{method:'POST'});
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
        const d=await apiFetch('/api/goals',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({description:desc,type:type,priority:priority})});
        if(d.status==='ok'){document.getElementById('add-goal-form').style.display='none';document.getElementById('new-goal-desc').value='';fetchGoals();}
        else{alert(d.error||'Failed');}
      }catch(e){alert('Error: '+e);}
    }

    async function removeGoal(id){
      if(!confirm('Remove goal "'+id+'"?'))return;
      try{
        const d=await apiFetch('/api/goals/'+encodeURIComponent(id),{method:'DELETE'});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function promoteGoal(id){
      try{
        const d=await apiFetch('/api/goals/promote/'+encodeURIComponent(id),{method:'POST'});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function demoteGoal(id){
      if(!confirm('Move "'+id+'" to backlog?'))return;
      try{
        const d=await apiFetch('/api/goals/demote/'+encodeURIComponent(id),{method:'POST'});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    async function updateGoalStatus(id){
      const status=prompt('New status (not-started, in-progress, blocked, completed):');
      if(!status)return;
      try{
        const d=await apiFetch('/api/goals/'+encodeURIComponent(id)+'/status',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({status:status})});
        if(d.status==='ok')fetchGoals();
        else alert(d.error||'Failed');
      }catch(e){alert('Error: '+e);}
    }

    /* --- Traces --- */
    async function fetchTraces(){
      try{
        const d=await apiFetch('/api/traces');
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
        const d=await apiFetch('/api/memory/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query:q})});
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

    function mgApplyFilters(){
      const checks={};
      document.querySelectorAll('.mem-filter').forEach(cb=>{checks[cb.dataset.type]=cb.checked;});
      mgFiltered=mgNodes.filter(n=>{
        if(checks[n.type]===false)return false;
        const lbl=(n.label||'').toLowerCase();
        if(lbl.indexOf('goal-board:snapshot')>=0)return false;
        return true;
      });
      const ids=new Set(mgFiltered.map(n=>n.id));
      mgFilteredEdges=mgEdges.filter(e=>ids.has(e.source)&&ids.has(e.target));
      mgRender();
    }
    document.querySelectorAll('.mem-filter').forEach(cb=>cb.addEventListener('change',mgApplyFilters));

    async function fetchMemoryGraph(){
      try{
        const d=await apiFetch('/api/memory/graph');
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
        ctx.strokeStyle='rgba(88,166,255,0.35)';ctx.lineWidth=1.5;ctx.stroke();
      });
      const r=8;
      mgFiltered.forEach(n=>{
        const lblLow=(n.label||'').toLowerCase();
        const isGoal=lblLow.indexOf('goal')>=0;
        const nr=isGoal?12:r;
        ctx.beginPath();ctx.arc(n.x,n.y,n===mgPinned?nr+3:nr,0,Math.PI*2);
        ctx.fillStyle=isGoal?'#FFD700':(mgColors[n.type]||'#8b949e');
        if(n===mgPinned){ctx.lineWidth=2;ctx.strokeStyle='#fff';ctx.stroke();}
        ctx.fill();
        const lbl=n.label||'';
        if(lbl.length>0&&mgScale>0.5){
          ctx.fillStyle='#c9d1d9';ctx.font='10px sans-serif';ctx.textAlign='center';
          ctx.fillText(lbl.substring(0,30),n.x,n.y-nr-4);
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
        const d=await apiFetch('/api/costs');
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
        const d=await apiFetch('/api/budget');
        document.getElementById('budget-daily').value=d.daily_budget_usd||500;
        document.getElementById('budget-weekly').value=d.weekly_budget_usd||2500;
      }catch(e){}
    }
    async function saveBudget(){
      const daily=parseFloat(document.getElementById('budget-daily').value)||500;
      const weekly=parseFloat(document.getElementById('budget-weekly').value)||2500;
      try{
        const d=await apiFetch('/api/budget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({daily_budget_usd:daily,weekly_budget_usd:weekly})});
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
        const d=await apiFetch('/api/workboard');
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
              <span style="flex:1">${renderActionDetail(a.result)}</span>
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
        const d=await apiFetch('/api/ooda-thinking');
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
                ${rpt.outcomes.map(o=>{
                  const se=o.spawn_engineer;
                  let seBlock='';
                  if(se){
                    const statusColor=se.status==='live'?'var(--green)':se.status==='skipped'?'var(--yellow)':se.status==='denied'?'var(--yellow)':'var(--red)';
                    const agent=se.subordinate_agent;
                    const agentLink=agent?`<a href='javascript:void(0)' onclick="openAgentLog('${esc(agent)}');return false;"><code>${esc(agent)}</code></a>`:'<em>(no agent)</em>';
                    seBlock=`<div class="spawn-engineer-block" style="margin-top:.35rem;padding:.4rem .55rem;border-left:3px solid ${statusColor};background:rgba(255,255,255,0.03);border-radius:4px">
                      <div><span style="color:${statusColor}">●</span> <strong>spawn_engineer</strong> · ${esc(se.last_action||'')} · <span style="color:${statusColor}">${esc(se.status||'')}</span></div>
                      <div>subordinate: ${agentLink}${se.goal_id?` · goal <code>${esc(se.goal_id)}</code>`:''}</div>
                      ${se.task_summary?`<div>task: ${esc(se.task_summary)}</div>`:''}
                    </div>`;
                  }
                  const det=o.detail||'';
                  const detLow=det.toLowerCase();
                  const hasArtifact=detLow.indexOf('pr #')>=0||detLow.indexOf('commit')>=0;
                  const isAssessmentOnly=detLow.indexOf('assessed')>=0&&detLow.indexOf('verified=0')>=0;
                  const linkIcon=hasArtifact?'<span style="color:#2ea043;margin-right:4px" title="produced artifact">🔗</span>':'';
                  const assessBadge=(!hasArtifact&&isAssessmentOnly)?' <span class="badge-assessment" style="background:#fb8500;color:#fff;padding:1px 6px;border-radius:3px;font-size:11px;margin-left:6px">assessment only</span>':'';
                  return `<div class="outcome ${o.success?'success':'failure'}">
                    ${o.success?'✅':'❌'} <code>${esc(o.action_kind)}</code> — ${esc(o.action_description)}${assessBadge}
                    <div class="outcome-detail">${linkIcon}${esc(det.substring(0,300))}${det.length>300?'…':''}</div>
                    ${seBlock}
                  </div>`;
                }).join('')}
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

    /* --- Agent log terminal (issue #947) --- */
    let agentLogTerm = null;
    let agentLogWS = null;
    /* Issue #946: jump from a Thinking-tab spawn_engineer outcome straight to
       the agent terminal viewer. Switches tabs, populates the agent-name
       input, and clicks Connect. */
    function openAgentLog(name){
      const tab = document.querySelector('.tab[data-tab="terminal"]');
      if(tab) tab.click();
      const input = document.getElementById('agent-log-name');
      if(input) input.value = name || '';
      // initAgentLogTerminal is invoked by the tab click handler; defer
      // connect a tick so xterm has been mounted.
      setTimeout(()=>{ try{ connectAgentLog(); }catch(e){} }, 50);
    }
    function setAgentLogStatus(text, color){
      const el = document.getElementById('agent-log-status');
      if(!el) return;
      el.textContent = text;
      el.style.color = color || '#8b949e';
    }
    function initAgentLogTerminal(){
      if(agentLogTerm) return;
      if(typeof Terminal === 'undefined'){
        setAgentLogStatus('xterm.js failed to load (CDN unreachable)', '#f85149');
        return;
      }
      agentLogTerm = new Terminal({
        convertEol: true,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
        fontSize: 13,
        theme: { background: '#000000', foreground: '#c9d1d9' },
      });
      agentLogTerm.open(document.getElementById('xterm-host'));
    }
    function connectAgentLog(){
      initAgentLogTerminal();
      if(!agentLogTerm) return;
      const raw = (document.getElementById('agent-log-name').value || '').trim();
      // Client-side allow-list mirrors the server sanitizer (^[A-Za-z0-9_-]{1,64}$).
      if(!/^[A-Za-z0-9_-]{1,64}$/.test(raw)){
        setAgentLogStatus('invalid agent name (allowed: letters, digits, _ and -, up to 64 chars)', '#f85149');
        return;
      }
      if(agentLogWS){ try { agentLogWS.close(); } catch(_) {} agentLogWS = null; }
      agentLogTerm.clear();
      const proto = (window.location.protocol === 'https:') ? 'wss:' : 'ws:';
      const url = proto + '//' + window.location.host + '/ws/agent_log/' + encodeURIComponent(raw);
      setAgentLogStatus('connecting…', '#d29922');
      let ws;
      try { ws = new WebSocket(url); }
      catch(e){ setAgentLogStatus('connect failed: ' + (e && e.message || e), '#f85149'); return; }
      agentLogWS = ws;
      ws.onopen = () => setAgentLogStatus('● connected to ' + raw, '#3fb950');
      ws.onmessage = (ev) => {
        // Plain text frames; one frame per line (server already stripped \n).
        if(typeof ev.data === 'string' && agentLogTerm){ agentLogTerm.writeln(ev.data); }
      };
      ws.onerror = () => setAgentLogStatus('socket error', '#f85149');
      ws.onclose = () => { setAgentLogStatus('disconnected', '#8b949e'); if(agentLogWS === ws) agentLogWS = null; };
    }
    function disconnectAgentLog(){
      if(agentLogWS){ try { agentLogWS.close(); } catch(_) {} agentLogWS = null; }
      setAgentLogStatus('disconnected', '#8b949e');
    }

    /* --- Azlin tmux sessions panel (WS-1) --- */
    function fmtUnixTs(ts){
      if(typeof ts !== 'number' || !isFinite(ts) || ts <= 0) return '—';
      try { return new Date(ts*1000).toLocaleString(); } catch(_) { return String(ts); }
    }
    async function fetchTmuxSessions(){
      const body = document.getElementById('tmux-sessions-body');
      if(!body) return;
      try {
        const data = await apiFetch('/api/azlin/tmux-sessions');
        const hosts = Array.isArray(data.hosts) ? data.hosts : [];
        if(hosts.length === 0){
          body.innerHTML = '<div style="color:#8b949e;font-size:.85rem">No configured hosts.</div>';
        } else {
          body.innerHTML = hosts.map(h => renderTmuxHost(h)).join('');
        }
        const ts = document.getElementById('tmux-last-refreshed');
        if(ts) ts.textContent = data.refreshed_at ? new Date(data.refreshed_at).toLocaleString() : new Date().toLocaleString();
      } catch(e) {
        body.innerHTML = '<div style="color:#f85149;font-size:.85rem">Failed to load tmux sessions: '+esc(e.message||e)+'</div>';
      }
    }
    function renderTmuxHost(h){
      const host = String(h.host || '');
      const reachable = !!h.reachable;
      const sessions = Array.isArray(h.sessions) ? h.sessions : [];
      const errText = h.error ? String(h.error) : '';
      const headerColor = reachable ? '#3fb950' : '#f85149';
      const status = reachable ? '● reachable' : '○ unreachable';
      let inner;
      if(!reachable){
        inner = '<div style="color:#8b949e;font-size:.85rem;padding:.5rem">'
              + (errText ? esc(errText) : 'host unreachable')
              + '</div>';
      } else if(sessions.length === 0){
        inner = '<div style="color:#8b949e;font-size:.85rem;padding:.5rem">No tmux sessions on this host.</div>';
      } else {
        const rows = sessions.map(s => {
          const name = String(s.name || '');
          const created = fmtUnixTs(s.created);
          const attached = s.attached ? '✓' : '—';
          const wins = (s.windows == null) ? '—' : String(s.windows);
          const tid = 'tmux-open-'+host+'-'+name;
          return '<tr>'
               + '<td style="padding:.3rem .5rem;font-family:monospace">'+esc(name)+'</td>'
               + '<td style="padding:.3rem .5rem;color:#8b949e">'+esc(created)+'</td>'
               + '<td style="padding:.3rem .5rem;text-align:center">'+attached+'</td>'
               + '<td style="padding:.3rem .5rem;text-align:right">'+esc(wins)+'</td>'
               + '<td style="padding:.3rem .5rem;text-align:right">'
               +   '<button class="btn" data-testid="'+esc(tid)+'" '
               +     'onclick="openTmuxAttach('+JSON.stringify(host)+','+JSON.stringify(name)+')">Open</button>'
               + '</td>'
               + '</tr>';
        }).join('');
        inner = '<table data-testid="tmux-table-'+esc(host)+'" '
              + 'style="width:100%;border-collapse:collapse;font-size:.88rem">'
              + '<thead><tr style="border-bottom:1px solid var(--border);color:#8b949e;text-align:left">'
              + '<th style="padding:.3rem .5rem">Session</th>'
              + '<th style="padding:.3rem .5rem">Created</th>'
              + '<th style="padding:.3rem .5rem;text-align:center">Attached?</th>'
              + '<th style="padding:.3rem .5rem;text-align:right">Windows</th>'
              + '<th style="padding:.3rem .5rem;text-align:right">Action</th>'
              + '</tr></thead><tbody>'
              + rows
              + '</tbody></table>';
      }
      // For unreachable hosts, also expose the host-keyed testid on the wrapper so
      // e2e tests can find error text without a sessions table.
      const wrapperTid = reachable ? '' : ' data-testid="tmux-table-'+esc(host)+'"';
      return '<div'+wrapperTid+' style="margin-top:.6rem;border:1px solid var(--border);border-radius:6px;overflow:hidden">'
           + '<div style="background:#1a1a2e;padding:.4rem .6rem;display:flex;justify-content:space-between;align-items:center">'
           +   '<strong style="font-family:monospace">'+esc(host)+'</strong>'
           +   '<span style="color:'+headerColor+';font-size:.85rem">'+status+'</span>'
           + '</div>'
           + inner
           + '</div>';
    }
    function openTmuxAttach(host, session){
      // Validate identifier shape client-side (mirror of server allow-list).
      const re = /^[A-Za-z0-9_.-]{1,64}$/;
      if(!re.test(host) || !re.test(session)){
        setAgentLogStatus('invalid host or session name', '#f85149');
        return;
      }
      initAgentLogTerminal();
      if(!agentLogTerm) return;
      // Tear down any existing agent-log WS before reusing the xterm instance.
      if(agentLogWS){ try { agentLogWS.close(); } catch(_) {} agentLogWS = null; }
      agentLogTerm.clear();
      // Surface the attached target in the existing status row.
      const nameInput = document.getElementById('agent-log-name');
      if(nameInput) nameInput.value = host + ':' + session;
      setAgentLogStatus('attaching to '+host+':'+session+'…', '#d29922');
      const proto = (window.location.protocol === 'https:') ? 'wss:' : 'ws:';
      const url = proto + '//' + window.location.host
                + '/ws/tmux_attach/' + encodeURIComponent(host)
                + '/' + encodeURIComponent(session);
      let ws;
      try { ws = new WebSocket(url); ws.binaryType = 'arraybuffer'; }
      catch(e){ setAgentLogStatus('connect failed: '+(e&&e.message||e), '#f85149'); return; }
      agentLogWS = ws;
      ws.onopen = () => setAgentLogStatus('attached: '+host+':'+session, '#3fb950');
      ws.onmessage = (ev) => {
        if(!agentLogTerm) return;
        if(typeof ev.data === 'string'){
          agentLogTerm.write(ev.data);
        } else if(ev.data instanceof ArrayBuffer){
          const bytes = new Uint8Array(ev.data);
          // Pass raw bytes through xterm so ANSI escapes render correctly.
          let s = '';
          for(let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
          agentLogTerm.write(s);
        }
      };
      ws.onerror = () => setAgentLogStatus('socket error', '#f85149');
      ws.onclose = () => { setAgentLogStatus('detached', '#8b949e'); if(agentLogWS === ws) agentLogWS = null; };
    }

    /* --- Init --- */
    fetchStatus(); fetchIssues(); fetchDistributed(); fetchAgentOverview();
    setInterval(fetchAgentOverview,30000);
    setInterval(fetchStatus,30000);
    setInterval(fetchIssues,120000);
  </script>
</body>
</html>
"#;
