use axum::{
    Json, Router,
    middleware,
    routing::{delete, get, post, put},
};
use serde_json::{Value, json};

use super::activity::{activity, traces};
use super::agent_log::{WS_AGENT_LOG_ROUTE, ws_agent_log_handler};
use super::auth::{login, login_page, require_auth};
use super::chat::ws_chat_handler;
use super::current_work::current_work;
use super::distributed::{distributed, vacate_vm};
use super::goals::{
    add_goal, demote_goal, goals, promote_backlog_item, remove_goal, seed_goals, update_goal_status,
};
use super::hosts::{add_host, get_hosts, remove_host};
use super::logs::{logs, processes};
use super::memory::{memory_graph, memory_search};
use super::metrics::{memory_metrics, ooda_thinking};
use super::monitoring::{costs, get_budget, metrics, set_budget};
use super::registry::{
    agent_graph, build_lock_force_release, build_lock_status, registry_deregister, registry_list,
    registry_reap, registry_register,
};
use super::subagent::{disk_usage_pct, subagent_sessions};
use super::tmux::{azlin_tmux_sessions, ws_tmux_attach_handler};
use super::workboard::workboard;

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
    axum::response::Html(super::index_html::index_html_string())
}

pub(crate) fn resolve_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}
