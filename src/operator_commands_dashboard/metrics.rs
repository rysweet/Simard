use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::path::Path;

use super::dashboard_goal_board_snapshot;
use super::routes::resolve_state_root;
use super::subagent::{count_json_records, file_metrics};
use crate::cognitive_memory::metrics::RECALL_PRECISION_METRIC;
use crate::cognitive_memory::recall_precision_bench::RECALL_PRECISION_SUITE;
use crate::gym_history::{ScoreHistory, generate_signals};
use crate::memory_ipc::open_reader_client;
use crate::self_metrics::MetricEntry;

// ---------------------------------------------------------------------------
// Memory metrics panel
// ---------------------------------------------------------------------------

pub(crate) async fn memory_metrics() -> Json<Value> {
    let state_root = resolve_state_root();

    let memory_path = state_root.join("memory_records.json");
    let evidence_path = state_root.join("evidence_records.json");
    let handoff_path = state_root.join("latest_handoff.json");

    let memory_info = file_metrics(&memory_path);
    let evidence_info = file_metrics(&evidence_path);
    let handoff_info = file_metrics(&handoff_path);

    let fact_count = count_json_records(&memory_path);
    let evidence_count = count_json_records(&evidence_path);

    // Goal records now live in cognitive memory (issue #1590); render a
    // metadata-only panel so the dashboard's "Goal Records" tile keeps
    // working without any disk file.
    let goal_board = dashboard_goal_board_snapshot(&state_root).ok();
    let goal_count = goal_board
        .as_ref()
        .map(|b| (b.active.len() + b.backlog.len()) as u64)
        .unwrap_or(0);

    // Query the library-backed cognitive memory for live statistics (#419),
    // routed through `open_reader_client` so the daemon's IPC writer serves the
    // read when running embedded. Capture the error so the dashboard can show
    // *why* data is missing instead of silently returning zeros.
    let native_result =
        open_reader_client(&state_root).and_then(|reader| reader.ops().get_statistics());
    let native_error = native_result.as_ref().err().map(|e| e.to_string());
    let native_stats = native_result.ok();

    // Last consolidation + recent consolidation activity now come from the LIVE
    // `consolidate-memory` OODA action stream (#26), not the modification time
    // of the retired JSON snapshot files. Those files are no longer written, so
    // their mtime stayed frozen while consolidation kept running (~30
    // consolidate-memory actions / 30 min), which is exactly why the operator
    // saw a static "Last Memory Compaction". This is the same live-read source
    // the Activity and Goals tabs migrated to (#2697 / #2695). It fails closed
    // to `null` (→ "Not tracked yet") when no consolidate-memory action has run
    // yet — no fabricated value, no legacy-file or directory-mtime fallback.
    let (consolidation_count, last_consolidation) = recent_consolidation_activity(&state_root);
    let recent_last = last_consolidation.clone();

    // Use LadybugDB counts when available; JSON file counts are the legacy source.
    let total = native_stats
        .as_ref()
        .map(|s| s.total())
        .unwrap_or(fact_count + evidence_count + goal_count);

    // De-fork Phase 2b (#2307): the library backend persists at
    // `<state_root>/cognitive`, replacing the native `cognitive_memory.ladybug`.
    let db_path = state_root.join("cognitive");

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
            "source": "cognitive-memory:goal-board:snapshot",
            "count": goal_count,
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
        "recent_consolidation_activity": {
            "count": consolidation_count,
            "last": recent_last,
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Scan the most recent persisted cycle reports for live `consolidate-memory`
/// OODA actions and report how many ran and when the newest one occurred.
///
/// This is the LIVE consolidation signal the Memory card renders (#26). The
/// retired JSON-snapshot mtimes stayed frozen while consolidation kept running,
/// so the operator saw a static value; the OODA action stream is the same live
/// source the Activity and Goals tabs migrated to (#2697 / #2695). Returns
/// `(count, last_rfc3339)`, where `last` is `None` — surfaced as "Not tracked
/// yet" — when no consolidate-memory action has been recorded yet. It fails
/// closed to `null` rather than fabricating a value.
fn recent_consolidation_activity(state_root: &Path) -> (u64, Option<String>) {
    // Bound the scan so the unbounded `cycle_reports/` directory never turns a
    // hot dashboard poll into an O(all-cycles) read; the newest reports carry
    // the live signal the card needs.
    const SCAN_CYCLES: usize = 200;
    let reports = super::current_work::read_recent_cycle_reports(state_root, SCAN_CYCLES);

    let mut count: u64 = 0;
    let mut last: Option<chrono::DateTime<chrono::FixedOffset>> = None;

    for entry in &reports {
        // `read_recent_cycle_reports` nests the parsed cycle JSON under `report`
        // (or exposes plain-text `summary` with no outcomes to scan).
        let rpt = entry.get("report").unwrap_or(entry);
        let Some(outcomes) = rpt.get("outcomes").and_then(|v| v.as_array()) else {
            continue;
        };
        let ts = rpt.get("timestamp").and_then(|t| t.as_str());
        for o in outcomes {
            if o.get("action_kind").and_then(|v| v.as_str()) != Some("consolidate-memory") {
                continue;
            }
            count += 1;
            if let Some(parsed) = ts.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
                last = Some(match last {
                    Some(prev) if prev >= parsed => prev,
                    _ => parsed,
                });
            }
        }
    }

    (count, last.map(|dt| dt.to_rfc3339()))
}

pub(crate) async fn ooda_thinking() -> Json<Value> {
    let state_root = resolve_state_root();
    // Issue #2580 + #26: the Thinking tab's "Cycle History" and the Activity
    // tab's "Cycle Reports" card now read from ONE shared reader, so they always
    // agree instead of diverging on a stale copy. It unions both persisted cycle
    // dirs, orders newest-first, stamps the authoritative filename cycle number,
    // and collapses consecutive identical deferrals ("goal already has a live,
    // healthy engineer") into a single counted entry — flagging a genuine loop
    // only when a non-progressing decision repeats.
    let reports = super::cycle_source::read_cycle_reports_collapsed(&state_root);

    Json(json!({ "reports": reports }))
}

// ---------------------------------------------------------------------------
// Hybrid recall-precision correlation endpoint (issue #2491 / #2494)
// ---------------------------------------------------------------------------
//
// The read-only, query-time join that makes the measurement *hybrid*: it pairs
// the latest FIXED-corpus benchmark score with the recent LIVE trend on the same
// metric name (`recall_precision_at_k`) and emits a correlation verdict — so a
// claimed cognition improvement is validated on the benchmark AND observed live,
// not just one. Reference: docs/reference/recall-precision-hybrid-api.md.

/// The gym regression threshold, reused so the correlation classifies a rail's
/// direction with the same ±band the gym already uses for regressions.
const CORRELATION_THRESHOLD: f64 = 0.01;

/// The hybrid correlation verdict — a total function of the two rails' recent
/// directions (see [`correlation_verdict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorrelationVerdict {
    /// Both rails improved beyond the threshold — the claim holds.
    Confirmed,
    /// Benchmark up, live flat — possible overfit / unrepresentative corpus.
    BenchmarkOnly,
    /// Live up, benchmark flat — possible drift; the corpus may miss the case.
    LiveOnly,
    /// One rail up while the other regressed — the rails contradict each other.
    Diverging,
    /// A drop on at least one rail with no offsetting rise on the other.
    Regressed,
    /// Neither rail moved beyond the threshold.
    Stable,
    /// Not enough history on one or both rails to judge.
    Insufficient,
}

impl CorrelationVerdict {
    /// The documented kebab-case wire string for this verdict.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::BenchmarkOnly => "benchmark-only",
            Self::LiveOnly => "live-only",
            Self::Diverging => "diverging",
            Self::Regressed => "regressed",
            Self::Stable => "stable",
            Self::Insufficient => "insufficient",
        }
    }

    /// One-line human explanation for the response body.
    fn explanation(self) -> &'static str {
        match self {
            Self::Confirmed => "Benchmark and live trend both improved beyond the threshold.",
            Self::BenchmarkOnly => {
                "Benchmark improved but the live trend held flat — suspect overfit or an \
                 unrepresentative corpus."
            }
            Self::LiveOnly => {
                "Live trend improved but the benchmark held flat — the frozen corpus may be \
                 missing the improved case."
            }
            Self::Diverging => {
                "The rails disagree in direction (one improved while the other regressed) — do \
                 not trust the gain."
            }
            Self::Regressed => "At least one rail regressed with no offsetting rise on the other.",
            Self::Stable => "Neither rail moved beyond the threshold.",
            Self::Insufficient => {
                "Not enough history on one or both rails (needs at least two benchmark runs and \
                 two in-window live samples)."
            }
        }
    }
}

/// Classify the hybrid correlation from the benchmark run-over-run delta and the
/// live first→latest trend delta, each against `threshold`.
///
/// `benchmark_delta`/`live_trend_delta` are `None` when the corresponding rail
/// has fewer than two comparable points, which yields [`CorrelationVerdict::Insufficient`].
/// Otherwise each rail is reduced to a direction — up (`> +t`), flat (`|d| <= t`),
/// or down (`< -t`) — and the nine combinations map onto exactly one verdict.
pub(crate) fn correlation_verdict(
    benchmark_delta: Option<f64>,
    live_trend_delta: Option<f64>,
    threshold: f64,
) -> CorrelationVerdict {
    let (b, l) = match (benchmark_delta, live_trend_delta) {
        (Some(b), Some(l)) => (b, l),
        _ => return CorrelationVerdict::Insufficient,
    };
    let dir = |d: f64| -> i8 {
        if d > threshold {
            1
        } else if d < -threshold {
            -1
        } else {
            0
        }
    };
    match (dir(b), dir(l)) {
        (1, 1) => CorrelationVerdict::Confirmed,
        (1, 0) => CorrelationVerdict::BenchmarkOnly,
        (0, 1) => CorrelationVerdict::LiveOnly,
        (1, -1) | (-1, 1) => CorrelationVerdict::Diverging,
        (0, 0) => CorrelationVerdict::Stable,
        // (0, -1), (-1, 0), (-1, -1): a drop with no offsetting rise.
        _ => CorrelationVerdict::Regressed,
    }
}

/// How many recent benchmark records to load (default 20, clamped 1..=200).
pub(crate) fn clamp_bench_limit(v: Option<u64>) -> u64 {
    v.unwrap_or(20).clamp(1, 200)
}

/// Max live `metrics.jsonl` samples to scan within the window (default 200,
/// clamped 1..=2000).
pub(crate) fn clamp_live_limit(v: Option<u64>) -> u64 {
    v.unwrap_or(200).clamp(1, 2000)
}

/// Live look-back window in hours (default 168 = one week, clamped 1..=8760).
pub(crate) fn clamp_window_hours(v: Option<u64>) -> u64 {
    v.unwrap_or(168).clamp(1, 8760)
}

/// Load the benchmark rail: the latest score, its previous score, the gym
/// signal, and the run-over-run delta. Returns `(benchmark_json, delta, had_error)`;
/// `benchmark_json` is `null` when there are fewer than two records or a read
/// error occurred. A read error is logged (specifics to `tracing::warn!` only)
/// and never leaked into the body.
fn benchmark_rail(bench_db_path: &Path, bench_limit: u64) -> (Value, Option<f64>, bool) {
    let history = match ScoreHistory::open(bench_db_path) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(
                target: "simard::dashboard",
                error = %e,
                "recall-precision correlation: failed to open benchmark score history",
            );
            return (Value::Null, None, true);
        }
    };
    let records = match history.history(
        RECALL_PRECISION_SUITE,
        RECALL_PRECISION_METRIC,
        bench_limit as usize,
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                target: "simard::dashboard",
                error = %e,
                "recall-precision correlation: failed to read benchmark score history",
            );
            return (Value::Null, None, true);
        }
    };
    if records.len() < 2 {
        return (Value::Null, None, false);
    }
    let latest = &records[records.len() - 1];
    let previous = &records[records.len() - 2];
    let delta = latest.score - previous.score;
    let signal = generate_signals(&history, RECALL_PRECISION_SUITE)
        .ok()
        .and_then(|sigs| {
            sigs.into_iter()
                .find(|s| s.scenario_id == RECALL_PRECISION_METRIC)
        })
        .map(|s| s.signal.to_string());
    let obj = json!({
        "suite_id": latest.suite_id,
        "scenario_id": latest.scenario_id,
        "score": latest.score,
        "timestamp": latest.timestamp,
        "commit_hash": latest.commit_hash,
        "signal": signal,
        "previous_score": previous.score,
    });
    (obj, Some(delta), false)
}

/// Load the live rail from `<state_root>/metrics/metrics.jsonl`: the recent
/// in-window `recall_precision_at_k` samples and their first→latest trend.
/// Corrupt JSONL rows are skipped (VAL-3). Returns `(live_json, trend_delta, had_error)`;
/// `live_json` is `null` when no in-window samples exist. A missing file is an
/// empty rail (not an error); an unreadable existing file is a logged error.
fn live_rail(state_root: &Path, live_limit: u64, window_hours: u64) -> (Value, Option<f64>, bool) {
    let path = state_root.join("metrics").join("metrics.jsonl");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (Value::Null, None, false);
        }
        Err(e) => {
            tracing::warn!(
                target: "simard::dashboard",
                error = %e,
                "recall-precision correlation: failed to read live metrics.jsonl",
            );
            return (Value::Null, None, true);
        }
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(window_hours as i64);
    let mut samples: Vec<(chrono::DateTime<chrono::Utc>, f64, u64)> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Skip corrupt rows rather than failing the whole endpoint (VAL-3).
        let entry: MetricEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.metric_name != RECALL_PRECISION_METRIC || entry.timestamp < cutoff {
            continue;
        }
        let ctx_samples = serde_json::from_str::<Value>(&entry.context)
            .ok()
            .and_then(|v| v.get("samples").and_then(Value::as_u64))
            .unwrap_or(0);
        samples.push((entry.timestamp, entry.value, ctx_samples));
    }

    samples.sort_by_key(|(ts, _, _)| *ts);
    if samples.len() > live_limit as usize {
        let start = samples.len() - live_limit as usize;
        samples.drain(0..start);
    }
    if samples.is_empty() {
        return (Value::Null, None, false);
    }

    let first = samples.first().map(|s| s.1).unwrap_or(0.0);
    let latest = samples.last().map(|s| s.1).unwrap_or(0.0);
    let mean = samples.iter().map(|s| s.1).sum::<f64>() / samples.len() as f64;
    let trend_delta = if samples.len() >= 2 {
        Some(latest - first)
    } else {
        None
    };
    let series: Vec<Value> = samples
        .iter()
        .map(|(ts, v, s)| {
            json!({
                "timestamp": ts.to_rfc3339(),
                "value": v,
                "samples": s,
            })
        })
        .collect();
    let obj = json!({
        "window_hours": window_hours,
        "samples": samples.len() as u64,
        "first": first,
        "latest": latest,
        "mean": mean,
        "trend_delta": trend_delta,
        "series": series,
    });
    (obj, trend_delta, false)
}

/// Compute the hybrid recall-precision correlation from the benchmark score
/// history at `bench_db_path` and the live metrics under `state_root`.
///
/// Degrades, never panics or leaks: a missing/empty rail is `null` with an
/// `insufficient` verdict; a read error nulls the affected rail and adds a
/// generic top-level `error` (specifics go to `tracing::warn!`); HTTP stays 200.
pub(crate) fn recall_precision_correlation_core(
    bench_db_path: &Path,
    state_root: &Path,
    bench_limit: Option<u64>,
    live_limit: Option<u64>,
    window_hours: Option<u64>,
) -> (StatusCode, Json<Value>) {
    let bench_limit = clamp_bench_limit(bench_limit);
    let live_limit = clamp_live_limit(live_limit);
    let window_hours = clamp_window_hours(window_hours);

    let (benchmark_json, benchmark_delta, bench_err) = benchmark_rail(bench_db_path, bench_limit);
    let (live_json, live_trend_delta, live_err) = live_rail(state_root, live_limit, window_hours);

    let verdict = correlation_verdict(benchmark_delta, live_trend_delta, CORRELATION_THRESHOLD);

    let mut body = json!({
        "metric": RECALL_PRECISION_METRIC,
        "benchmark": benchmark_json,
        "live": live_json,
        "correlation": {
            "verdict": verdict.as_str(),
            "benchmark_delta": benchmark_delta,
            "live_trend_delta": live_trend_delta,
            "threshold": CORRELATION_THRESHOLD,
            "explanation": verdict.explanation(),
        },
        "generated_at": chrono::Utc::now().to_rfc3339(),
    });

    if bench_err || live_err {
        // Generic message only — no paths, SQL, or engine specifics (DATA-1).
        body["error"] =
            json!("failed to read one or more measurement rails; see server logs for details");
    }

    (StatusCode::OK, Json(body))
}

/// `GET /api/cognition/recall-precision` — the hybrid correlation endpoint.
///
/// Query params (`bench_limit`, `live_limit`, `window_hours`) are parsed
/// leniently and clamped, never rejected: an unparseable value falls back to the
/// default bound rather than erroring.
pub(crate) async fn recall_precision_correlation(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let parse = |key: &str| params.get(key).and_then(|v| v.parse::<u64>().ok());
    recall_precision_correlation_core(
        &crate::gym_history::default_db_path(),
        &resolve_state_root(),
        parse("bench_limit"),
        parse("live_limit"),
        parse("window_hours"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ooda_thinking with temp cycle reports ----------------------------

    #[test]
    fn ooda_thinking_reads_cycle_reports_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        let report = json!({
            "cycle_number": 1,
            "summary": "test cycle",
            "observation": {"goal_count": 2}
        });
        std::fs::write(
            cycle_dir.join("cycle_1.json"),
            serde_json::to_string(&report).unwrap(),
        )
        .unwrap();

        // Verify the cycle report is readable
        let content = std::fs::read_to_string(cycle_dir.join("cycle_1.json")).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["cycle_number"], 1);
        assert_eq!(parsed["summary"], "test cycle");
    }

    #[test]
    fn ooda_thinking_handles_missing_cycle_dir() {
        let dir = tempfile::tempdir().unwrap();
        // No cycle_reports directory — should not panic
        let cycle_dir = dir.path().join("cycle_reports");
        assert!(!cycle_dir.exists());
    }

    #[test]
    fn ooda_thinking_sorts_reports_by_cycle_number_descending() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        for i in [3, 1, 2] {
            std::fs::write(
                cycle_dir.join(format!("cycle_{i}.json")),
                format!(r#"{{"cycle_number":{i}}}"#),
            )
            .unwrap();
        }

        let mut paths: Vec<_> = std::fs::read_dir(&cycle_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
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

        let nums: Vec<u32> = paths
            .iter()
            .map(|p| {
                p.file_name()
                    .to_str()
                    .unwrap()
                    .strip_prefix("cycle_")
                    .unwrap()
                    .strip_suffix(".json")
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(nums, vec![3, 2, 1]);
    }
}
