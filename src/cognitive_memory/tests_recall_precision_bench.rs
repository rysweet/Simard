//! Failing TDD tests for the recall-precision BENCHMARK rail (issue #2491 /
//! measurement issue #2494, G1 hybrid measurement, Step 7).
//!
//! The benchmark rail scores a **fixed, in-repo corpus** through the same
//! precision@k primitive the live rail uses, and persists one comparable
//! [`ScoreRecord`] per run so a claimed cognition improvement can be validated
//! on a stable benchmark (not only observed live). The record flows through the
//! **existing** gym signal machinery (`generate_signals`) unchanged.
//!
//! Reference: `docs/reference/recall-precision-hybrid-api.md#benchmark-rail`
//!
//! ```rust
//! // src/cognitive_memory/recall_precision_bench.rs
//! pub fn score_recall_precision_corpus() -> f64;
//! pub fn run_recall_precision_bench(
//!     history: &ScoreHistory,
//!     commit_hash: Option<String>,
//! ) -> SimardResult<ScoreRecord>;
//! ```
//!
//! The written record must be:
//! `ScoreRecord { suite_id: "cognition", scenario_id: "recall_precision_at_k",
//!  score: <mean precision@k over the fixed corpus>, timestamp: <unix secs>,
//!  commit_hash: <Some(hash)> }` where `scenario_id == RECALL_PRECISION_METRIC`,
//! the exact live metric name — that shared join key is what lets the hybrid
//! correlation line the two rails up.
//!
//! References the not-yet-created `recall_precision_bench` module on purpose —
//! the compile failure is the intended TDD red state.

#[cfg(test)]
mod tests {
    use crate::cognitive_memory::metrics::RECALL_PRECISION_METRIC;
    use crate::cognitive_memory::recall_precision_bench::{
        run_recall_precision_bench, score_recall_precision_corpus,
    };
    use crate::gym_history::{GymSignal, ScoreHistory, generate_signals};

    /// The benchmark's suite id (compile-time constant, never request-derived).
    const BENCH_SUITE: &str = "cognition";

    fn mem_history() -> ScoreHistory {
        ScoreHistory::open(":memory:").expect("open in-memory score history")
    }

    /// The fixed corpus produces a deterministic score — identical on every
    /// call — so the benchmark number is comparable run-over-run. This is the
    /// core property that makes it a *benchmark* rather than a live sample.
    #[test]
    fn corpus_score_is_deterministic() {
        let a = score_recall_precision_corpus();
        let b = score_recall_precision_corpus();
        assert_eq!(
            a, b,
            "fixed-corpus score must be deterministic across calls"
        );
    }

    /// Hollow-benchmark guard: the corpus must be discriminating — it contains
    /// both relevant and irrelevant top-k facts, so the mean precision@k lands
    /// strictly between 0.0 and 1.0. A score of exactly 0.0 or 1.0 would mean a
    /// trivial (all-miss / all-hit) corpus that can never move, i.e. a
    /// meaningless benchmark.
    #[test]
    fn corpus_score_is_a_meaningful_fraction() {
        let s = score_recall_precision_corpus();
        assert!(
            s > 0.0 && s < 1.0,
            "corpus score must be a non-trivial fraction in (0.0, 1.0), got {s}"
        );
    }

    /// A run persists exactly the contract record: suite `cognition`, scenario
    /// == the live metric name, score == the fixed-corpus score, and the passed
    /// commit hash stamped for lineage.
    #[test]
    fn run_persists_contract_score_record() {
        let history = mem_history();
        let expected = score_recall_precision_corpus();

        let rec = run_recall_precision_bench(&history, Some("abc1234".to_string()))
            .expect("benchmark run records a score");

        assert_eq!(
            rec.suite_id, BENCH_SUITE,
            "suite must be the cognition suite"
        );
        assert_eq!(
            rec.scenario_id, RECALL_PRECISION_METRIC,
            "scenario id must equal the live metric name (the shared join key)"
        );
        assert_eq!(rec.scenario_id, "recall_precision_at_k");
        assert_eq!(
            rec.score, expected,
            "recorded score must be the deterministic fixed-corpus score"
        );
        assert_eq!(
            rec.commit_hash.as_deref(),
            Some("abc1234"),
            "commit hash must be stamped for lineage"
        );
        assert!(
            rec.timestamp > 0,
            "timestamp must be a real unix epoch value"
        );

        // Persisted and reads back through the standard history accessor.
        let latest = history
            .latest(BENCH_SUITE, RECALL_PRECISION_METRIC)
            .expect("recorded score must be retrievable");
        assert_eq!(latest.score, expected);
    }

    /// The benchmark record flows through the **existing** gym signal machinery:
    /// two runs of the fixed corpus yield two identical scores, so
    /// `generate_signals(&history, "cognition")` emits a `Stable` signal for the
    /// `recall_precision_at_k` scenario (delta 0, below the 0.01 threshold). The
    /// point is that no bespoke signal path is forked — the benchmark reuses the
    /// same regression/promotion logic every other suite uses.
    #[test]
    fn run_feeds_existing_generate_signals() {
        let history = mem_history();
        let r1 = run_recall_precision_bench(&history, None).unwrap();
        let r2 = run_recall_precision_bench(&history, None).unwrap();
        assert_eq!(
            r1.score, r2.score,
            "the fixed corpus is deterministic, so both runs score identically"
        );

        let signals = generate_signals(&history, BENCH_SUITE).expect("signals generate");
        let sig = signals
            .iter()
            .find(|s| s.scenario_id == RECALL_PRECISION_METRIC)
            .expect("a signal must exist for the recall_precision_at_k scenario");
        assert_eq!(
            sig.signal,
            GymSignal::Stable,
            "two identical fixed-corpus scores are a Stable signal (delta below threshold)"
        );
    }
}
