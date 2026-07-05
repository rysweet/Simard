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
// (`recall_facts_ranked`). `precision_at_k` is the pure, deterministic spine;
// the aggregate below folds one observation per recall so the per-cycle sweep
// can emit a single durable sample instead of writing `metrics.jsonl` on the
// hot recall path.

/// Durable self-metric name for ranked-recall precision, emitted to
/// `metrics.jsonl` once per OODA cycle by [`flush_recall_precision_metric`].
pub const RECALL_PRECISION_METRIC: &str = "recall_precision_at_k";

/// Aggregate site label for the ranked fact-recall (`recall_facts_ranked`)
/// path.
pub const RECALL_PRECISION_SITE: &str = "recall_facts_ranked";

/// Tokenize a recall query the same way the keyword recall gate does:
/// whitespace-split, lowercased, dropping punctuation-only tokens (e.g. the
/// wildcard `*`) so a wildcard/empty query yields no tokens and is treated as
/// "no measurable relevance target".
fn query_tokens(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect()
}

/// Whether a fact is query-relevant under the keyword proxy: its lowercased
/// `concept` or `content` contains at least one query token (substring match,
/// mirroring the episodic recall keyword gate).
fn fact_is_relevant(fact: &CognitiveFact, tokens: &[String]) -> bool {
    let concept = fact.concept.to_lowercase();
    let content = fact.content.to_lowercase();
    tokens
        .iter()
        .any(|t| concept.contains(t.as_str()) || content.contains(t.as_str()))
}

/// Precision\@k for a ranked recall result: of the top-`k` returned `facts`, the
/// fraction that are **query-relevant**.
///
/// Relevance is a coarse keyword proxy (see `fact_is_relevant`): a fact counts
/// when its `concept`/`content` contains a query token as a substring — the same
/// `.contains` keyword gate the episodic recall path uses, NOT the ranker's
/// exact token/Jaccard `text_relevance` score. It is deliberately broader than
/// the ranker (e.g. `cat` matches `concatenate`), which is acceptable for a
/// self-metric baseline: it needs no external ground-truth labels — the query
/// itself is the relevance oracle — and it moves in the same direction as
/// ranking quality. This makes the metric a self-contained, deterministic
/// measure of *ranking precision*: if the ranker floats an off-topic fact into
/// the top-`k`, precision falls.
///
/// Returns `None` (undefined, **not** `0.0`) when the query has no usable
/// tokens (empty / wildcard `*`) or the result set is empty, so callers skip
/// emitting a meaningless sample rather than dragging the mean toward zero.
/// `k` is clamped to the number of returned facts.
pub fn precision_at_k(query: &str, facts: &[CognitiveFact], k: usize) -> Option<f64> {
    let tokens = query_tokens(query);
    if tokens.is_empty() {
        return None;
    }
    let window = k.min(facts.len());
    if window == 0 {
        return None;
    }
    let relevant = facts[..window]
        .iter()
        .filter(|f| fact_is_relevant(f, &tokens))
        .count();
    Some(relevant as f64 / window as f64)
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
}
