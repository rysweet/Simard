use axum::{
    Json, Router, middleware,
    routing::{delete, get, post, put},
};
use serde_json::{Value, json};

use super::activity::{activity, traces};
use super::agent_log::{WS_AGENT_LOG_ROUTE, ws_agent_log_handler};
use super::auth::{login, login_page, require_auth};
use super::brain_failures::brain_failures;
use super::chat::ws_chat_handler;
use super::chat_store::{chat_session_by_id, chat_sessions};
use super::creative_ideas::{creative_ideas, creative_ideas_search};
use super::current_work::current_work;
use super::distributed::{distributed, vacate_vm};
use super::feedback::{feedback_status, feedback_submit};
use super::goals::{
    add_goal, demote_goal, goals, promote_backlog_item, remove_goal, seed_goals, update_goal_status,
};
use super::hosts::{add_host, get_hosts, remove_host};
use super::journal::{journal_dates, journal_entry, journal_render, journal_search};
use super::logs::{logs, processes};
use super::memory::{memory_graph, memory_history, memory_recent, memory_search};
use super::merge_judge::merge_judge_decisions;
use super::merge_readiness::merge_readiness;
use super::metrics::{memory_metrics, ooda_thinking, recall_precision_correlation};
use super::monitoring::{costs, get_budget, metrics, set_budget};
use super::ooda_cycles::ooda_cycles;
use super::overseer::overseer;
use super::pr_readiness::pr_readiness;
use super::registry::{
    agent_graph, build_lock_force_release, build_lock_status, registry_deregister, registry_list,
    registry_reap, registry_register,
};
use super::status::status_snapshot;
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
        .route("/api/memory/recent", get(memory_recent))
        .route("/api/memory/history", get(memory_history))
        .route("/api/memory/search", post(memory_search))
        .route("/api/memory/graph", get(memory_graph))
        .route(
            "/api/cognition/recall-precision",
            get(recall_precision_correlation),
        )
        .route("/api/merge-judge", get(merge_judge_decisions))
        .route("/api/merge-readiness", get(merge_readiness))
        .route("/api/traces", get(traces))
        .route("/api/activity", get(activity))
        .route("/api/workboard", get(workboard))
        .route("/api/current-work", get(current_work))
        .route("/api/ooda-thinking", get(ooda_thinking))
        .route("/api/ooda-cycles", get(ooda_cycles))
        .route("/api/brain-failures", get(brain_failures))
        .route("/api/overseer", get(overseer))
        .route("/api/prs", get(pr_readiness))
        .route("/api/journal/dates", get(journal_dates))
        .route("/api/journal/search", post(journal_search))
        .route("/api/journal/entry/{date}", get(journal_entry))
        .route("/api/journal/render/{date}", get(journal_render))
        .route("/api/creative-ideas", get(creative_ideas))
        .route("/api/creative-ideas/search", post(creative_ideas_search))
        .route("/api/status/snapshot", get(status_snapshot))
        .route("/api/feedback", post(feedback_submit))
        .route("/api/feedback/status/{id}", get(feedback_status))
        .route("/api/subagent-sessions", get(subagent_sessions))
        .route("/api/chat/sessions", get(chat_sessions))
        .route("/api/chat/sessions/{id}", get(chat_session_by_id))
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

/// The UTC build/deploy instant baked into the binary at compile time by
/// `build.rs` (issue #2727), parsed from the `SIMARD_BUILD_TIMESTAMP` env.
/// Returns `None` on unusual toolchains where the env was not emitted, so the
/// `deployed` field degrades to being omitted (back-compatible).
pub(crate) fn deployed_timestamp_utc() -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = option_env!("SIMARD_BUILD_TIMESTAMP")?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Format a UTC instant as the header-ready deployment datetime in US Pacific
/// time (`America/Los_Angeles`), e.g. `2026-07-06 11:03 PDT`. Owns ALL timezone
/// and daylight-saving logic: the `%Z` token renders whatever the tz database
/// selects for that instant — `PST` (UTC-8) in standard time, `PDT` (UTC-7) in
/// daylight time — so the correct abbreviation and offset are chosen
/// automatically. Nothing is hardcoded to a fixed offset or literal `PST`/`PDT`.
pub(crate) fn format_deployed_pt(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.with_timezone(&chrono_tz::America::Los_Angeles)
        .format("%Y-%m-%d %H:%M %Z")
        .to_string()
}

/// The composed, header-ready deployment datetime string (issue #2727): the
/// compile-time build timestamp rendered in US Pacific time. `None` when no
/// build timestamp is baked in, so callers omit the field. This is the exact
/// value surfaced as the additive `/api/status` `deployed` field.
pub(crate) fn deployed_pt() -> Option<String> {
    deployed_timestamp_utc().map(format_deployed_pt)
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

    if let Some(mut h) = daemon_health {
        // System Status reads `daemon_health.cycle_number`. Surface the
        // persistent cumulative cycle number (issue #1680) so it agrees with
        // the Thinking tab and Recent Actions instead of the process-local
        // "#1" that daemon_health carries after a daemon restart.
        let authoritative =
            super::cycle_source::authoritative_cycle_number(&resolve_state_root(), Some(&h));
        if let Some(obj) = h.as_object_mut() {
            obj.insert("cycle_number".to_string(), json!(authoritative));
        }
        status_json["daemon_health"] = h;
    }

    // Issue #2727: additive `deployed` field — the deployment datetime in US
    // Pacific time (PST/PDT), sourced from the compile-time build timestamp.
    // Omitted (not faked/empty) when no build timestamp is baked in.
    if let Some(deployed) = deployed_pt() {
        status_json["deployed"] = json!(deployed);
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

// Vacate a remote VM: stop Simard processes and export memory snapshot.
//
// Steps:
// 1. Connect via azlin and stop simard-ooda service
// 2. Kill any remaining simard/cargo processes
// 3. Export cognitive memory snapshot (if available)
// 4. Remove from configured hosts

// Strip ANSI escape sequences (CSI, OSC, and single-char escapes) so that
// output from azlin/SSH can be reliably parsed for KEY=value markers.

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
