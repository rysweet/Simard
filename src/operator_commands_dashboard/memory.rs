use axum::Json;
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory, as_f64, as_i64, as_str};

// ---------------------------------------------------------------------------
// Recent memories — plain-English view for #1997
// ---------------------------------------------------------------------------

/// Extract an approximate Unix timestamp (seconds) from a UUIDv7 simple-hex
/// string embedded in a LadybugDB node id like `epi_<uuid_hex>`.
fn timestamp_from_v7_id(id: &str) -> Option<f64> {
    let hex = id.split('_').nth(1)?;
    if hex.len() < 12 {
        return None;
    }
    // UUIDv7 encodes the Unix-epoch millisecond timestamp in the first 48
    // bits (12 hex characters).
    let millis = u64::from_str_radix(&hex[..12], 16).ok()?;
    Some(millis as f64 / 1000.0)
}

/// Plain-English label for an internal node type.
fn human_category(node_type: &str) -> &'static str {
    match node_type {
        "WorkingMemory" => "Current task context",
        "SemanticFact" => "Learned fact",
        "EpisodicMemory" => "Past event",
        "ProceduralMemory" => "How-to knowledge",
        "ProspectiveMemory" => "Planned reminder",
        "SensoryBuffer" => "Recent observation",
        _ => "Memory",
    }
}

/// `GET /api/memory/recent` — returns recent memories sorted by time,
/// with plain-English summaries and a count of items from the last hour.
pub(crate) async fn memory_recent() -> Json<Value> {
    let state_root = resolve_state_root();
    let mem = match NativeCognitiveMemory::open_read_only(&state_root) {
        Ok(m) => m,
        Err(e) => {
            return Json(json!({
                "items": [],
                "total": 0,
                "last_hour_count": 0,
                "error": format!("Cannot open memory store: {e}"),
            }));
        }
    };

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let one_hour_ago = now_secs - 3600.0;

    let query_rows =
        |cypher: &str| -> Vec<Vec<lbug::Value>> { mem.query(cypher).unwrap_or_default() };

    #[derive(Debug)]
    struct RecentItem {
        id: String,
        node_type: &'static str,
        summary: String,
        detail: String,
        ts: f64,
    }

    let mut items: Vec<RecentItem> = Vec::new();

    // Working memory
    for row in query_rows("MATCH (w:WorkingMemory) RETURN w.id, w.content, w.slot_type LIMIT 50") {
        if let Some(id) = row.first().and_then(as_str) {
            let content = row.get(1).and_then(as_str).unwrap_or("");
            let summary = first_sentence(content);
            items.push(RecentItem {
                ts: timestamp_from_v7_id(id).unwrap_or(0.0),
                id: id.to_string(),
                node_type: "WorkingMemory",
                summary,
                detail: content.to_string(),
            });
        }
    }

    // Semantic facts
    for row in query_rows("MATCH (f:Fact) RETURN f.id, f.concept, f.content, f.confidence LIMIT 50")
    {
        if let Some(id) = row.first().and_then(as_str) {
            let concept = row.get(1).and_then(as_str).unwrap_or("");
            let content = row.get(2).and_then(as_str).unwrap_or("");
            let summary = if concept.is_empty() {
                first_sentence(content)
            } else {
                concept.to_string()
            };
            items.push(RecentItem {
                ts: timestamp_from_v7_id(id).unwrap_or(0.0),
                id: id.to_string(),
                node_type: "SemanticFact",
                summary,
                detail: content.to_string(),
            });
        }
    }

    // Episodes
    for row in query_rows("MATCH (e:Episode) RETURN e.id, e.content, e.source_label LIMIT 50") {
        if let Some(id) = row.first().and_then(as_str) {
            let content = row.get(1).and_then(as_str).unwrap_or("");
            let summary = first_sentence(content);
            items.push(RecentItem {
                ts: timestamp_from_v7_id(id).unwrap_or(0.0),
                id: id.to_string(),
                node_type: "EpisodicMemory",
                summary,
                detail: content.to_string(),
            });
        }
    }

    // Procedures
    for row in query_rows("MATCH (p:Procedure) RETURN p.id, p.name, p.steps LIMIT 50") {
        if let Some(id) = row.first().and_then(as_str) {
            let name = row.get(1).and_then(as_str).unwrap_or("");
            let summary = if name.is_empty() {
                "(unnamed procedure)".to_string()
            } else {
                name.to_string()
            };
            items.push(RecentItem {
                ts: timestamp_from_v7_id(id).unwrap_or(0.0),
                id: id.to_string(),
                node_type: "ProceduralMemory",
                summary,
                detail: row.get(2).and_then(as_str).unwrap_or("").to_string(),
            });
        }
    }

    // Prospective memory
    for row in
        query_rows("MATCH (p:Prospective) RETURN p.id, p.description, p.trigger_condition LIMIT 50")
    {
        if let Some(id) = row.first().and_then(as_str) {
            let desc = row.get(1).and_then(as_str).unwrap_or("");
            let summary = first_sentence(desc);
            items.push(RecentItem {
                ts: timestamp_from_v7_id(id).unwrap_or(0.0),
                id: id.to_string(),
                node_type: "ProspectiveMemory",
                summary,
                detail: desc.to_string(),
            });
        }
    }

    // Sensory
    for row in query_rows("MATCH (s:Sensory) RETURN s.id, s.modality, s.raw_data LIMIT 50") {
        if let Some(id) = row.first().and_then(as_str) {
            let modality = row.get(1).and_then(as_str).unwrap_or("");
            let raw = row.get(2).and_then(as_str).unwrap_or("");
            let summary = if raw.is_empty() {
                format!("({modality} observation)")
            } else {
                first_sentence(raw)
            };
            items.push(RecentItem {
                ts: timestamp_from_v7_id(id).unwrap_or(0.0),
                id: id.to_string(),
                node_type: "SensoryBuffer",
                summary,
                detail: raw.to_string(),
            });
        }
    }

    // Sort newest first
    items.sort_by(|a, b| b.ts.partial_cmp(&a.ts).unwrap_or(std::cmp::Ordering::Equal));

    let total = items.len();
    let last_hour_count = items.iter().filter(|i| i.ts >= one_hour_ago).count();

    let json_items: Vec<Value> = items
        .iter()
        .take(100)
        .map(|item| {
            let ts_str = if item.ts > 0.0 {
                chrono::DateTime::from_timestamp(item.ts as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            json!({
                "id": item.id,
                "category": human_category(item.node_type),
                "summary": item.summary,
                "detail": item.detail,
                "timestamp": ts_str,
                "epoch_secs": item.ts,
            })
        })
        .collect();

    Json(json!({
        "items": json_items,
        "total": total,
        "last_hour_count": last_hour_count,
        "server_time": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Extract the first sentence (or first ~120 chars) as a summary.
fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "(empty)".to_string();
    }
    // If it starts with '[' it might be a consolidation tag — skip it
    let cleaned = if trimmed.starts_with('[') {
        trimmed
            .find(']')
            .map(|i| &trimmed[i + 1..])
            .unwrap_or(trimmed)
            .trim()
    } else {
        trimmed
    };
    if cleaned.is_empty() {
        return trimmed.chars().take(120).collect();
    }
    // Find first sentence boundary
    for (i, c) in cleaned.char_indices() {
        if i > 10 && (c == '.' || c == '!' || c == '?') {
            let sentence: String = cleaned[..=i].to_string();
            if sentence.len() <= 200 {
                return sentence;
            }
        }
        if i >= 120 {
            return format!("{}…", &cleaned[..i]);
        }
    }
    cleaned.to_string()
}

pub(crate) async fn memory_search(Json(body): Json<Value>) -> Json<Value> {
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

    // Search the cognitive-memory goal-board snapshot too (issue #1590 —
    // goal data no longer lives on disk).
    if let Ok(board) = super::dashboard_goal_board_snapshot(&state_root) {
        let needle = query.to_lowercase();
        let active_matches: Vec<&crate::goal_curation::ActiveGoal> = board
            .active
            .iter()
            .filter(|g| {
                g.id.to_lowercase().contains(&needle)
                    || g.description.to_lowercase().contains(&needle)
            })
            .take(5)
            .collect();
        for goal in active_matches {
            results.push(json!({
                "source": "active_goal",
                "data": {
                    "id": goal.id,
                    "description": goal.description,
                    "priority": goal.priority,
                    "status": goal.status.to_string(),
                    "assigned_to": goal.assigned_to,
                },
            }));
        }
        let backlog_matches: Vec<&crate::goal_curation::BacklogItem> = board
            .backlog
            .iter()
            .filter(|b| {
                b.id.to_lowercase().contains(&needle)
                    || b.description.to_lowercase().contains(&needle)
            })
            .take(5)
            .collect();
        for item in backlog_matches {
            results.push(json!({
                "source": "backlog_goal",
                "data": {
                    "id": item.id,
                    "description": item.description,
                    "source": item.source,
                    "score": item.score,
                },
            }));
        }
    }

    Json(json!({
        "query": query,
        "result_count": results.len(),
        "results": results,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

pub(crate) async fn memory_graph() -> Json<Value> {
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

    let query_rows =
        |cypher: &str| -> Vec<Vec<lbug::Value>> { mem.query(cypher).unwrap_or_default() };

    for row in query_rows(
        "MATCH (w:WorkingMemory) RETURN w.id, w.slot_type, w.content, w.task_id, w.relevance LIMIT 100",
    ) {
        if let Some(id) = row.first().and_then(as_str) {
            let content = row.get(2).and_then(as_str).unwrap_or("");
            let label = if content.len() > 60 {
                format!("{}…", &content[..60])
            } else {
                content.to_string()
            };
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
                if content.len() > 60 {
                    format!("{}…", &content[..60])
                } else {
                    content.to_string()
                }
            } else {
                concept.to_string()
            };
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
            let label = if content.len() > 60 {
                format!("{}…", &content[..60])
            } else {
                content.to_string()
            };
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
    let working_nodes: Vec<(String, String)> = nodes
        .iter()
        .filter(|n| n["type"] == "WorkingMemory")
        .filter_map(|n| {
            let id = n["id"].as_str()?.to_string();
            let tid = n["task_id"].as_str()?.to_string();
            if tid.is_empty() {
                None
            } else {
                Some((id, tid))
            }
        })
        .collect();
    for wn in &working_nodes {
        for other in &nodes {
            if other["type"] == "WorkingMemory" {
                continue;
            }
            if let Some(oid) = other["id"].as_str()
                && let Some(src) = other["source_id"].as_str()
                && !src.is_empty()
                && src == wn.1
            {
                edges.push(json!({"source": wn.0, "target": oid, "type": "REFERENCES"}));
            }
        }
    }

    // Link episodes with sequential temporal indices
    let mut episode_ids: Vec<(String, i64)> = nodes
        .iter()
        .filter(|n| n["type"] == "EpisodicMemory")
        .filter_map(|n| {
            Some((
                n["id"].as_str()?.to_string(),
                n["temporal_index"].as_i64().unwrap_or(0),
            ))
        })
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

/// Classify an agent role into one of three layers used by the dashboard
/// graph visualization. Returns ("ooda" | "engineer" | "session").
pub(crate) fn classify_agent_layer(role: &str) -> &'static str {
    let r = role.to_ascii_lowercase();
    if r.contains("ooda") || r.contains("operator") || r.contains("supervisor") {
        "ooda"
    } else if r.contains("engineer") || r.contains("planner") || r.contains("builder") {
        "engineer"
    } else {
        "session"
    }
}

/// Build a {nodes, edges} graph value from registry entries. Edges connect
/// every OODA node to every engineer, and every engineer to every session,
/// matching the OODA -> engineers -> sessions topology requested in #951.
pub(crate) fn build_agent_graph(entries: &[crate::agent_registry::AgentEntry]) -> Value {
    let mut nodes = Vec::with_capacity(entries.len());
    let mut ooda_ids: Vec<&str> = Vec::new();
    let mut engineer_ids: Vec<&str> = Vec::new();
    let mut session_ids: Vec<&str> = Vec::new();

    for e in entries {
        let layer = classify_agent_layer(&e.role);
        nodes.push(json!({
            "id": e.id,
            "type": layer,
            "role": e.role,
            "host": e.host,
            "pid": e.pid,
            "state": format!("{:?}", e.state),
        }));
        match layer {
            "ooda" => ooda_ids.push(&e.id),
            "engineer" => engineer_ids.push(&e.id),
            _ => session_ids.push(&e.id),
        }
    }

    let mut edges = Vec::new();
    for o in &ooda_ids {
        for eng in &engineer_ids {
            edges.push(json!({"src": o, "dst": eng}));
        }
    }
    for eng in &engineer_ids {
        for s in &session_ids {
            edges.push(json!({"src": eng, "dst": s}));
        }
    }

    json!({
        "nodes": nodes,
        "edges": edges,
        "layers": {
            "ooda": ooda_ids.len(),
            "engineer": engineer_ids.len(),
            "session": session_ids.len(),
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}
