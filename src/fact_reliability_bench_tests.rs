//! Tests for the reliability-gate BENCHMARK rail (perpetual-cognition goal:
//! reasoner reliability).
//!
//! The benchmark scores a **fixed, in-repo corpus** of labeled facts through the
//! same [`crate::fact_reliability::fact_passes_gate`] the live write boundaries
//! use, and persists one comparable [`crate::gym_history::ScoreRecord`] per run
//! so a regression in the reliability gate's discrimination is caught on a stable
//! benchmark (not only as a live per-pass block-rate). The record flows through
//! the **existing** gym signal machinery (`generate_signals`) unchanged.
//!
//! Reference: `docs/reference/reliability-gate-benchmark.md`

#[cfg(test)]
mod tests {
    use crate::fact_reliability_bench::{
        RELIABILITY_GATE_SCENARIO, RELIABILITY_GATE_SUITE, corpus_is_discriminating,
        reliability_gate_corpus_size, run_reliability_gate_bench, score_reliability_gate_corpus,
    };
    use crate::gym_history::{GymSignal, ScoreHistory, generate_signals};

    fn mem_history() -> ScoreHistory {
        ScoreHistory::open(":memory:").expect("open in-memory score history")
    }

    /// The fixed corpus produces a deterministic score — identical on every call
    /// — so the benchmark number is comparable run-over-run. This is the core
    /// property that makes it a *benchmark* rather than a live sample.
    #[test]
    fn corpus_score_is_deterministic() {
        let a = score_reliability_gate_corpus();
        let b = score_reliability_gate_corpus();
        assert_eq!(
            a, b,
            "fixed-corpus score must be deterministic across calls"
        );
    }

    /// Hollow-benchmark guard: the corpus must be discriminating — it contains
    /// both cases the gate must STORE and cases it must QUARANTINE. Because
    /// [`corpus_is_discriminating`] is `true` iff BOTH labels are present, this
    /// is exactly equivalent to "neither a degenerate always-store nor a
    /// degenerate always-quarantine classifier can reach accuracy `1.0`". Only
    /// then does accuracy `1.0` mean "the gate discriminates both directions",
    /// not "a constant classifier matched a single-label corpus".
    #[test]
    fn corpus_is_discriminating_across_both_dispositions() {
        assert!(
            corpus_is_discriminating(),
            "corpus must carry at least one store-expected AND one quarantine-expected case"
        );
        assert!(
            reliability_gate_corpus_size() >= 2,
            "a discriminating corpus needs >= 2 cases"
        );
    }

    /// Baseline: the current, correct gate classifies EVERY frozen case as its
    /// rubric prescribes, so accuracy is exactly `1.0`. Any future change that
    /// mis-scores a frozen case drops this below 1.0 — the regression the
    /// benchmark exists to catch.
    #[test]
    fn baseline_accuracy_is_one() {
        assert_eq!(
            score_reliability_gate_corpus(),
            1.0,
            "the current gate must correctly classify every frozen, rubric-labeled case"
        );
    }

    /// A run persists exactly the contract record: suite `cognition`, the
    /// reliability-gate scenario id, the deterministic corpus score, and the
    /// passed commit hash stamped for lineage.
    #[test]
    fn run_persists_contract_score_record() {
        let history = mem_history();
        let expected = score_reliability_gate_corpus();

        let rec = run_reliability_gate_bench(&history, Some("abc1234".to_string()))
            .expect("benchmark run records a score");

        assert_eq!(
            rec.suite_id, RELIABILITY_GATE_SUITE,
            "suite must be the cognition suite"
        );
        assert_eq!(rec.suite_id, "cognition");
        assert_eq!(
            rec.scenario_id, RELIABILITY_GATE_SCENARIO,
            "scenario id must be the reliability-gate accuracy join key"
        );
        assert_eq!(rec.scenario_id, "reliability_gate_accuracy");
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
            .latest(RELIABILITY_GATE_SUITE, RELIABILITY_GATE_SCENARIO)
            .expect("recorded score must be retrievable");
        assert_eq!(latest.score, expected);
    }

    /// The benchmark record flows through the **existing** gym signal machinery:
    /// two runs of the fixed corpus yield two identical scores, so
    /// `generate_signals(&history, "cognition")` emits a `Stable` signal for the
    /// reliability-gate scenario (delta 0, below the regression threshold). No
    /// bespoke signal path is forked — the benchmark reuses the same
    /// regression/promotion logic every other suite uses.
    #[test]
    fn run_feeds_existing_generate_signals() {
        let history = mem_history();
        let r1 = run_reliability_gate_bench(&history, None).unwrap();
        let r2 = run_reliability_gate_bench(&history, None).unwrap();
        assert_eq!(
            r1.score, r2.score,
            "the fixed corpus is deterministic, so both runs score identically"
        );

        let signals = generate_signals(&history, RELIABILITY_GATE_SUITE).expect("signals generate");
        let sig = signals
            .iter()
            .find(|s| s.scenario_id == RELIABILITY_GATE_SCENARIO)
            .expect("a signal must exist for the reliability_gate_accuracy scenario");
        assert_eq!(
            sig.signal,
            GymSignal::Stable,
            "two identical fixed-corpus scores are a Stable signal (delta below threshold)"
        );
    }
}
