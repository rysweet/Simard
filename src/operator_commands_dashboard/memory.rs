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
/// Trailing window, in seconds, over which the "mem/hr" growth rate is measured
/// (#4107). The rate is a *recent-activity* signal, so it is anchored at the
/// newest sample and looks back at most this far — 24 hours. Older snapshots in
/// the retained ring buffer (which spans weeks, including multi-day
/// daemon-down gaps) are ignored so an active hour of memory formation is not
/// diluted to ~0/hr by a multi-week denominator.
const GROWTH_RATE_WINDOW_SECS: f64 = 86_400.0;

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

/// A zero growth-rate payload — every rail reports `0.0/hr`. Returned when
/// there is no usable pair of samples inside the trailing window (fewer than
/// two snapshots, no prior sample within the window, or a degenerate zero-span
/// pair), so the served rate is an honest "insufficient recent data" rather
/// than a distorted whole-history figure.
fn zero_rate() -> Value {
    json!({
        "total": 0.0,
        "long_term_total": 0.0,
        "episodic": 0.0,
        "semantic": 0.0,
        "procedural": 0.0,
        "prospective": 0.0,
    })
}

/// Compute a per-hour memory growth rate over a bounded trailing window (#4107).
///
/// The rate is a *recent-activity* signal. It is anchored at the **newest**
/// snapshot and diffs against the **oldest snapshot still inside the trailing
/// [`GROWTH_RATE_WINDOW_SECS`] window** (`epoch_secs >= newest - window`, edge
/// inclusive). Snapshots older than the window — which in the retained
/// ring-buffer can be weeks old and separated by multi-day daemon-down gaps —
/// are ignored so they cannot dilute the denominator.
///
/// Returns [`zero_rate`] when there is no usable in-window pair:
///   * fewer than two snapshots total;
///   * only the newest sample lies inside the window (fresh daemon after a long
///     gap) — an honest "insufficient recent data" rather than a distorted
///     whole-span figure;
///   * the in-window baseline and newest share an epoch (zero elapsed span).
///
/// This mirrors the bounded-window discipline of [`select_last_hour_baseline`]
/// (#2679); the previous implementation used `snapshots[0]` (oldest *retained*)
/// and divided by the full multi-week span, so the advertised "per hour" figure
/// was meaningless.
pub(crate) fn rate_per_hour(snapshots: &[MemorySnapshot]) -> Value {
    if snapshots.len() < 2 {
        return zero_rate();
    }
    let newest = &snapshots[snapshots.len() - 1];
    let window_start = newest.epoch_secs - GROWTH_RATE_WINDOW_SECS;

    // Oldest snapshot at-or-after the window edge, excluding the newest itself.
    // The buffer is appended in time order, so the first in-window entry is the
    // oldest one inside the window.
    let baseline = snapshots[..snapshots.len() - 1]
        .iter()
        .find(|s| s.epoch_secs >= window_start);

    let baseline = match baseline {
        Some(b) => b,
        None => return zero_rate(),
    };

    let elapsed_hours = (newest.epoch_secs - baseline.epoch_secs) / 3600.0;
    if elapsed_hours < 0.001 {
        return zero_rate();
    }
    let rate = |newer: u64, older: u64| -> f64 { (newer as f64 - older as f64) / elapsed_hours };
    json!({
        "total": rate(newest.total, baseline.total),
        "long_term_total": rate(newest.long_term_total, baseline.long_term_total),
        "episodic": rate(newest.episodic, baseline.episodic),
        "semantic": rate(newest.semantic, baseline.semantic),
        "procedural": rate(newest.procedural, baseline.procedural),
        "prospective": rate(newest.prospective, baseline.prospective),
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
        "rate_window_secs": GROWTH_RATE_WINDOW_SECS,
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
/// per-item listing was then stubbed to always-empty on the library backend.
///
/// That stub was stale: the same shared reader (`open_reader_client`) that
/// backs `/api/memory/graph` DOES enumerate per-item memory through
/// `CognitiveMemoryOps::list_all_episodes` (newest-first by `temporal_index`).
/// So the Memory tab's "Recent Memories" panel (frontend #1997) — which already
/// ships a renderer expecting `items:[{category,summary,timestamp}]` — was
/// permanently empty even while the daemon logged thousands of episodes. We now
/// populate `items` from the most recent episodes so the panel answers "what
/// has Simard been remembering/observing recently?".
///
/// The *aggregate* stored total is available via the same `get_statistics()`
/// path that `/api/memory/history` uses. We surface it as `total` so the Memory
/// tab can stop telling a human "No memories stored yet" while tens of
/// thousands of memories are actually held (#2358).
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
    let note = "Recent items are the newest episodic memories (events Simard \
                recorded), newest-first; `total` is the live aggregate stored \
                count across all six memory types. See /api/memory/graph for the \
                full per-type graph and /api/memory/history for the per-type \
                growth breakdown.";

    // Open the shared reader ONCE and reuse it for both the aggregate statistics
    // (drives `last_hour_count`/`total`) and the per-item episode enumeration
    // (drives `items`) — the same reader path `/api/memory/graph` uses, so the
    // listing reflects real writes, not a divergent store.
    let reader = match open_reader_client(state_root) {
        Ok(r) => r,
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
    let ops = reader.ops();

    let stats = match ops.get_statistics() {
        Ok(s) => s,
        Err(e) => {
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

    // Per-item recent feed: the newest episodes (newest-first by temporal_index),
    // capped. Episodes are the only memory type carrying a wall-clock timestamp,
    // so they form the natural time-ordered "recent activity" stream. A read
    // failure here is best-effort (the aggregate above is still valid): report an
    // empty list with `available:false` rather than failing the whole endpoint.
    let (items, items_available) = match ops.list_all_episodes(RECENT_ITEMS_MAX as u32) {
        Ok(episodes) => (build_recent_episode_items(&episodes), true),
        Err(_) => (Vec::new(), false),
    };

    Json(json!({
        "items": items,
        "total": total,
        "last_hour_count": last_hour_count,
        "available": items_available,
        "note": note,
        "server_time": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Maximum number of recent per-item episodes returned by `/api/memory/recent`.
/// Bounds the payload; the panel is a "recent glance", not a full dump (the
/// graph tab covers exhaustive enumeration).
const RECENT_ITEMS_MAX: usize = 25;

/// Map recent episodes to the frontend "Recent Memories" item shape
/// (`{category, summary, timestamp, ...}`, part_03.rs `fetchRecentMemories`).
/// Episodes render under the "Past event" category. `summary`/content is bounded
/// by [`GRAPH_NODE_CONTENT_MAX`] so a single large episode cannot bloat the
/// payload.
///
/// `timestamp` is always `null`: the library backend's only episode writer path
/// (`LibraryCognitiveMemory::store_episode`) records episodes with
/// `temporal_index: None`, so the library assigns a monotonic ordinal
/// (1, 2, 3, …) — an ordering key, not a wall-clock instant — and does not
/// surface the episode's `created_at` on [`CognitiveEpisode`]. Emitting the
/// ordinal as an epoch would render a nonsensical 1970s date, so we omit it and
/// let the frontend drop the "time ago" label. Newest-first ordering is still
/// correct because it derives from the `temporal_index` sort, not this field.
fn build_recent_episode_items(
    episodes: &[crate::memory_cognitive::CognitiveEpisode],
) -> Vec<Value> {
    episodes
        .iter()
        .map(|e| {
            json!({
                "category": "Past event",
                "summary": truncate_graph_content(&e.content),
                "timestamp": Value::Null,
                "source": e.source_label,
                "node_id": e.node_id,
            })
        })
        .collect()
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

    let mut nodes: Vec<Value> = Vec::new();
    let mut edges: Vec<Value> = Vec::new();
    // Fail-LOUD accumulator (issue #2627): a read `Err` or a stats-vs-nodes
    // discrepancy is recorded here and surfaced as a top-level `error` rather
    // than swallowed into a phantom empty/partial graph. A genuinely empty
    // store leaves this empty and OMITS the `error` key.
    let mut errors: Vec<String> = Vec::new();

    // Six type hubs — always present so the legend + all six filters are
    // meaningful even when a type currently holds no items (and so an in-build
    // failure still renders a legend-complete graph behind the error overlay).
    for (ty, label) in MEMORY_TYPE_HUBS {
        nodes.push(json!({
            "id": format!("hub:{ty}"),
            "type": ty,
            "label": label,
            "hub": true,
            "content": format!("{label}: cognitive memory type hub"),
        }));
    }

    // Live statistics. A read failure must NOT be swallowed into an all-zeros
    // phantom (the retired `unwrap_or_default()`): record it and drop the
    // discrepancy guard (we have no trustworthy counts to compare against).
    let stats = match ops.get_statistics() {
        Ok(s) => Some(s),
        Err(e) => {
            errors.push(format!("statistics read failed: {e}"));
            None
        }
    };

    // Each enumerator yields `Some(item_node_count)` on success (0 = read OK but
    // empty) or `None` when the read itself failed (already recorded in
    // `errors`). The count feeds the stats-vs-nodes discrepancy guard below.

    // Semantic facts — wildcard enumeration (confidence-desc), capped.
    let semantic_items = match ops.search_facts("*", cap, 0.0) {
        Ok(facts) => {
            let mut count = 0usize;
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
                count += 1;
            }
            Some(count)
        }
        Err(e) => {
            errors.push(format!("semantic-fact read failed: {e}"));
            None
        }
    };

    // Episodes — unfiltered enumeration, newest-first, capped.
    let episodic_items = match ops.list_all_episodes(cap) {
        Ok(episodes) => {
            let mut count = 0usize;
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
                count += 1;
            }
            Some(count)
        }
        Err(e) => {
            errors.push(format!("episodic-memory read failed: {e}"));
            None
        }
    };

    // Procedures — wildcard enumeration (usage-desc), capped.
    let procedural_items = match ops.recall_procedure("*", cap) {
        Ok(procedures) => {
            let mut count = 0usize;
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
                count += 1;
            }
            Some(count)
        }
        Err(e) => {
            errors.push(format!("procedural-memory read failed: {e}"));
            None
        }
    };

    // Prospective memories — every status, priority-ordered, capped.
    let prospective_items = match ops.list_all_prospective(cap) {
        Ok(prospective) => {
            let mut count = 0usize;
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
                count += 1;
            }
            Some(count)
        }
        Err(e) => {
            errors.push(format!("prospective-memory read failed: {e}"));
            None
        }
    };

    // Stats-vs-nodes discrepancy guard (issue #2627): if statistics claim an
    // ENUMERABLE type holds content while its enumerator read OK yet yielded no
    // item nodes, the store would be misrepresented as hub-only. Fail loud.
    // Scoped strictly to the four enumerable types — working memory and the
    // sensory buffer are transient and hub-only (no per-item enumerator), so
    // non-zero working/sensory counts with zero item nodes is a VALID state.
    // `emitted.is_some()` skips types whose read already failed (double-flag),
    // and a capped read always emits `cap > 0` items so capping never trips it.
    if let Some(s) = &stats {
        let checks: [(&str, u64, Option<usize>); 4] = [
            ("semantic fact", s.semantic_count, semantic_items),
            ("episodic memory", s.episodic_count, episodic_items),
            ("procedural memory", s.procedural_count, procedural_items),
            ("prospective memory", s.prospective_count, prospective_items),
        ];
        for (name, stat, emitted) in checks {
            if let Some(0) = emitted
                && stat > 0
            {
                errors.push(format!(
                    "{name} statistics report {stat} item(s) but the live \
                     enumerator returned none (stats-vs-nodes discrepancy)"
                ));
            }
        }
    }

    let stats_block = match &stats {
        Some(s) => json!({
            "working": s.working_count,
            "semantic": s.semantic_count,
            "episodic": s.episodic_count,
            "procedural": s.procedural_count,
            "prospective": s.prospective_count,
            "sensory": s.sensory_count,
        }),
        // Statistics could not be read — surface null, never a phantom all-zeros
        // block (the `error` field carries the real cause).
        None => Value::Null,
    };

    let mut payload = json!({
        "nodes": nodes,
        "edges": edges,
        // The reader WAS reachable (this builder only runs against a live `ops`),
        // so `available` stays true even for an in-build read failure — the
        // handler owns the reader-unreachable `available:false` branch.
        "available": true,
        "stats": stats_block,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    if !errors.is_empty() {
        // Single, non-empty top-level error surface. OMITTED entirely (not null)
        // on the success/empty paths so the client only shows the error overlay
        // for a genuine failure.
        payload["error"] = Value::String(format!(
            "Memory graph loaded with errors: {}",
            errors.join("; ")
        ));
    }

    payload
}

/// `GET /api/memory/graph` — LIVE cognitive-memory graph visualization
/// (issue #2627).
///
/// Restores the dedicated **Memory** tab's node/edge graph after the ~17→9 tab
/// consolidation left it a de-fork #2307 stub. Reads the live cognitive store
/// via a single shared reader ([`open_reader_client`]) and renders it through
/// [`build_live_memory_graph`]: six type hubs, live per-item nodes for the four
/// enumerable memory types linked to their hubs, and `stats` mirroring
/// `get_statistics()`. When the reader is unreachable the handler fails LOUD
/// (issue #2627): it returns a path-free top-level `error` with an empty graph
/// (`available:false`), never a silent blank or a hidden `note` — so the Memory
/// tab shows a visible error state instead of an empty canvas. The
/// client-facing `error` is sanitized (no filesystem paths, SR-DATA-1); the
/// underlying cause (which may embed the socket path) is logged server-side.
pub(crate) async fn memory_graph() -> Json<Value> {
    let state_root = resolve_state_root();
    match open_reader_client(&state_root) {
        Ok(reader) => Json(build_live_memory_graph(reader.ops())),
        Err(e) => {
            tracing::warn!(
                target: "simard::dashboard",
                error = %e,
                "memory_graph: cognitive reader unavailable; failing loud with an \
                 empty graph + error"
            );
            Json(json!({
                "nodes": [],
                "edges": [],
                "available": false,
                // Fail-LOUD, PATH-FREE (SR-DATA-1): the detailed cause `e` may
                // embed the socket/state-root path, so it is logged above rather
                // than returned. The client shows this as a visible error state.
                "error": "Cognitive memory reader is unavailable.",
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

    /// Build a snapshot at `epoch_secs` with every long-term rail equal to
    /// `long_term` (split across episodic) so the rate math is easy to assert.
    fn snap_at(epoch_secs: f64, long_term: u64) -> MemorySnapshot {
        MemorySnapshot {
            timestamp: "".into(),
            epoch_secs,
            sensory: 0,
            working: 0,
            episodic: long_term,
            semantic: 0,
            procedural: 0,
            prospective: 0,
            total: long_term,
            long_term_total: long_term,
        }
    }

    /// #4107: an ancient baseline outside the trailing 24 h window must NOT be
    /// used. The rate is anchored at the newest sample and diffs against the
    /// oldest sample *inside* the window, so a multi-week-old first snapshot
    /// cannot dilute the denominator to a meaningless near-zero figure.
    #[test]
    fn rate_per_hour_ignores_ancient_baseline_outside_window() {
        let day = 86_400.0;
        // Newest at t=40 days. An ancient sample 39 days ago (outside the 24 h
        // window) plus a fresh one 1 h before newest (inside the window).
        let newest_t = 40.0 * day;
        let snaps = vec![
            snap_at(newest_t - 39.0 * day, 100), // ancient, out of window
            snap_at(newest_t - 3600.0, 200),     // 1 h ago, in window
            snap_at(newest_t, 260),              // newest
        ];
        let r = rate_per_hour(&snaps);
        // Windowed: (260 - 200) / 1 h = 60/hr — NOT the whole-history
        // (260 - 100) / (~40 days) ≈ 0.17/hr the naive calc would report.
        assert_eq!(r["long_term_total"], 60.0);
        assert_eq!(r["episodic"], 60.0);
    }

    /// #4107: when only the newest sample lies inside the trailing window
    /// (a fresh daemon after a long down-gap), report 0.0 — an honest
    /// "insufficient recent data" rather than a distorted whole-span figure.
    #[test]
    fn rate_per_hour_no_recent_sample_reports_zero() {
        let day = 86_400.0;
        let newest_t = 10.0 * day;
        let snaps = vec![
            snap_at(newest_t - 8.0 * day, 100), // way outside the 24 h window
            snap_at(newest_t, 500),             // newest — the only in-window sample
        ];
        let r = rate_per_hour(&snaps);
        assert_eq!(r["long_term_total"], 0.0, "no in-window baseline → 0.0");
        assert_eq!(r["total"], 0.0);
    }

    /// #4107: the trailing-window edge is inclusive. A baseline exactly at
    /// `newest - GROWTH_RATE_WINDOW_SECS` is used (not dropped by an off-by-one).
    #[test]
    fn rate_per_hour_window_edge_is_inclusive() {
        let newest_t = 1_000_000.0;
        let snaps = vec![
            // Exactly 24 h before newest — must be included.
            snap_at(newest_t - GROWTH_RATE_WINDOW_SECS, 100),
            snap_at(newest_t, 124),
        ];
        let r = rate_per_hour(&snaps);
        // (124 - 100) / 24 h = 1.0/hr.
        assert_eq!(r["long_term_total"], 1.0);
    }

    /// #4107: a sample just *outside* the window edge is excluded, and the
    /// oldest sample still *inside* the window becomes the baseline.
    #[test]
    fn rate_per_hour_selects_oldest_in_window_baseline() {
        let newest_t = 1_000_000.0;
        let snaps = vec![
            // 1 second past the 24 h edge — excluded.
            snap_at(newest_t - GROWTH_RATE_WINDOW_SECS - 1.0, 0),
            // Oldest inside the window (12 h ago) — the chosen baseline.
            snap_at(newest_t - 12.0 * 3600.0, 100),
            snap_at(newest_t - 3600.0, 130),
            snap_at(newest_t, 160),
        ];
        let r = rate_per_hour(&snaps);
        // (160 - 100) / 12 h = 5.0/hr, using the oldest in-window sample.
        assert_eq!(r["long_term_total"], 5.0);
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
            v["nodes"]
        );
        assert!(
            text.contains(ep_marker),
            "the live episode {ep_marker:?} must appear in the graph nodes. Nodes: {}",
            v["nodes"]
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
            v["nodes"]
        );
        assert!(
            text.contains(prosp_marker),
            "the live prospective memory {prosp_marker:?} must appear in the graph \
             nodes; nodes: {}",
            v["nodes"]
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

    // -----------------------------------------------------------------------
    // Fail-LOUD contract (issue #2627): the memory graph must NEVER silently
    // blank. A data-load failure — reader unreachable, a per-type
    // statistics/enumeration read `Err`, or a stats-vs-nodes discrepancy — must
    // surface a top-level `error` string; a genuinely empty store is a distinct,
    // VALID non-error state. See
    // docs/reference/dashboard-memory-graph-fail-loud.md.
    //
    // TDD note (Step 7 / red): these FAIL against the current builder, which
    // swallows every read error via `unwrap_or_default()` / `if let Ok(...)` and
    // never emits `error`, and whose handler returns a silent `available:false`
    // + `note` payload on reader failure. They pass once the builder/handler are
    // rewired to fail loud.
    // -----------------------------------------------------------------------

    use crate::error::{SimardError, SimardResult};
    use crate::memory_cognitive::{
        CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
        CognitiveWorkingSlot,
    };

    /// True iff the payload carries a top-level `error` key that is a non-empty
    /// string. The contract OMITS `error` entirely (not `null`) on the
    /// success/empty paths, so a present-but-null key is treated as absent.
    fn error_string(v: &Value) -> Option<String> {
        v.as_object()
            .and_then(|m| m.get("error"))
            .and_then(|e| e.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// Whether the top-level `error` key is present at all. On the non-error
    /// paths the key must be OMITTED (not serialized as `null`).
    fn has_error_key(v: &Value) -> bool {
        v.as_object().is_some_and(|m| m.contains_key("error"))
    }

    /// The set of `type` literals carried by the always-present type-hub nodes
    /// (`"hub": true`).
    fn hub_types(v: &Value) -> HashSet<String> {
        nodes_of(v)
            .iter()
            .filter(|n| n.get("hub").and_then(Value::as_bool) == Some(true))
            .filter_map(|n| n["type"].as_str().map(str::to_string))
            .collect()
    }

    /// An in-build failure (per-type read `Err` or discrepancy) must still keep
    /// the six type hubs so the legend and all six filters stay meaningful.
    fn assert_six_hub_types(v: &Value) {
        let hubs = hub_types(v);
        for t in NODE_TYPE_ALLOWLIST {
            assert!(
                hubs.contains(t),
                "an in-build failure must keep the six type hubs (legend/filters stay \
                 meaningful); missing hub {t:?}. Nodes: {}",
                v["nodes"]
            );
        }
    }

    /// Configurable fault-injecting [`CognitiveMemoryOps`] double. Each read the
    /// graph builder relies on can be flipped to return `Err` (the `*_err`
    /// flags) or to yield a controlled value; `get_statistics` returns the
    /// `stats` block verbatim. This exercises the fail-LOUD paths deterministically
    /// without a live daemon — a read fault, or a store whose `stats` claim
    /// content while the matching enumerator yields nothing (the stats-vs-nodes
    /// discrepancy). Every write op is a benign `Ok` (never used by the builder).
    #[derive(Default)]
    struct GraphFaultOps {
        stats: CognitiveStatistics,
        stats_err: bool,
        facts_err: bool,
        episodes_err: bool,
        procedures_err: bool,
        prospective_err: bool,
        facts: Vec<CognitiveFact>,
        episodes: Vec<CognitiveEpisode>,
        procedures: Vec<CognitiveProcedure>,
        prospective: Vec<CognitiveProspective>,
    }

    impl GraphFaultOps {
        fn boom(op: &str) -> SimardError {
            SimardError::MemoryIntegrityError {
                path: std::path::PathBuf::from("<graph-fault-double>"),
                reason: format!("injected {op} read fault (fail-loud test double)"),
            }
        }
    }

    impl CognitiveMemoryOps for GraphFaultOps {
        fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
            Ok(String::new())
        }
        fn prune_expired_sensory(&self) -> SimardResult<usize> {
            Ok(0)
        }
        fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
            Ok(String::new())
        }
        fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
            Ok(vec![])
        }
        fn clear_working(&self, _t: &str) -> SimardResult<usize> {
            Ok(0)
        }
        fn store_episode(
            &self,
            _c: &str,
            _s: &str,
            _m: Option<&serde_json::Value>,
        ) -> SimardResult<String> {
            Ok(String::new())
        }
        fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
            Ok(None)
        }
        fn store_fact(
            &self,
            _c: &str,
            _co: &str,
            _cf: f64,
            _t: &[String],
            _s: &str,
        ) -> SimardResult<String> {
            Ok(String::new())
        }
        fn search_facts(&self, _q: &str, _l: u32, _m: f64) -> SimardResult<Vec<CognitiveFact>> {
            if self.facts_err {
                return Err(Self::boom("search_facts"));
            }
            Ok(self.facts.clone())
        }
        fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
            Ok(String::new())
        }
        fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
            if self.procedures_err {
                return Err(Self::boom("recall_procedure"));
            }
            Ok(self.procedures.clone())
        }
        fn store_prospective(
            &self,
            _d: &str,
            _tc: &str,
            _a: &str,
            _p: i64,
        ) -> SimardResult<String> {
            Ok(String::new())
        }
        fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
            Ok(vec![])
        }
        fn list_all_episodes(&self, _limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
            if self.episodes_err {
                return Err(Self::boom("list_all_episodes"));
            }
            Ok(self.episodes.clone())
        }
        fn list_all_prospective(&self, _limit: u32) -> SimardResult<Vec<CognitiveProspective>> {
            if self.prospective_err {
                return Err(Self::boom("list_all_prospective"));
            }
            Ok(self.prospective.clone())
        }
        fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
            if self.stats_err {
                return Err(Self::boom("get_statistics"));
            }
            Ok(self.stats.clone())
        }
    }

    #[test]
    fn build_live_memory_graph_surfaces_statistics_read_error() {
        // Trigger #2 (per-type read Err): a failing get_statistics() must NOT be
        // swallowed by `unwrap_or_default()` into a phantom all-zeros graph.
        let ops = GraphFaultOps {
            stats_err: true,
            ..Default::default()
        };
        let v = build_live_memory_graph(&ops);
        assert!(
            error_string(&v).is_some(),
            "a get_statistics() read error must surface as a non-empty `error` field \
             (fail-loud), not be swallowed by unwrap_or_default(); got {v}"
        );
        assert_eq!(
            v["available"],
            Value::Bool(true),
            "an in-build read failure keeps available:true (the reader WAS reachable); got {v}"
        );
        assert_six_hub_types(&v);
    }

    #[test]
    fn build_live_memory_graph_surfaces_each_enumerator_read_error() {
        // Trigger #2 for every per-item enumerator: a read Err from any of the four
        // trait enumerators must surface as `error`, not be dropped by the old
        // `if let Ok(...)` swallow that silently produced a partial graph.
        type FaultSetter = fn(&mut GraphFaultOps);
        let cases: [(&str, FaultSetter); 4] = [
            ("search_facts", |o| o.facts_err = true),
            ("list_all_episodes", |o| o.episodes_err = true),
            ("recall_procedure", |o| o.procedures_err = true),
            ("list_all_prospective", |o| o.prospective_err = true),
        ];
        for (name, set) in cases {
            let mut ops = GraphFaultOps::default();
            set(&mut ops);
            let v = build_live_memory_graph(&ops);
            assert!(
                error_string(&v).is_some(),
                "a {name}() read error must surface as a non-empty `error` field \
                 (fail-loud), not be swallowed into a partial graph; got {v}"
            );
            assert_eq!(
                v["available"],
                Value::Bool(true),
                "an in-build enumerator failure keeps available:true; got {v}"
            );
            assert_six_hub_types(&v);
        }
    }

    #[test]
    fn build_live_memory_graph_flags_stats_vs_nodes_discrepancy_per_enumerable_type() {
        // Trigger #3: a store whose stats claim an ENUMERABLE type holds content
        // while its enumerator yields zero item nodes would misrepresent a populated
        // store as hub-only. The builder must fail loud rather than silently degrade.
        for ty in ["semantic", "episodic", "procedural", "prospective"] {
            let mut ops = GraphFaultOps::default();
            match ty {
                "semantic" => ops.stats.semantic_count = 5,
                "episodic" => ops.stats.episodic_count = 5,
                "procedural" => ops.stats.procedural_count = 5,
                "prospective" => ops.stats.prospective_count = 5,
                _ => unreachable!(),
            }
            let v = build_live_memory_graph(&ops);
            assert!(
                error_string(&v).is_some(),
                "stats.{ty} = 5 but 0 item nodes for it must surface a discrepancy \
                 `error` (fail-loud), not present a populated store as hub-only; got {v}"
            );
            assert_eq!(
                v["available"],
                Value::Bool(true),
                "a discrepancy is an in-build failure — available stays true; got {v}"
            );
            assert_six_hub_types(&v);
        }
    }

    #[test]
    fn build_live_memory_graph_discrepancy_guard_exempts_transient_types() {
        // False-positive guard: working memory + the sensory buffer are transient
        // and hub-only (no per-item enumerator), so non-zero working/sensory counts
        // with zero item nodes is a VALID state — it must NOT trip the discrepancy
        // error. The guard is scoped strictly to the four enumerable types.
        let mut ops = GraphFaultOps::default();
        ops.stats.working_count = 9;
        ops.stats.sensory_count = 9;
        let v = build_live_memory_graph(&ops);
        assert!(
            !has_error_key(&v),
            "non-zero working/sensory counts with hub-only rendering is valid and must \
             NOT be flagged as an error (discrepancy guard is scoped to the four \
             enumerable types); got {v}"
        );
        assert_eq!(v["available"], Value::Bool(true));
        assert_six_hub_types(&v);
    }

    #[test]
    fn build_live_memory_graph_genuine_empty_is_not_an_error() {
        // A truly empty store (all enumerable stats 0, every read Ok+empty) is a
        // distinct, VALID state: six hubs, no item nodes, no edges, available:true,
        // and `error` OMITTED (not null) so the client shows the neutral empty
        // message rather than the error overlay.
        let ops = GraphFaultOps::default();
        let v = build_live_memory_graph(&ops);
        assert!(
            !has_error_key(&v),
            "a genuinely empty store must OMIT the `error` key entirely (not null); got {v}"
        );
        assert_eq!(v["available"], Value::Bool(true));
        assert_six_hub_types(&v);
        let item_nodes = nodes_of(&v)
            .iter()
            .filter(|n| n.get("hub").and_then(Value::as_bool) != Some(true))
            .count();
        assert_eq!(
            item_nodes, 0,
            "an empty store must render hubs only, no item nodes; got {v}"
        );
        assert!(
            edges_of(&v).is_empty(),
            "an empty store has no edges; got {v}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn memory_graph_handler_reader_unavailable_surfaces_path_free_error() {
        // Fail-LOUD trigger #1 (reader unreachable): when open_reader_client() fails
        // closed (socket present but unconnectable), the handler must return a
        // sanitized `error` — NOT the retired silent `available:false` + `note`
        // payload — and the error must be PATH-FREE (SR-DATA-1): no state_root,
        // $HOME, or temp-dir path may leak to the client. Invariant on this branch:
        // `error` present <=> nodes == [] && available == false.
        use crate::memory_ipc::{
            clear_in_process_writer, clear_tier2_store_cache, socket_path_for,
        };

        clear_in_process_writer();
        clear_tier2_store_cache();
        let state = HermeticState::new();
        let root = state.state_root().to_path_buf();

        // Occupy the socket path with a plain file: `sock.exists()` is true but the
        // connect fails, so open_reader_client() must fail closed (no divergent
        // tier-2 fallback). This is the deterministic way to force the
        // reader-unavailable branch without a live daemon (mirrors
        // memory_ipc::tests_launcher_fail_closed_2896).
        let sock = socket_path_for(&root);
        if let Some(parent) = sock.parent() {
            std::fs::create_dir_all(parent).expect("create socket parent dir");
        }
        std::fs::write(&sock, b"not a socket").expect("occupy socket path");

        let out = memory_graph().await;
        let v = &out.0;

        clear_tier2_store_cache();

        let err = error_string(v).unwrap_or_else(|| {
            panic!(
                "reader-unavailable must surface a non-empty `error` (fail-loud), \
                 replacing the retired silent available:false + note payload; got {v}"
            )
        });
        assert_eq!(
            nodes_of(v).len(),
            0,
            "reader-unavailable invariant: nodes must be [] when error is present; got {v}"
        );
        assert_eq!(
            v["available"],
            Value::Bool(false),
            "reader-unavailable invariant: available must be false; got {v}"
        );
        assert!(
            v.as_object().is_some_and(|m| !m.contains_key("note")),
            "the retired silent `note` fallback must be replaced by `error` (single \
             error surface); got {v}"
        );

        // SR-DATA-1: no filesystem path may leak into the client-facing error string.
        let root_str = root.to_string_lossy();
        assert!(
            !err.contains(root_str.as_ref()),
            "the client-facing `error` must be PATH-FREE (SR-DATA-1) — it leaked the \
             state_root path {root_str:?}: {err:?}"
        );
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy().to_string();
            if !home.is_empty() {
                assert!(
                    !err.contains(&home),
                    "the client-facing `error` leaked $HOME {home:?}: {err:?}"
                );
            }
        }
    }
}
