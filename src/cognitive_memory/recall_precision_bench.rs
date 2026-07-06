//! Fixed-corpus recall-precision BENCHMARK rail (issue #2491 / measurement
//! issue #2494, G1 hybrid measurement).
//!
//! The benchmark scores a small, hand-authored, **in-repo fixed corpus** through
//! the same precision@k primitive the live rail uses (via the
//! [`crate::cognitive_memory::metrics::precision_at_k`] adapter, which delegates
//! to `amplihack_memory::measurement`), and persists one comparable
//! [`ScoreRecord`] per run. Because the corpus is frozen and the scorer is
//! deterministic, the score is reproducible and comparable run-over-run — the
//! property that makes it a *benchmark* rather than a live sample.
//!
//! The recorded score flows through the **existing** gym signal machinery
//! ([`crate::gym_history::generate_signals`]) unchanged, so a benchmark
//! regression raises the same `Regression` signal every other gym scenario does.

use crate::cognitive_memory::metrics::{RECALL_PRECISION_METRIC, precision_at_k};
use crate::error::{SimardError, SimardResult};
use crate::gym_history::{ScoreHistory, ScoreRecord};
use crate::memory_cognitive::CognitiveFact;

/// Suite id for the cognition benchmark. A compile-time constant, never
/// request-derived, so no untrusted value ever reaches a SQL `WHERE` clause.
pub const RECALL_PRECISION_SUITE: &str = "cognition";

/// One fixed benchmark case: a query, a ranked set of facts, and the top-`k`
/// window to score.
struct Case {
    query: &'static str,
    facts: Vec<CognitiveFact>,
    k: usize,
}

/// Build a benchmark fact from a `(concept, content)` pair. Only `concept` and
/// `content` matter to precision@k; the other fields carry inert placeholders.
fn fact(concept: &str, content: &str) -> CognitiveFact {
    CognitiveFact {
        node_id: format!("bench-{concept}"),
        concept: concept.to_string(),
        content: content.to_string(),
        confidence: 1.0,
        source_id: "recall_precision_bench".to_string(),
        tags: Vec::new(),
        usage_count: 0,
        last_accessed_at: None,
    }
}

/// The frozen recall-precision corpus.
///
/// Six cases mixing fully-relevant top-k windows (precision 1.0) with windows
/// that float one off-topic fact into the top-k (precision 0.5), so the mean is
/// a non-trivial fraction in `(0.0, 1.0)` — a discriminating benchmark that can
/// move in either direction, not a trivial all-hit / all-miss one.
fn corpus() -> Vec<Case> {
    vec![
        // All top-k relevant → 1.0.
        Case {
            query: "kafka",
            facts: vec![
                fact("kafka streaming", "backpressure"),
                fact("kafka broker", "partition rebalance"),
            ],
            k: 2,
        },
        Case {
            query: "postgres index",
            facts: vec![
                fact("postgres index", "btree bloat"),
                fact("postgres vacuum", "autovacuum tuning"),
            ],
            k: 2,
        },
        Case {
            query: "rust ownership",
            facts: vec![
                fact("rust borrow checker", "ownership moves"),
                fact("rust lifetimes", "elision rules"),
            ],
            k: 2,
        },
        Case {
            query: "redis",
            facts: vec![
                fact("redis cache", "eviction policy"),
                fact("redis streams", "consumer groups"),
            ],
            k: 2,
        },
        // One off-topic fact in the top-k window → 0.5.
        Case {
            query: "graph traversal",
            facts: vec![
                fact("graph bfs", "queue frontier"),
                fact("postgres index", "btree bloat"),
            ],
            k: 2,
        },
        Case {
            query: "python asyncio",
            facts: vec![
                fact("python asyncio", "event loop"),
                fact("rust tokio", "reactor"),
            ],
            k: 2,
        },
    ]
}

/// The number of scored cases in the fixed corpus (surfaced as `samples` by the
/// operator command).
pub fn recall_precision_corpus_size() -> usize {
    corpus().iter().filter(|c| !c.facts.is_empty()).count()
}

/// Score the fixed recall-precision corpus: the deterministic mean precision@k
/// over every case (cases whose query/result set yield no defined precision are
/// skipped). The corpus is a non-empty in-repo constant, so the score is
/// reproducible and comparable across runs.
pub fn score_recall_precision_corpus() -> f64 {
    let mut sum = 0.0_f64;
    let mut scored = 0_u32;
    for case in corpus() {
        if let Some(p) = precision_at_k(case.query, &case.facts, case.k) {
            sum += p;
            scored += 1;
        }
    }
    if scored == 0 {
        0.0
    } else {
        sum / f64::from(scored)
    }
}

/// Run the benchmark and append one [`ScoreRecord`] to the shared gym history,
/// returning the recorded score. `commit_hash` stamps the record for lineage.
///
/// The record's `scenario_id` is [`RECALL_PRECISION_METRIC`] — the exact live
/// metric name — so the hybrid correlation can line the benchmark and live rails
/// up on one shared join key.
pub fn run_recall_precision_bench(
    history: &ScoreHistory,
    commit_hash: Option<String>,
) -> SimardResult<ScoreRecord> {
    let record = ScoreRecord {
        suite_id: RECALL_PRECISION_SUITE.to_string(),
        scenario_id: RECALL_PRECISION_METRIC.to_string(),
        score: score_recall_precision_corpus(),
        timestamp: chrono::Utc::now().timestamp(),
        commit_hash,
    };
    history
        .record(&record)
        .map_err(|e| SimardError::GymHistoryDb {
            action: "record_recall_precision".into(),
            reason: e.to_string(),
        })?;
    Ok(record)
}
