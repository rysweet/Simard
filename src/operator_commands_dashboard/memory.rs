use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::memory_cognitive::CognitiveStatistics;
use crate::memory_ipc::open_reader_bridge;

// ---------------------------------------------------------------------------
// Memory history — per-cycle snapshots with deltas and growth rates (#2136)
// ---------------------------------------------------------------------------

/// Maximum number of snapshots to retain in the ring-buffer file.
const HISTORY_MAX_SNAPSHOTS: usize = 500;
/// Minimum seconds between auto-recorded snapshots (5 minutes).
const SNAPSHOT_MIN_INTERVAL_SECS: i64 = 300;

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
    // `open_reader_bridge` so the daemon's IPC writer serves embedded reads.
    let stats_result =
        open_reader_bridge(&state_root).and_then(|reader| reader.ops().get_statistics());

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

/// `GET /api/memory/recent` — recent-memory listing.
///
/// De-fork Phase 2b (issue #2307): this panel previously enumerated every
/// node type with raw Cypher against the deleted native LadybugDB schema. The
/// library backend exposes no equivalent "list all nodes by type" API through
/// `CognitiveMemoryOps`, so the per-item listing is reported as unavailable
/// rather than reading the abandoned native store. Aggregate counts remain
/// available via `GET /api/memory/history`.
pub(crate) async fn memory_recent() -> Json<Value> {
    Json(json!({
        "items": [],
        "total": 0,
        "last_hour_count": 0,
        "available": false,
        "note": "Per-item recent-memory listing is unavailable on the library \
                 backend (de-fork Phase 2b, #2307). See /api/memory/history for counts.",
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

/// `GET /api/memory/graph` — memory graph visualization.
///
/// De-fork Phase 2b (issue #2307): the node/edge graph was built from raw
/// Cypher against the deleted native schema. Aggregate statistics are still
/// available through the trait (routed via `open_reader_bridge`), so those are
/// surfaced; the per-node graph is reported as unavailable rather than reading
/// the abandoned native store.
pub(crate) async fn memory_graph() -> Json<Value> {
    let state_root = resolve_state_root();
    let stats = open_reader_bridge(&state_root)
        .and_then(|reader| reader.ops().get_statistics())
        .unwrap_or_default();
    Json(json!({
        "nodes": [],
        "edges": [],
        "available": false,
        "note": "Memory graph visualization is unavailable on the library \
                 backend (de-fork Phase 2b, #2307); aggregate stats are shown.",
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
