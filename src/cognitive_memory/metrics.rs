//! Cognitive-memory in-process self-metrics.
//!
//! Two lightweight, test-snapshotable metric families live here:
//!
//! * **Silent-drop counter** (issue #1975) — mirrors the
//!   `meeting_silent_drop_total` pattern from #1956: an in-process
//!   `OnceLock<HashMap<(kind, site), AtomicU64>>` counter that tests can
//!   snapshot and reset without touching global state outside their scope.
//! * **Recall precision\@k aggregate** — a running mean of the ranked
//!   fact-recall path's precision\@k, folded on every recall (no per-recall I/O)
//!   and drained once per OODA cycle into the durable `recall_precision_at_k`
//!   `metrics.jsonl` series, so ranking quality is observable over time and a
//!   regression is visible rather than silent.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::memory_cognitive::CognitiveFact;

/// Label pair for each counter bucket.
type Key = (String, String);

fn counters() -> &'static Mutex<HashMap<Key, AtomicU64>> {
    static COUNTERS: OnceLock<Mutex<HashMap<Key, AtomicU64>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Increment the silent-drop counter for `(kind, site)`.
pub fn increment(kind: &str, site: &str) {
    let mut map = counters().lock().expect("metrics lock poisoned");
    map.entry((kind.to_owned(), site.to_owned()))
        .or_insert_with(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the current counter value for `(kind, site)`.
pub fn cognitive_memory_silent_drop_count(kind: &str, site: &str) -> u64 {
    let map = counters().lock().expect("metrics lock poisoned");
    map.get(&(kind.to_owned(), site.to_owned()))
        .map(|v| v.load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// Reset **all** counters to zero.  For serial-test isolation only.
pub fn scoped_reset() {
    let mut map = counters().lock().expect("metrics lock poisoned");
    map.clear();
    drop(map);
    precision_agg()
        .lock()
        .expect("metrics lock poisoned")
        .clear();
}

// ───────────────────────── recall precision@k ──────────────────────────────
//
// A relevance-ranked-recall quality signal for the ranked fact-recall path
// (`recall_facts_ranked`). `precision_at_k` delegates to the upstream
// measurement primitive (guideline G2); the aggregate below folds one
// observation per recall so the per-cycle sweep can emit a single durable
// sample instead of writing `metrics.jsonl` on the hot recall path.

/// Durable self-metric name for ranked-recall precision, emitted to
/// `metrics.jsonl` once per OODA cycle by [`flush_recall_precision_metric`].
pub const RECALL_PRECISION_METRIC: &str = "recall_precision_at_k";

/// Aggregate site label for the ranked fact-recall (`recall_facts_ranked`)
/// path.
pub const RECALL_PRECISION_SITE: &str = "recall_facts_ranked";

/// Precision\@k for a ranked recall result: of the top-`k` returned `facts`, the
/// fraction that are **query-relevant**.
///
/// This is a thin adapter (guideline G2): it maps Simard's [`CognitiveFact`] onto
/// the upstream primitive's decoupled `(concept, content)` pairs and delegates
/// the scoring to [`amplihack_memory::measurement::precision_at_k`]. The scoring
/// math — the keyword relevance proxy (a query token is a case-insensitive
/// substring of `concept`/`content`) and the top-`k` window — lives in
/// amplihack-memory-lib, not forked here, so the benchmark and live rails score
/// the same quantity with the same code.
///
/// Returns `None` (undefined, **not** `0.0`) when the query has no usable tokens
/// (empty / wildcard `*`) or the result set is empty, so callers skip emitting a
/// meaningless sample rather than dragging the mean toward zero. `k` is clamped
/// to the number of returned facts.
///
/// # Relevance oracle caveat (issue #4378)
///
/// This metric's relevance judgment (the substring proxy above) is
/// **deliberately DIFFERENT** from the relevance definition that gates the recall
/// path a user is actually served. Three definitions coexist across the cognition
/// stack, by design:
///
///   1. **Served recall gate — word-boundary.**
///      [`LibraryCognitiveMemory::search_facts`](crate::cognitive_memory::LibraryCognitiveMemory)
///      gates a clean query token at a WORD BOUNDARY (`fact_shares_query_relevance`
///      / `needle_matches_word`), so an interior hit (`act` in "re*act*or") is
///      NOT relevant.
///   2. **Ranked recall — ungated** ([`recall_facts_ranked`](crate::cognitive_memory::CognitiveMemoryOps::recall_facts_ranked)):
///      scores every live fact with NO keyword gate, so `precision_at_k < 1.0` is
///      a meaningful ranking-quality signal (gating it would destroy that infra).
///   3. **This metric — substring proxy** (upstream, kept unforked per G2).
///
/// The `recall_precision_at_k` self-metric therefore scores the ungated ranker
/// (#2) with the substring oracle (#3), which can count as relevant a fact the
/// served word-boundary gate (#1) would exclude — so the metric can read higher
/// than served precision. This is intentional (the divergence is pinned by
/// `cognitive_memory::tests_relevance_definition_divergence`; convergence is a
/// relevance-definition change routed to CONSENSUS_WORKFLOW). See
/// `docs/reference/recall-precision-hybrid-api.md` §"Relationship to the served
/// word-boundary gate and the ranker" for the full rationale and interpretation.
pub fn precision_at_k(query: &str, facts: &[CognitiveFact], k: usize) -> Option<f64> {
    // The only Simard-side glue is the CognitiveFact -> (concept, content)
    // mapping; it carries no scoring logic.
    let pairs: Vec<(&str, &str)> = facts
        .iter()
        .map(|f| (f.concept.as_str(), f.content.as_str()))
        .collect();
    amplihack_memory::measurement::precision_at_k(query, &pairs, k)
}

/// Running recall-precision aggregate per site: `(samples, sum_of_precision)`.
fn precision_agg() -> &'static Mutex<HashMap<String, (u64, f64)>> {
    static AGG: OnceLock<Mutex<HashMap<String, (u64, f64)>>> = OnceLock::new();
    AGG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fold one recall-precision observation into the running aggregate for `site`.
///
/// Cheap and lock-guarded; called on every ranked recall so no per-recall write
/// hits `metrics.jsonl`. The per-cycle sweep drains it via
/// [`flush_recall_precision_metric`] / [`drain_recall_precision`].
pub fn observe_recall_precision(site: &str, value: f64) {
    let mut map = precision_agg().lock().expect("metrics lock poisoned");
    let entry = map.entry(site.to_owned()).or_insert((0, 0.0));
    entry.0 += 1;
    entry.1 += value;
}

/// Snapshot (without draining) the running `(mean, samples)` for `site`, or
/// `None` when nothing has been observed since the last drain/reset. Test-only
/// peek accessor (the production path drains via [`drain_recall_precision`]).
#[cfg(test)]
pub fn recall_precision_mean(site: &str) -> Option<(f64, u64)> {
    let map = precision_agg().lock().expect("metrics lock poisoned");
    map.get(site)
        .and_then(|&(n, sum)| (n > 0).then_some((sum / n as f64, n)))
}

/// Drain the running aggregate for `site`, returning `(mean, samples)` and
/// resetting it. `None` when nothing was observed. The per-cycle metric sweep
/// calls this (via [`flush_recall_precision_metric`]) to emit one aggregated
/// sample and start the next cycle fresh.
pub fn drain_recall_precision(site: &str) -> Option<(f64, u64)> {
    let mut map = precision_agg().lock().expect("metrics lock poisoned");
    match map.remove(site) {
        Some((n, sum)) if n > 0 => Some((sum / n as f64, n)),
        _ => None,
    }
}

/// Drain the ranked-recall precision aggregate and, if any recall ran this
/// cycle, emit ONE aggregated [`RECALL_PRECISION_METRIC`] sample (the mean
/// precision\@k over the drain window, with the sample count in context) to
/// `metrics.jsonl`.
///
/// Called once per OODA cycle by the daemon metric sweep, **unconditionally**
/// (on both successful and errored cycles), so a cycle that recalled then
/// errored cannot bleed its observations into the next emission. The emitted
/// value is the mean over every ranked fact recall folded since the last drain
/// — across all in-process sources sharing the store (OODA preparation,
/// IPC-served recalls, consolidation) — so it is a windowed cross-source mean,
/// not a single-recall figure; `samples` conveys the volume. No-op when no
/// ranked recall ran this window, so the series carries signal only.
/// Best-effort: a metrics-write failure is logged, never propagated. Skipped
/// under `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl` — the aggregate is still drained so a
/// stale mean never bleeds across test cases.
pub fn flush_recall_precision_metric() {
    let Some((mean, samples)) = drain_recall_precision(RECALL_PRECISION_SITE) else {
        return;
    };
    if cfg!(test) {
        return;
    }
    let context = serde_json::json!({
        "site": RECALL_PRECISION_SITE,
        "samples": samples,
    })
    .to_string();
    if let Err(e) = crate::self_metrics::record_metric(RECALL_PRECISION_METRIC, mean, &context) {
        tracing::warn!(
            target: "simard::memory",
            error = %e,
            "failed to record recall_precision_at_k metric (recall unaffected)",
        );
    }
}

// ─────────────────────── graph-memory grounding coverage ────────────────────
//
// A graph-memory *health* signal: the fraction of semantic facts that are
// connected into the provenance graph (carry at least one `DERIVES_FROM` edge
// back to the episode they were distilled from). Provenance grounding is what
// turns the flat fact store into a traversable graph (see
// `docs/reference/cognitive-memory-provenance.md`) AND is the dominant term in
// the per-fact reliability gate (`crate::fact_reliability`), so coverage
// dropping means facts are entering semantic memory ungrounded — a weakening of
// graph memory that was previously visible only as raw OpenTelemetry edge-count
// gauges (`simard.memory.edges`), never as a durable, comparable, regressable
// `metrics.jsonl` series. Emitting the *ratio* here (not just the raw counts)
// makes a grounding regression raise the same gym-history signal every other
// self-metric does.

/// Durable self-metric name for graph-memory grounding coverage, emitted to
/// `metrics.jsonl` once per OODA cycle by [`record_provenance_coverage_metric`].
pub const FACT_PROVENANCE_COVERAGE_METRIC: &str = "fact_provenance_coverage";

/// Fraction of semantic facts that carry provenance (`facts_with_provenance /
/// facts_total`), in `[0.0, 1.0]`.
///
/// Returns `None` (undefined, **not** `0.0`) when there are no facts, so a store
/// with an empty semantic layer contributes no misleading `0.0` sample — the
/// same "skip rather than drag the series to zero" convention
/// [`precision_at_k`] uses for an undefined recall. `facts_with_provenance` is
/// clamped to `facts_total` defensively so a backend that over-counts can never
/// yield a ratio above `1.0`.
pub fn provenance_coverage(facts_with_provenance: u64, facts_total: u64) -> Option<f64> {
    if facts_total == 0 {
        return None;
    }
    let grounded = facts_with_provenance.min(facts_total);
    Some(grounded as f64 / facts_total as f64)
}

/// Emit ONE durable [`FACT_PROVENANCE_COVERAGE_METRIC`] sample (the grounding
/// ratio over the current `graph_stats()` snapshot) to `metrics.jsonl`.
///
/// Called once per OODA cycle by the daemon metric sweep from the same block
/// that already reads `graph_stats()` for the OpenTelemetry edge gauges, so it
/// adds no extra store read. A snapshot-shaped metric (store state, not a
/// per-cycle accumulator): the denominator is every semantic node the store
/// reports (live + archived/superseded revisions, matching `GraphStats`), so
/// the series tracks grounding across the whole semantic layer.
///
/// No-op when the store holds no facts (undefined coverage — see
/// [`provenance_coverage`]), so the series carries signal only. Best-effort: a
/// metrics-write failure is logged, never propagated. Skipped under
/// `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl`.
pub fn record_provenance_coverage_metric(facts_with_provenance: u64, facts_total: u64) {
    let Some(coverage) = provenance_coverage(facts_with_provenance, facts_total) else {
        return;
    };
    if cfg!(test) {
        return;
    }
    let context = serde_json::json!({
        "facts_total": facts_total,
        "facts_with_provenance": facts_with_provenance,
    })
    .to_string();
    if let Err(e) =
        crate::self_metrics::record_metric(FACT_PROVENANCE_COVERAGE_METRIC, coverage, &context)
    {
        tracing::warn!(
            target: "simard::memory",
            error = %e,
            "failed to record fact_provenance_coverage metric (memory unaffected)",
        );
    }
}

// ─────────────────────── graph-memory snapshot dedup hygiene ────────────────
//
// A graph-memory *hygiene* signal complementary to grounding coverage above.
// Snapshot facts (those stored under a stable caller/dedup key — goal-board
// snapshots and the like) are revisioned: each new revision SUPERSEDES the
// prior one, and `prune_superseded` (controlled forgetting) reclaims the
// archived revisions over time. `snapshot_facts_total` counts every snapshot
// revision the store still holds (live + not-yet-pruned superseded);
// `distinct_snapshot_caller_keys` counts the distinct logical streams behind
// them. Their ratio is the average *liveness* of the snapshot layer: 1.0 when
// every stream holds exactly one revision, falling toward 0 as superseded
// revisions accumulate faster than pruning reclaims them. That accumulation is
// exactly the monotonic-growth failure controlled forgetting exists to prevent,
// and — like grounding coverage — it was previously visible only as raw
// `graph_stats()` counts, never as a durable, comparable, regressable
// `metrics.jsonl` series. Emitting the *ratio* makes a pruning/hygiene
// regression raise the same gym-history signal every other self-metric does.

/// Durable self-metric name for snapshot-layer dedup hygiene, emitted to
/// `metrics.jsonl` once per OODA cycle by [`record_snapshot_dedup_ratio_metric`].
pub const FACT_SNAPSHOT_DEDUP_RATIO_METRIC: &str = "fact_snapshot_dedup_ratio";

/// Average liveness of the snapshot layer (`distinct_snapshot_caller_keys /
/// snapshot_facts_total`), in `(0.0, 1.0]`. Higher is healthier: `1.0` means
/// every snapshot stream holds a single live revision; a value approaching `0`
/// means superseded revisions have piled up (the inverse — total / distinct —
/// is the mean revisions retained per stream).
///
/// Returns `None` (undefined, **not** `0.0`) when the store holds no snapshot
/// facts, so a store with an empty snapshot layer contributes no misleading
/// `0.0` sample — the same "skip rather than drag the series to zero"
/// convention [`provenance_coverage`] and [`precision_at_k`] use.
/// `distinct_snapshot_caller_keys` is clamped to `snapshot_facts_total`
/// defensively (a stream always has ≥1 revision, so distinct ≤ total holds), so
/// a backend that miscounts can never yield a ratio above `1.0`.
pub fn snapshot_dedup_ratio(
    distinct_snapshot_caller_keys: u64,
    snapshot_facts_total: u64,
) -> Option<f64> {
    if snapshot_facts_total == 0 {
        return None;
    }
    let distinct = distinct_snapshot_caller_keys.min(snapshot_facts_total);
    Some(distinct as f64 / snapshot_facts_total as f64)
}

/// Emit ONE durable [`FACT_SNAPSHOT_DEDUP_RATIO_METRIC`] sample (the snapshot
/// liveness ratio over the current `graph_stats()` snapshot) to `metrics.jsonl`.
///
/// Called once per OODA cycle by the daemon metric sweep from the same block
/// that already reads `graph_stats()` for the OpenTelemetry edge gauges and
/// [`record_provenance_coverage_metric`], so it adds no extra store read. A
/// snapshot-shaped metric (store state, not a per-cycle accumulator).
///
/// No-op when the store holds no snapshot facts (undefined ratio — see
/// [`snapshot_dedup_ratio`]), so the series carries signal only. Best-effort: a
/// metrics-write failure is logged, never propagated. Skipped under
/// `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl`.
pub fn record_snapshot_dedup_ratio_metric(
    distinct_snapshot_caller_keys: u64,
    snapshot_facts_total: u64,
) {
    let Some(ratio) = snapshot_dedup_ratio(distinct_snapshot_caller_keys, snapshot_facts_total)
    else {
        return;
    };
    if cfg!(test) {
        return;
    }
    let context = serde_json::json!({
        "snapshot_facts_total": snapshot_facts_total,
        "distinct_snapshot_caller_keys": distinct_snapshot_caller_keys,
    })
    .to_string();
    if let Err(e) =
        crate::self_metrics::record_metric(FACT_SNAPSHOT_DEDUP_RATIO_METRIC, ratio, &context)
    {
        tracing::warn!(
            target: "simard::memory",
            error = %e,
            "failed to record fact_snapshot_dedup_ratio metric (memory unaffected)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(concept: &str, content: &str) -> CognitiveFact {
        CognitiveFact {
            node_id: format!("n-{concept}"),
            concept: concept.to_string(),
            content: content.to_string(),
            confidence: 1.0,
            source_id: "test".to_string(),
            tags: vec![],
            usage_count: 0,
            last_accessed_at: None,
        }
    }

    // ── precision_at_k: pure math ───────────────────────────────────────────

    #[test]
    fn precision_all_relevant_is_one() {
        let facts = [
            fact("kafka streaming", "backpressure"),
            fact("kafka broker", "partition rebalance"),
        ];
        assert_eq!(precision_at_k("kafka", &facts, 2), Some(1.0));
    }

    #[test]
    fn precision_half_relevant_in_window() {
        // Two relevant (kafka) ranked above two irrelevant → precision@2 == 1.0
        // but precision over the full window == 0.5.
        let facts = [
            fact("kafka streaming", "backpressure"),
            fact("kafka broker", "rebalance"),
            fact("postgres index", "btree bloat"),
            fact("redis cache", "eviction"),
        ];
        assert_eq!(precision_at_k("kafka", &facts, 2), Some(1.0));
        assert_eq!(precision_at_k("kafka", &facts, 4), Some(0.5));
    }

    #[test]
    fn precision_matches_on_content_not_only_concept() {
        let facts = [fact("infra note", "the kafka consumer lagged")];
        assert_eq!(precision_at_k("kafka", &facts, 1), Some(1.0));
    }

    #[test]
    fn precision_zero_when_top_k_all_irrelevant() {
        let facts = [fact("postgres", "vacuum"), fact("redis", "ttl")];
        assert_eq!(precision_at_k("kafka", &facts, 2), Some(0.0));
    }

    #[test]
    fn precision_clamps_k_to_result_len() {
        let facts = [fact("kafka", "lag")];
        // k larger than the result set clamps to the single (relevant) fact.
        assert_eq!(precision_at_k("kafka", &facts, 10), Some(1.0));
    }

    #[test]
    fn precision_is_none_for_empty_results() {
        let facts: [CognitiveFact; 0] = [];
        assert_eq!(precision_at_k("kafka", &facts, 5), None);
    }

    #[test]
    fn precision_is_none_for_wildcard_or_empty_query() {
        let facts = [fact("kafka", "lag")];
        assert_eq!(precision_at_k("*", &facts, 1), None);
        assert_eq!(precision_at_k("   ", &facts, 1), None);
        assert_eq!(precision_at_k("", &facts, 1), None);
    }

    #[test]
    fn precision_multi_token_query_is_case_insensitive() {
        let facts = [
            fact("Kafka Streaming", "Backpressure"),
            fact("unrelated", "topic"),
        ];
        // "streaming" (lowercased) matches fact 0's concept; fact 1 matches
        // neither token → precision@2 == 0.5.
        assert_eq!(precision_at_k("KAFKA streaming", &facts, 2), Some(0.5));
    }

    // ── running aggregate: unique per-test site keys keep it isolation-safe
    //    under parallel execution (the aggregate is process-global). ─────────

    #[test]
    fn aggregate_folds_running_mean() {
        let site = "test-agg-mean";
        assert_eq!(recall_precision_mean(site), None);
        observe_recall_precision(site, 1.0);
        observe_recall_precision(site, 0.0);
        observe_recall_precision(site, 0.5);
        assert_eq!(recall_precision_mean(site), Some((0.5, 3)));
    }

    #[test]
    fn drain_returns_mean_and_resets() {
        let site = "test-agg-drain";
        observe_recall_precision(site, 1.0);
        observe_recall_precision(site, 0.5);
        assert_eq!(drain_recall_precision(site), Some((0.75, 2)));
        // Drained → empty on the next read.
        assert_eq!(recall_precision_mean(site), None);
        assert_eq!(drain_recall_precision(site), None);
    }

    // ── graph-memory grounding coverage: pure math ──────────────────────────

    #[test]
    fn provenance_coverage_is_the_grounded_fraction() {
        assert_eq!(provenance_coverage(3, 4), Some(0.75));
        assert_eq!(provenance_coverage(0, 4), Some(0.0));
        assert_eq!(provenance_coverage(4, 4), Some(1.0));
    }

    #[test]
    fn provenance_coverage_is_none_for_an_empty_store() {
        // No facts → undefined coverage (skip, do NOT emit a misleading 0.0),
        // matching the precision@k "None rather than drag the series to zero"
        // convention. A grounded count with a zero denominator is still None.
        assert_eq!(provenance_coverage(0, 0), None);
        assert_eq!(provenance_coverage(5, 0), None);
    }

    #[test]
    fn provenance_coverage_clamps_overcount_to_one() {
        // A backend that reports more grounded facts than total must never
        // yield a ratio above 1.0.
        assert_eq!(provenance_coverage(7, 4), Some(1.0));
    }

    #[test]
    fn record_provenance_coverage_metric_is_a_no_op_under_test() {
        // Guards the operator's real metrics.jsonl: the emitter is cfg!(test)-
        // skipped, and an empty store is a no-op regardless. Neither call may
        // panic or touch global state.
        record_provenance_coverage_metric(3, 4);
        record_provenance_coverage_metric(0, 0);
    }

    // ── graph-memory snapshot dedup hygiene: pure math ──────────────────────

    #[test]
    fn snapshot_dedup_ratio_is_distinct_streams_over_total_revisions() {
        // Two streams, four revisions retained → each stream averages two
        // revisions → liveness 0.5. One-revision-per-stream is a healthy 1.0.
        assert_eq!(snapshot_dedup_ratio(2, 4), Some(0.5));
        assert_eq!(snapshot_dedup_ratio(4, 4), Some(1.0));
        assert_eq!(snapshot_dedup_ratio(1, 8), Some(0.125));
        // Snapshot facts present but none carry a grouping key → distinct 0 over
        // a nonzero total is a real, maximally-unhealthy 0.0 (emit it), NOT the
        // undefined None reserved for an empty snapshot layer.
        assert_eq!(snapshot_dedup_ratio(0, 4), Some(0.0));
    }

    #[test]
    fn snapshot_dedup_ratio_is_none_for_an_empty_snapshot_layer() {
        // No snapshot facts → undefined ratio (skip, do NOT emit a misleading
        // 0.0), matching the provenance_coverage / precision@k convention. A
        // nonzero distinct count with a zero denominator is still None.
        assert_eq!(snapshot_dedup_ratio(0, 0), None);
        assert_eq!(snapshot_dedup_ratio(3, 0), None);
    }

    #[test]
    fn snapshot_dedup_ratio_clamps_overcount_to_one() {
        // distinct ≤ total always holds (a stream has ≥1 revision); a backend
        // that miscounts must never yield a ratio above 1.0.
        assert_eq!(snapshot_dedup_ratio(9, 4), Some(1.0));
    }

    #[test]
    fn record_snapshot_dedup_ratio_metric_is_a_no_op_under_test() {
        // Guards the operator's real metrics.jsonl: the emitter is cfg!(test)-
        // skipped, and an empty snapshot layer is a no-op regardless. Neither
        // call may panic or touch global state.
        record_snapshot_dedup_ratio_metric(2, 4);
        record_snapshot_dedup_ratio_metric(0, 0);
    }
}
