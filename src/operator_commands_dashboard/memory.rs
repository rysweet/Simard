use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::memory_cognitive::CognitiveStatistics;
use crate::memory_ipc::open_reader_client;

// ---------------------------------------------------------------------------
// Memory history — per-cycle snapshots with deltas and growth rates (#2136)
// ---------------------------------------------------------------------------

/// Maximum number of snapshots to retain in the ring-buffer file.
const HISTORY_MAX_SNAPSHOTS: usize = 500;
/// Minimum seconds between auto-recorded snapshots (5 minutes).
const SNAPSHOT_MIN_INTERVAL_SECS: i64 = 300;
/// Trailing window, in seconds, for the "remembered in the last hour" metric
/// (#2679). A snapshot whose `epoch_secs` is at-or-before `now - this` is the
/// one-hour-ago baseline the live long-term total is diffed against.
const TRAILING_WINDOW_SECS: f64 = 3600.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct MemorySnapshot {
    pub timestamp: String,
    pub epoch_secs: f64,
    pub sensory: u64,
    pub working: u64,
    pub episodic: u64,
    pub semantic: u64,
    pub procedural: u64,
    pub prospective: u64,
    pub total: u64,
    pub long_term_total: u64,
}

impl MemorySnapshot {
    fn from_stats(stats: &CognitiveStatistics) -> Self {
        let now = chrono::Utc::now();
        let total = stats.total();
        let long_term = stats.episodic_count
            + stats.semantic_count
            + stats.procedural_count
            + stats.prospective_count;
        Self {
            timestamp: now.to_rfc3339(),
            epoch_secs: now.timestamp() as f64,
            sensory: stats.sensory_count,
            working: stats.working_count,
            episodic: stats.episodic_count,
            semantic: stats.semantic_count,
            procedural: stats.procedural_count,
            prospective: stats.prospective_count,
            total,
            long_term_total: long_term,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct MemoryDeltas {
    pub sensory: i64,
    pub working: i64,
    pub episodic: i64,
    pub semantic: i64,
    pub procedural: i64,
    pub prospective: i64,
    pub total: i64,
    pub long_term_total: i64,
    pub interval_secs: f64,
}

pub(crate) fn compute_deltas(older: &MemorySnapshot, newer: &MemorySnapshot) -> MemoryDeltas {
    MemoryDeltas {
        sensory: newer.sensory as i64 - older.sensory as i64,
        working: newer.working as i64 - older.working as i64,
        episodic: newer.episodic as i64 - older.episodic as i64,
        semantic: newer.semantic as i64 - older.semantic as i64,
        procedural: newer.procedural as i64 - older.procedural as i64,
        prospective: newer.prospective as i64 - older.prospective as i64,
        total: newer.total as i64 - older.total as i64,
        long_term_total: newer.long_term_total as i64 - older.long_term_total as i64,
        interval_secs: newer.epoch_secs - older.epoch_secs,
    }
}

/// Determine trend direction from long-term total delta.
pub(crate) fn trend_label(deltas: &MemoryDeltas) -> &'static str {
    if deltas.long_term_total > 0 {
        "growing"
    } else if deltas.long_term_total < 0 {
        "shrinking"
    } else {
        "stable"
    }
}

/// Compute a per-hour growth rate from a slice of snapshots using the oldest
/// and newest entries within the window.
pub(crate) fn rate_per_hour(snapshots: &[MemorySnapshot]) -> Value {
    if snapshots.len() < 2 {
        return json!({
            "total": 0.0,
            "long_term_total": 0.0,
            "episodic": 0.0,
            "semantic": 0.0,
            "procedural": 0.0,
            "prospective": 0.0,
        });
    }
    let oldest = &snapshots[0];
    let newest = &snapshots[snapshots.len() - 1];
    let elapsed_hours = (newest.epoch_secs - oldest.epoch_secs) / 3600.0;
    if elapsed_hours < 0.001 {
        return json!({
            "total": 0.0,
            "long_term_total": 0.0,
            "episodic": 0.0,
            "semantic": 0.0,
            "procedural": 0.0,
            "prospective": 0.0,
        });
    }
    let rate = |newer: u64, older: u64| -> f64 { (newer as f64 - older as f64) / elapsed_hours };
    json!({
        "total": rate(newest.total, oldest.total),
        "long_term_total": rate(newest.long_term_total, oldest.long_term_total),
        "episodic": rate(newest.episodic, oldest.episodic),
        "semantic": rate(newest.semantic, oldest.semantic),
        "procedural": rate(newest.procedural, oldest.procedural),
        "prospective": rate(newest.prospective, oldest.prospective),
    })
}

/// Load snapshot history from disk, returning empty vec on any error.
pub(crate) fn load_history(path: &std::path::Path) -> Vec<MemorySnapshot> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<MemorySnapshot>>(&s).ok())
        .unwrap_or_default()
}

/// Persist snapshot history to disk using atomic write (write-rename).
pub(crate) fn save_history(path: &std::path::Path, history: &[MemorySnapshot]) {
    let tmp = path.with_extension("json.tmp");
    if let Ok(data) = serde_json::to_string(history)
        && std::fs::write(&tmp, &data).is_ok()
    {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Append a snapshot to the history if enough time has elapsed since the last
/// recorded entry. Returns the (possibly updated) history.
pub(crate) fn append_snapshot_if_due(
    path: &std::path::Path,
    stats: &CognitiveStatistics,
) -> Vec<MemorySnapshot> {
    let mut history = load_history(path);
    let now_secs = chrono::Utc::now().timestamp() as f64;

    let should_append = history
        .last()
        .map(|last| (now_secs - last.epoch_secs) >= SNAPSHOT_MIN_INTERVAL_SECS as f64)
        .unwrap_or(true);

    if should_append {
        history.push(MemorySnapshot::from_stats(stats));
        // Trim ring buffer
        if history.len() > HISTORY_MAX_SNAPSHOTS {
            let excess = history.len() - HISTORY_MAX_SNAPSHOTS;
            history.drain(..excess);
        }
        save_history(path, &history);
    }

    history
}

/// `GET /api/memory/history` — returns historical snapshots, deltas, growth
/// rate, and trend for the cognitive memory system (#2136).
pub(crate) async fn memory_history() -> Json<Value> {
    let state_root = resolve_state_root();
    let history_path = state_root.join("memory_history.json");

    // Get current stats via the library backend, routed through
    // `open_reader_client` so the daemon's IPC writer serves embedded reads.
    let stats_result =
        open_reader_client(&state_root).and_then(|reader| reader.ops().get_statistics());

    let stats = match stats_result {
        Ok(s) => s,
        Err(e) => {
            return Json(json!({
                "snapshots": [],
                "deltas": null,
                "rate_per_hour": null,
                "trend": "unknown",
                "error": format!("Cannot read cognitive memory: {e}"),
            }));
        }
    };

    let history = append_snapshot_if_due(&history_path, &stats);

    let deltas = if history.len() >= 2 {
        let prev = &history[history.len() - 2];
        let curr = &history[history.len() - 1];
        Some(compute_deltas(prev, curr))
    } else {
        None
    };

    let trend = deltas.as_ref().map(|d| trend_label(d)).unwrap_or("unknown");

    let rate = rate_per_hour(&history);

    Json(json!({
        "schema_version": 1,
        "snapshots": history,
        "deltas": deltas,
        "rate_per_hour": rate,
        "trend": trend,
        "snapshot_count": history.len(),
        "sample_interval_seconds": SNAPSHOT_MIN_INTERVAL_SECS,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// Recent memories — plain-English view for #1997
// ---------------------------------------------------------------------------

/// Select the long-term total of the snapshot that best represents "one hour
/// ago" for the trailing-hour delta (#2679).
///
/// Rule (pins the window edge against off-by-one / cause d):
///   1. Prefer the **most-recent** snapshot whose `epoch_secs` is
///      **at-or-before** the window edge `now_secs - TRAILING_WINDOW_SECS`
///      (`<= cutoff`), i.e. the tightest available "≥1 h old" reference.
///   2. If every snapshot is *inside* the hour (sub-hour uptime), fall back to
///      the **earliest** snapshot — an honest partial-window under-count.
///   3. Empty history has no baseline (`None`); the caller then diffs the live
///      total against itself, yielding an honest `0`.
pub(crate) fn select_last_hour_baseline(history: &[MemorySnapshot], now_secs: f64) -> Option<u64> {
    if history.is_empty() {
        return None;
    }
    let cutoff = now_secs - TRAILING_WINDOW_SECS;
    let cmp_epoch = |a: &&MemorySnapshot, b: &&MemorySnapshot| {
        a.epoch_secs
            .partial_cmp(&b.epoch_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    history
        .iter()
        .filter(|s| s.epoch_secs <= cutoff)
        .max_by(cmp_epoch)
        .or_else(|| history.iter().min_by(cmp_epoch))
        .map(|s| s.long_term_total)
}

/// `GET /api/memory/recent` — recent-memory listing.
///
/// De-fork Phase 2b (issue #2307): this panel previously enumerated every
/// node type with raw Cypher against the deleted native LadybugDB schema. The
/// library backend exposes no equivalent "list all nodes by type" API through
/// `CognitiveMemoryOps`, so the per-item listing is reported as unavailable
/// rather than reading the abandoned native store.
///
/// The *aggregate* stored total, however, is available via the same
/// `get_statistics()` path that `/api/memory/history` uses. We surface it as
/// `total` so the Memory tab can stop telling a human "No memories stored yet"
/// while tens of thousands of memories are actually held (#2358). The per-item
/// list stays empty/unavailable on this backend.
///
/// `last_hour_count` (#2679): this field previously returned a hardcoded literal
/// `0` — a placeholder left by de-fork Phase 2b (#2307) — so the dashboard told
/// operators "remembered 0 items in the last hour" even while long-term memory
/// was actively growing. It now reports the LIVE net growth of long-term memory
/// over the trailing hour, computed by [`memory_recent_at`].
pub(crate) async fn memory_recent() -> Json<Value> {
    memory_recent_at(&resolve_state_root()).await
}

/// Env-free core of [`memory_recent`]: build the recent-memory payload from the
/// EXPLICIT `state_root` rather than resolving `SIMARD_STATE_ROOT` ambiently
/// (mirrors the [`goals`](super::goals::goals) → `goals_at` split), so the
/// trailing-hour delta can be driven deterministically in tests.
///
/// `last_hour_count` is `max(0, live_long_term_total − baseline_long_term_total)`
/// where the live long-term total (episodic + semantic + procedural +
/// prospective) is read through the single shared reader
/// (`open_reader_client` → `get_statistics`) and the baseline is the
/// most-recent `memory_history.json` snapshot at-or-before `now − 1h`
/// (see [`select_last_hour_baseline`]). The read fails closed: on a live-read
/// error it returns an `error` payload with `last_hour_count: null` — never a
/// misleading `0`.
pub(crate) async fn memory_recent_at(state_root: &std::path::Path) -> Json<Value> {
    // Preserved back-compat note describing the per-item listing limitation.
    let note = "Per-item recent-memory listing is unavailable on the library \
                backend (de-fork Phase 2b, #2307); `total` is the live aggregate \
                stored count. See /api/memory/history for the per-type breakdown.";

    // Live read via the SAME shared reader path `/api/memory/history` uses so
    // the count reflects real writes, not a divergent store.
    let stats =
        match open_reader_client(state_root).and_then(|reader| reader.ops().get_statistics()) {
            Ok(s) => s,
            Err(e) => {
                // Fail closed (#2561 prior art): surface the error and emit a null
                // count so the frontend renders "—", never a misleading 0.
                return Json(json!({
                    "items": [],
                    "total": Value::Null,
                    "last_hour_count": Value::Null,
                    "available": false,
                    "note": note,
                    "error": format!("Cannot read cognitive memory: {e}"),
                    "server_time": chrono::Utc::now().to_rfc3339(),
                }));
            }
        };

    let total = stats.total();
    // "Remembered" ⇒ consolidated long-term memory (facts + procedures +
    // records/events + intentions); excludes transient sensory/working churn.
    let live_long_term = stats.episodic_count
        + stats.semantic_count
        + stats.procedural_count
        + stats.prospective_count;

    // Accumulate the trailing-hour baseline in the shared history ring buffer so
    // the metric self-heals across polls (the same file `/api/memory/history`
    // maintains).
    let history_path = state_root.join("memory_history.json");
    let history = append_snapshot_if_due(&history_path, &stats);

    let now_secs = chrono::Utc::now().timestamp() as f64;
    // Absent a one-hour-ago baseline (empty history), diff the live total
    // against itself → honest 0. `saturating_sub` clamps a pruning-dominated
    // (net-negative) interval to 0 without underflowing.
    let baseline = select_last_hour_baseline(&history, now_secs).unwrap_or(live_long_term);
    let last_hour_count = live_long_term.saturating_sub(baseline);

    Json(json!({
        "items": [],
        "total": total,
        "last_hour_count": last_hour_count,
        "available": false,
        "note": note,
        "server_time": chrono::Utc::now().to_rfc3339(),
    }))
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

/// Per-type cap on live item nodes emitted into the memory graph. Bounds the
/// response so a large store cannot stream tens of thousands of nodes into the
/// visualization — an uncapped per-type dump is a payload-bloat / DoS vector on
/// a mature store (issue #2627).
const GRAPH_MAX_PER_TYPE: usize = 200;

/// Maximum bytes of per-node `content` surfaced in the graph. Agent memory
/// content can be multi-KB; the visualization only needs a preview for the
/// hover tooltip / detail panel, so content is truncated server-side rather
/// than streaming a full memory dump per node.
const GRAPH_NODE_CONTENT_MAX: usize = 2048;

/// The six cognitive memory types, in a fixed order, each paired with the
/// human-facing label the dashboard legend / stat line uses. Every type becomes
/// an always-present "type hub" node so the graph renders legend-complete (all
/// six filters map to something) even on an empty store, and live per-item nodes
/// attach to their hub. The literals must stay in lockstep with `mgColors` in
/// `index_html/part_03.rs` — a `type` outside that set renders uncolored and
/// unfilterable.
const MEMORY_TYPE_HUBS: [(&str, &str); 6] = [
    ("WorkingMemory", "Thinking"),
    ("SensoryBuffer", "Observed"),
    ("SemanticFact", "Facts"),
    ("EpisodicMemory", "Events"),
    ("ProceduralMemory", "Procedures"),
    ("ProspectiveMemory", "Planned"),
];

/// Truncate `content` to at most `GRAPH_NODE_CONTENT_MAX` bytes on a UTF-8 char
/// boundary, appending an ellipsis when it was actually shortened.
fn truncate_graph_content(content: &str) -> String {
    if content.len() <= GRAPH_NODE_CONTENT_MAX {
        return content.to_string();
    }
    let mut end = GRAPH_NODE_CONTENT_MAX;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &content[..end])
}

/// Short single-line label for a node (truncated on a char boundary). Keeps the
/// on-canvas label readable without shipping the full content twice.
fn graph_label(raw: &str) -> String {
    const MAX: usize = 80;
    let trimmed = raw.trim();
    if trimmed.len() <= MAX {
        return trimmed.to_string();
    }
    let mut end = MAX;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &trimmed[..end])
}

/// Namespace a raw store id under a per-type prefix so item ids never collide
/// with a type-hub id (`hub:<Type>`) or across memory types, and every edge
/// endpoint resolves to exactly one emitted node.
fn graph_node_id(prefix: &str, raw: &str) -> String {
    format!("{prefix}:{raw}")
}

/// Build the LIVE memory-graph payload `{nodes, edges, available, stats}` from a
/// cognitive-memory reader.
///
/// Topology (type-clustered): six always-present type-hub nodes (one per
/// cognitive memory type) plus live per-item nodes for the four trait-enumerable
/// types — semantic facts (`search_facts` wildcard), episodes
/// (`list_all_episodes`), procedures (`recall_procedure` wildcard), and
/// prospective memories (`list_all_prospective`) — each linked back to its hub.
/// Working and sensory memory expose no trait-level per-item enumerator, so they
/// render as their hub alone. `stats` mirrors `get_statistics()`. Payload is
/// bounded by [`GRAPH_MAX_PER_TYPE`] (item nodes per type) and
/// [`GRAPH_NODE_CONTENT_MAX`] (per-node content bytes).
pub(crate) fn build_live_memory_graph(
    ops: &dyn crate::cognitive_memory::CognitiveMemoryOps,
) -> Value {
    let cap = GRAPH_MAX_PER_TYPE as u32;
    let stats = ops.get_statistics().unwrap_or_default();

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();

    // Six type hubs — always present so the legend + all six filters are
    // meaningful even when a type currently holds no items.
    for (ty, label) in MEMORY_TYPE_HUBS {
        nodes.push(json!({
            "id": format!("hub:{ty}"),
            "type": ty,
            "label": label,
            "hub": true,
            "content": format!("{label}: cognitive memory type hub"),
        }));
    }

    // Semantic facts — wildcard enumeration (confidence-desc), capped.
    if let Ok(facts) = ops.search_facts("*", cap, 0.0) {
        for f in facts {
            let id = graph_node_id("fact", &f.node_id);
            nodes.push(json!({
                "id": id.clone(),
                "type": "SemanticFact",
                "label": graph_label(&f.concept),
                "content": truncate_graph_content(&f.content),
                "confidence": f.confidence,
                "tags": f.tags,
            }));
            edges.push(json!({"source": id, "target": "hub:SemanticFact"}));
        }
    }

    // Episodes — unfiltered enumeration, newest-first, capped.
    if let Ok(episodes) = ops.list_all_episodes(cap) {
        for e in episodes {
            let id = graph_node_id("episode", &e.node_id);
            nodes.push(json!({
                "id": id.clone(),
                "type": "EpisodicMemory",
                "label": graph_label(&e.content),
                "content": truncate_graph_content(&e.content),
                "source": e.source_label,
            }));
            edges.push(json!({"source": id, "target": "hub:EpisodicMemory"}));
        }
    }

    // Procedures — wildcard enumeration (usage-desc), capped.
    if let Ok(procedures) = ops.recall_procedure("*", cap) {
        for p in procedures {
            let id = graph_node_id("procedure", &p.node_id);
            let body = truncate_graph_content(&p.steps.join("\n"));
            nodes.push(json!({
                "id": id.clone(),
                "type": "ProceduralMemory",
                "label": graph_label(&p.name),
                "content": body,
                "steps": p.steps.len(),
                "usage_count": p.usage_count,
            }));
            edges.push(json!({"source": id, "target": "hub:ProceduralMemory"}));
        }
    }

    // Prospective memories — every status, priority-ordered, capped.
    if let Ok(prospective) = ops.list_all_prospective(cap) {
        for pr in prospective {
            let id = graph_node_id("prospective", &pr.node_id);
            let body = truncate_graph_content(&format!(
                "when: {}\n→ {}",
                pr.trigger_condition, pr.action_on_trigger
            ));
            nodes.push(json!({
                "id": id.clone(),
                "type": "ProspectiveMemory",
                "label": graph_label(&pr.description),
                "content": body,
                "status": pr.status,
                "priority": pr.priority,
            }));
            edges.push(json!({"source": id, "target": "hub:ProspectiveMemory"}));
        }
    }

    json!({
        "nodes": nodes,
        "edges": edges,
        "available": true,
        "stats": {
            "working": stats.working_count,
            "semantic": stats.semantic_count,
            "episodic": stats.episodic_count,
            "procedural": stats.procedural_count,
            "prospective": stats.prospective_count,
            "sensory": stats.sensory_count,
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

/// `GET /api/memory/graph` — LIVE cognitive-memory graph visualization
/// (issue #2627).
///
/// Restores the dedicated **Memory** tab's node/edge graph after the ~17→9 tab
/// consolidation left it a de-fork #2307 stub. Reads the live cognitive store
/// via a single shared reader ([`open_reader_client`]) and renders it through
/// [`build_live_memory_graph`]: six type hubs, live per-item nodes for the four
/// enumerable memory types linked to their hubs, and `stats` mirroring
/// `get_statistics()`. When the reader is unreachable the graph falls back to an
/// empty, `available:false` payload with a path-free note rather than erroring.
pub(crate) async fn memory_graph() -> Json<Value> {
    let state_root = resolve_state_root();
    match open_reader_client(&state_root) {
        Ok(reader) => Json(build_live_memory_graph(reader.ops())),
        Err(e) => {
            tracing::warn!(
                target: "simard::dashboard",
                error = %e,
                "memory_graph: cognitive reader unavailable; serving empty graph"
            );
            Json(json!({
                "nodes": [],
                "edges": [],
                "available": false,
                "note": "Cognitive memory reader is currently unavailable; \
                         showing an empty graph.",
                "stats": {
                    "working": 0,
                    "semantic": 0,
                    "episodic": 0,
                    "procedural": 0,
                    "prospective": 0,
                    "sensory": 0,
                },
            }))
        }
    }
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

#[cfg(test)]
mod tests_memory_history {
    use super::*;
    use crate::memory_cognitive::CognitiveStatistics;

    fn sample_stats(seed: u64) -> CognitiveStatistics {
        CognitiveStatistics {
            sensory_count: seed,
            working_count: seed + 1,
            episodic_count: seed * 2,
            semantic_count: seed * 3,
            procedural_count: seed,
            prospective_count: seed / 2,
        }
    }

    #[test]
    fn snapshot_from_stats_computes_totals() {
        let stats = sample_stats(10);
        let snap = MemorySnapshot::from_stats(&stats);
        assert_eq!(snap.total, stats.total());
        assert_eq!(
            snap.long_term_total,
            stats.episodic_count
                + stats.semantic_count
                + stats.procedural_count
                + stats.prospective_count
        );
        assert!(snap.epoch_secs > 0.0);
        assert!(!snap.timestamp.is_empty());
    }

    #[test]
    fn compute_deltas_correct() {
        let s1 = MemorySnapshot {
            timestamp: "2024-01-01T00:00:00Z".into(),
            epoch_secs: 1000.0,
            sensory: 5,
            working: 3,
            episodic: 10,
            semantic: 20,
            procedural: 5,
            prospective: 2,
            total: 45,
            long_term_total: 37,
        };
        let s2 = MemorySnapshot {
            timestamp: "2024-01-01T00:05:00Z".into(),
            epoch_secs: 1300.0,
            sensory: 6,
            working: 4,
            episodic: 12,
            semantic: 25,
            procedural: 5,
            prospective: 3,
            total: 55,
            long_term_total: 45,
        };
        let d = compute_deltas(&s1, &s2);
        assert_eq!(d.sensory, 1);
        assert_eq!(d.working, 1);
        assert_eq!(d.episodic, 2);
        assert_eq!(d.semantic, 5);
        assert_eq!(d.procedural, 0);
        assert_eq!(d.prospective, 1);
        assert_eq!(d.total, 10);
        assert_eq!(d.long_term_total, 8);
        assert!((d.interval_secs - 300.0).abs() < 0.01);
    }

    #[test]
    fn trend_label_directions() {
        let growing = MemoryDeltas {
            long_term_total: 5,
            ..Default::default()
        };
        assert_eq!(trend_label(&growing), "growing");
        let shrinking = MemoryDeltas {
            long_term_total: -3,
            ..Default::default()
        };
        assert_eq!(trend_label(&shrinking), "shrinking");
        let stable = MemoryDeltas {
            long_term_total: 0,
            ..Default::default()
        };
        assert_eq!(trend_label(&stable), "stable");
    }

    #[test]
    fn rate_per_hour_two_snapshots() {
        let snaps = vec![
            MemorySnapshot {
                timestamp: "".into(),
                epoch_secs: 0.0,
                sensory: 0,
                working: 0,
                episodic: 10,
                semantic: 20,
                procedural: 5,
                prospective: 2,
                total: 37,
                long_term_total: 37,
            },
            MemorySnapshot {
                timestamp: "".into(),
                epoch_secs: 3600.0,
                sensory: 0,
                working: 0,
                episodic: 20,
                semantic: 30,
                procedural: 5,
                prospective: 2,
                total: 57,
                long_term_total: 57,
            },
        ];
        let r = rate_per_hour(&snaps);
        assert_eq!(r["long_term_total"], 20.0);
        assert_eq!(r["episodic"], 10.0);
    }

    #[test]
    fn rate_per_hour_empty_and_single() {
        let empty: Vec<MemorySnapshot> = vec![];
        let r = rate_per_hour(&empty);
        assert_eq!(r["total"], 0.0);

        let single = vec![MemorySnapshot {
            timestamp: "".into(),
            epoch_secs: 100.0,
            sensory: 1,
            working: 1,
            episodic: 1,
            semantic: 1,
            procedural: 1,
            prospective: 1,
            total: 6,
            long_term_total: 4,
        }];
        let r = rate_per_hour(&single);
        assert_eq!(r["total"], 0.0);
    }

    #[test]
    fn load_save_history_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory_history.json");

        let snaps = vec![MemorySnapshot {
            timestamp: "2024-01-01T00:00:00Z".into(),
            epoch_secs: 1000.0,
            sensory: 5,
            working: 3,
            episodic: 10,
            semantic: 20,
            procedural: 5,
            prospective: 2,
            total: 45,
            long_term_total: 37,
        }];
        save_history(&path, &snaps);

        let loaded = load_history(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].total, 45);
        assert_eq!(loaded[0].long_term_total, 37);
    }

    #[test]
    fn load_history_handles_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let loaded = load_history(&path);
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_history_handles_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, "not valid json").unwrap();
        let loaded = load_history(&path);
        assert!(loaded.is_empty());
    }

    #[test]
    fn append_snapshot_enforces_interval_and_ring_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory_history.json");
        let stats = sample_stats(10);

        // First call always appends
        let h1 = append_snapshot_if_due(&path, &stats);
        assert_eq!(h1.len(), 1);

        // Immediate second call should NOT append (< 5 min)
        let h2 = append_snapshot_if_due(&path, &stats);
        assert_eq!(h2.len(), 1);

        // Simulate old entries that exceed the ring buffer
        let mut big_history: Vec<MemorySnapshot> = (0..510)
            .map(|i| MemorySnapshot {
                timestamp: format!("2024-01-01T00:{:02}:00Z", i % 60),
                epoch_secs: i as f64 * 400.0,
                sensory: i as u64,
                working: 0,
                episodic: 0,
                semantic: 0,
                procedural: 0,
                prospective: 0,
                total: i as u64,
                long_term_total: 0,
            })
            .collect();
        // Make the last entry old enough for a new snapshot
        if let Some(last) = big_history.last_mut() {
            last.epoch_secs = 0.0;
        }
        save_history(&path, &big_history);

        let h3 = append_snapshot_if_due(&path, &stats);
        assert!(
            h3.len() <= HISTORY_MAX_SNAPSHOTS,
            "ring buffer should cap at {HISTORY_MAX_SNAPSHOTS}"
        );
    }

    #[test]
    fn index_html_contains_growth_card() {
        let html = crate::operator_commands_dashboard::index_html::index_html_string();
        assert!(
            html.contains("mem-growth-card"),
            "Memory tab must contain growth card"
        );
        assert!(
            html.contains("mem-growth-trend"),
            "Memory tab must contain trend indicator"
        );
        assert!(
            html.contains("mem-growth-deltas"),
            "Memory tab must contain delta badges container"
        );
        assert!(
            html.contains("mem-growth-sparkline"),
            "Memory tab must contain sparkline SVG"
        );
        assert!(
            html.contains("mem-growth-rate"),
            "Memory tab must contain growth rate element"
        );
        assert!(
            html.contains("memory-growth-card"),
            "Growth card must have data-testid"
        );
        assert!(
            html.contains("fetchMemoryHistory"),
            "JS must include fetchMemoryHistory function"
        );
    }
}

/// Contract tests for the LIVE memory-graph read path behind the restored
/// dedicated **Memory** tab (issue #2627).
///
/// REGRESSION: the ~17->9 tab consolidation left `GET /api/memory/graph` a
/// de-fork #2307 stub returning `{nodes:[], edges:[], available:false}`. These
/// tests pin the rebuilt handler to the LIVE cognitive store read via
/// `open_reader_client(state_root).ops()`: six always-present type hubs, live
/// per-item nodes (facts/episodes/procedures/prospective) linked to their hubs,
/// stats mirroring `get_statistics()`, and payload guards (per-type cap +
/// per-node content truncation).
///
/// TDD note (Step 7 / red): every assertion here is expected to **FAIL**
/// against the current stub — `available` is hard-coded `false` and `nodes`
/// is empty. They pass once `memory_graph()` is rewired to the live builder.
#[cfg(test)]
mod tests_memory_graph {
    use super::*;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
    use crate::test_support::HermeticState;
    use std::collections::HashSet;
    use std::sync::Arc;

    /// The exact six node-type literals the frontend renderer keys on
    /// (`mgColors` in `index_html/part_03.rs`). A node.type outside this set
    /// renders uncolored and unfilterable.
    const NODE_TYPE_ALLOWLIST: [&str; 6] = [
        "WorkingMemory",
        "SemanticFact",
        "EpisodicMemory",
        "ProceduralMemory",
        "ProspectiveMemory",
        "SensoryBuffer",
    ];

    /// RAII in-process writer registration. The dashboard handler resolves
    /// `open_reader_client(state_root)` to this same Arc via the tier-0
    /// same-process shortcut, so seeds written here are visible LIVE to
    /// `memory_graph()`.
    struct MemGuard {
        writer: Arc<dyn CognitiveMemoryOps>,
    }
    impl MemGuard {
        fn register(state: &HermeticState) -> Self {
            let writer: Arc<dyn CognitiveMemoryOps> =
                Arc::new(LibraryCognitiveMemory::open(state.state_root()).expect("open store"));
            register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
            Self { writer }
        }
        fn ops(&self) -> &dyn CognitiveMemoryOps {
            self.writer.as_ref()
        }
    }
    impl Drop for MemGuard {
        fn drop(&mut self) {
            clear_in_process_writer();
        }
    }

    fn nodes_of(v: &Value) -> Vec<Value> {
        v["nodes"].as_array().cloned().unwrap_or_default()
    }
    fn edges_of(v: &Value) -> Vec<Value> {
        v["edges"].as_array().cloned().unwrap_or_default()
    }
    fn node_types(v: &Value) -> HashSet<String> {
        nodes_of(v)
            .iter()
            .filter_map(|n| n["type"].as_str().map(str::to_string))
            .collect()
    }
    /// Serialized JSON of the nodes array, for live-content marker checks.
    fn nodes_text(v: &Value) -> String {
        serde_json::to_string(&v["nodes"]).unwrap_or_default()
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_reports_available_against_live_reader() {
        let state = HermeticState::new();
        let _mem = MemGuard::register(&state);

        let out = memory_graph().await;
        let v = &out.0;
        assert_eq!(
            v["available"],
            serde_json::Value::Bool(true),
            "memory_graph() must report available:true when the live cognitive \
             reader is reachable — the de-fork #2307 stub hard-codes false. Got: {v}"
        );
        assert!(
            v["nodes"].is_array() && v["edges"].is_array(),
            "graph must always carry nodes[] and edges[] arrays; got {v}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_emits_all_six_type_hub_nodes() {
        // Type-clustered topology: even an empty store yields the six memory-type
        // hub nodes so the tab renders a legend-complete graph and all six filters
        // map to something (working + sensory have no per-item enumerator).
        let state = HermeticState::new();
        let _mem = MemGuard::register(&state);

        let out = memory_graph().await;
        let v = &out.0;
        let types = node_types(v);
        for t in NODE_TYPE_ALLOWLIST {
            assert!(
                types.contains(t),
                "memory graph must contain a node of type {t:?} (the type hub) so the \
                 {t} filter is meaningful even on an empty store; got types {types:?}"
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_node_types_are_all_in_renderer_allowlist() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        mem.ops()
            .store_fact("marker-concept", "a learned fact", 0.9, &[], "test")
            .unwrap();
        mem.ops()
            .store_episode("an event happened", "test", None)
            .unwrap();

        let out = memory_graph().await;
        let v = &out.0;
        let allow: HashSet<&str> = NODE_TYPE_ALLOWLIST.iter().copied().collect();
        for n in nodes_of(v) {
            let t = n["type"].as_str().unwrap_or("<missing>");
            assert!(
                allow.contains(t),
                "node.type {t:?} is not in the renderer's 6-literal allowlist \
                 {NODE_TYPE_ALLOWLIST:?}; it would render uncolored/unfilterable. Node: {n}"
            );
            assert!(
                n["id"].as_str().is_some_and(|s| !s.is_empty()),
                "every node needs a non-empty string id (edges + the detail panel key \
                 on it); node: {n}"
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_renders_live_fact_and_episode_content() {
        // "render LIVE data, no stale/placeholder": a freshly-stored fact and
        // episode must surface as live nodes carrying their actual content.
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let fact_marker = "ZZFACTMARKER2627";
        let ep_marker = "ZZEPISODEMARKER2627";
        mem.ops()
            .store_fact(
                fact_marker,
                &format!("{fact_marker} body"),
                0.95,
                &[],
                "test",
            )
            .unwrap();
        mem.ops()
            .store_episode(&format!("{ep_marker} occurred"), "test", None)
            .unwrap();

        let out = memory_graph().await;
        let v = &out.0;
        let text = nodes_text(v);
        assert!(
            text.contains(fact_marker),
            "the live semantic fact {fact_marker:?} must appear in the graph nodes \
             (LIVE data, not a placeholder). Nodes: {}",
            &v["nodes"]
        );
        assert!(
            text.contains(ep_marker),
            "the live episode {ep_marker:?} must appear in the graph nodes. Nodes: {}",
            &v["nodes"]
        );
        let types = node_types(v);
        assert!(
            types.contains("SemanticFact"),
            "expected a SemanticFact node after seeding a fact; got {types:?}"
        );
        assert!(
            types.contains("EpisodicMemory"),
            "expected an EpisodicMemory node after seeding an episode; got {types:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_renders_live_procedure_and_prospective() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let proc_marker = "ZZPROCMARKER2627";
        let prosp_marker = "ZZPROSPMARKER2627";
        mem.ops()
            .store_procedure(proc_marker, &["step one".to_string()], &[])
            .unwrap();
        mem.ops()
            .store_prospective(&format!("{prosp_marker} plan"), "when x", "do y", 5)
            .unwrap();

        let out = memory_graph().await;
        let v = &out.0;
        let text = nodes_text(v);
        assert!(
            text.contains(proc_marker),
            "the live procedure {proc_marker:?} must appear in the graph nodes; nodes: {}",
            &v["nodes"]
        );
        assert!(
            text.contains(prosp_marker),
            "the live prospective memory {prosp_marker:?} must appear in the graph \
             nodes; nodes: {}",
            &v["nodes"]
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_edges_reference_existing_nodes() {
        // Edge integrity: the renderer does nodeMap[e.source]/nodeMap[e.target];
        // a dangling endpoint throws. The server must guarantee every edge
        // endpoint is an emitted node id, and link live items to their hubs.
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        for i in 0..3 {
            mem.ops()
                .store_fact(&format!("c{i}"), &format!("fact {i}"), 0.9, &[], "test")
                .unwrap();
            mem.ops()
                .store_episode(&format!("event {i}"), "test", None)
                .unwrap();
        }

        let out = memory_graph().await;
        let v = &out.0;
        let ids: HashSet<String> = nodes_of(v)
            .iter()
            .filter_map(|n| n["id"].as_str().map(str::to_string))
            .collect();
        let edges = edges_of(v);
        for e in &edges {
            let s = e["source"].as_str().unwrap_or("<missing>");
            let t = e["target"].as_str().unwrap_or("<missing>");
            assert!(
                ids.contains(s),
                "edge.source {s:?} references a non-existent node; edge: {e}"
            );
            assert!(
                ids.contains(t),
                "edge.target {t:?} references a non-existent node; edge: {e}"
            );
        }
        assert!(
            !edges.is_empty(),
            "with live item nodes present, the graph must link them to their type \
             hubs (non-empty edges)"
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_stats_mirror_live_statistics() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        for i in 0..4 {
            mem.ops()
                .store_fact(&format!("sc{i}"), &format!("sf {i}"), 0.9, &[], "test")
                .unwrap();
        }
        for i in 0..2 {
            mem.ops()
                .store_episode(&format!("se {i}"), "test", None)
                .unwrap();
        }

        let live = mem.ops().get_statistics().expect("stats");
        let out = memory_graph().await;
        let v = &out.0;
        let s = &v["stats"];
        assert_eq!(
            s["semantic"].as_u64(),
            Some(live.semantic_count),
            "stats.semantic must mirror live get_statistics().semantic_count ({}); got {s}",
            live.semantic_count
        );
        assert_eq!(
            s["episodic"].as_u64(),
            Some(live.episodic_count),
            "stats.episodic must mirror live get_statistics().episodic_count ({}); got {s}",
            live.episodic_count
        );
        assert!(
            live.semantic_count >= 4 && live.episodic_count >= 2,
            "sanity: the seeds must have landed in the live store"
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_truncates_oversized_node_content() {
        // DoS / payload-bloat guard: agent memory content can be huge; the graph
        // must truncate per-node content server-side (~2KB per the design) rather
        // than streaming a multi-KB blob per node.
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let huge = "X".repeat(8000);
        mem.ops()
            .store_fact("huge-concept", &huge, 0.9, &[], "test")
            .unwrap();

        let out = memory_graph().await;
        let v = &out.0;
        for n in nodes_of(v) {
            if let Some(c) = n["content"].as_str() {
                assert!(
                    c.len() <= 4096,
                    "node content must be truncated server-side (~2KB), got {} chars \
                     — an untruncated memory dump bloats the response",
                    c.len()
                );
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_caps_item_nodes_per_type() {
        // Bounded enumeration: more items of a type than the per-type cap
        // (~GRAPH_MAX_PER_TYPE, design ~200) must not blow up the response. Seed
        // well past the cap and assert the EpisodicMemory item nodes stay bounded
        // (+1 slack for the type hub). An uncapped dump (230 items) violates this.
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        for i in 0..230 {
            mem.ops()
                .store_episode(&format!("capped episode {i}"), "test", None)
                .unwrap();
        }

        let out = memory_graph().await;
        let v = &out.0;
        let episodic = nodes_of(v)
            .iter()
            .filter(|n| n["type"].as_str() == Some("EpisodicMemory"))
            .count();
        assert!(
            episodic <= 201,
            "EpisodicMemory nodes ({episodic}) must be capped near GRAPH_MAX_PER_TYPE \
             (~200, +1 hub) — an uncapped per-type dump is a DoS vector on large stores"
        );
    }
}
