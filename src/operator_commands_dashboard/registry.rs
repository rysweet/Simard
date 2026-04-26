use axum::Json;
use serde_json::{Value, json};

use crate::agent_registry::{AgentRegistry, FileBackedAgentRegistry};
use crate::build_lock::BuildLock;
use super::memory::build_agent_graph;
use super::routes::resolve_state_root;

// ---------------------------------------------------------------------------
// Agent Registry API (#296)
// ---------------------------------------------------------------------------

pub(crate) async fn registry_list() -> Json<Value> {
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

pub(crate) async fn registry_register(Json(body): Json<Value>) -> Json<Value> {
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

pub(crate) async fn registry_deregister(Json(body): Json<Value>) -> Json<Value> {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    let reg = FileBackedAgentRegistry::new(&resolve_state_root());
    match reg.deregister(id) {
        Ok(()) => Json(json!({"ok": true})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}

pub(crate) async fn registry_reap() -> Json<Value> {
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



pub(crate) async fn agent_graph() -> Json<Value> {
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

pub(crate) async fn build_lock_status() -> Json<Value> {
    let bl = BuildLock::new(&resolve_state_root());
    Json(json!({
        "locked": bl.is_locked(),
        "holder": bl.current_holder(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

pub(crate) async fn build_lock_force_release() -> Json<Value> {
    let bl = BuildLock::new(&resolve_state_root());
    match bl.force_release() {
        Ok(was_locked) => Json(json!({"ok": true, "was_locked": was_locked})),
        Err(e) => Json(json!({"ok": false, "error": e.to_string()})),
    }
}
