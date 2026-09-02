//! Fixed-corpus reliability-gate BENCHMARK rail (perpetual-cognition goal:
//! reasoner reliability).
//!
//! The distilled-fact reliability gate ([`crate::fact_reliability::fact_passes_gate`]
//! / [`crate::fact_reliability::score_fact_reliability`]) is the store-vs-quarantine
//! decision every distilled fact passes through on BOTH write boundaries (the
//! in-process sink and the IPC `StoreFactGated` handler). Its discrimination —
//! promoting grounded, informative facts while quarantining ungrounded or
//! no-information ones — is the reasoner-reliability axis of Simard's cognition.
//!
//! Until now that gate had two observability surfaces:
//!   * per-decision **unit tests** (binary pass/fail in `cargo test`), and
//!   * a live per-pass **`distill_reliability_gate`** metric (the pass block-rate).
//!
//! What it lacked — and what the recall-quality axis already has via
//! [`crate::cognitive_memory::recall_precision_bench`] — is a **fixed-corpus,
//! run-over-run comparable benchmark wired into the gym signal machinery**. So a
//! silent regression in the gate's *discrimination* (a weight/threshold change,
//! or a change to the informative-word content proxy) raised no gym `Regression`
//! signal the way a recall-precision regression does. This module closes that
//! gap for the reliability axis, mirroring the recall-precision benchmark rail:
//!
//! The benchmark scores a small, hand-authored, **in-repo fixed corpus** of
//! labeled facts through the **same** [`crate::fact_reliability::fact_passes_gate`]
//! the live write boundaries use, and persists one comparable [`ScoreRecord`] per
//! run. Because the corpus is frozen and the gate is a pure function, the score
//! is reproducible and comparable run-over-run — the property that makes it a
//! *benchmark* rather than a live sample.
//!
//! The score is the gate's **classification accuracy**: the fraction of the
//! frozen, rubric-labeled cases the gate classifies as its documented rubric
//! prescribes. It is `1.0` by construction on a correct gate — the corpus encodes
//! the [`crate::fact_reliability::score_fact_reliability`] rubric — so its purpose
//! is regression detection: any future change that mis-scores a frozen case drops
//! the accuracy, and the **existing** gym signal machinery
//! ([`crate::gym_history::generate_signals`]) raises the same `Regression` signal
//! every other gym scenario does (a score DROP beyond the regression threshold).
//!
//! Unlike recall precision — whose benchmark deliberately reuses the exact live
//! metric name as a shared join key so a hybrid correlation can line the
//! benchmark and live rails up — this benchmark measures gate *accuracy*, a
//! quantity with **no live twin** (the live `distill_reliability_gate` metric is
//! a block-*rate*, a different quantity). It therefore carries its own scenario
//! id, [`RELIABILITY_GATE_SCENARIO`], and is a benchmark-only signal.

use crate::error::{SimardError, SimardResult};
use crate::fact_reliability::fact_passes_gate;
use crate::gym_history::{ScoreHistory, ScoreRecord};

/// Suite id for the cognition benchmark family. A compile-time constant, never
/// request-derived, so no untrusted value ever reaches a SQL `WHERE` clause. The
/// same suite the recall-precision benchmark uses, so both cognition benchmarks
/// live under one gym suite.
pub const RELIABILITY_GATE_SUITE: &str = "cognition";

/// Scenario id for the reliability-gate accuracy benchmark. A benchmark-only
/// quantity (gate classification accuracy) with no live twin metric, so — unlike
/// recall precision — it does NOT reuse a live metric name; it carries its own
/// stable join key.
pub const RELIABILITY_GATE_SCENARIO: &str = "reliability_gate_accuracy";

/// One fixed benchmark case: a fact presented to the reliability gate together
/// with the disposition the gate's documented rubric prescribes.
struct Case {
    /// Concept label (a [`crate::fact_reliability::KNOWN_CONCEPTS`] member earns
    /// the concept-validity nudge; anything else does not).
    concept: &'static str,
    /// Fact body — scored by the informative-word content proxy.
    content: &'static str,
    /// Whether the fact's cited provenance resolved (batch-membership for the
    /// in-process sink, store-existence for the IPC handler). The dominant,
    /// *necessary* signal: grounding (0.5) alone reaches the threshold.
    grounded: bool,
    /// The disposition the rubric prescribes: `true` == the gate MUST store
    /// (score `>= RELIABILITY_THRESHOLD`), `false` == the gate MUST quarantine.
    expected_store: bool,
}

/// The frozen reliability-gate corpus.
///
/// A discriminating mix (see [`corpus_is_discriminating`]) of cases the gate must
/// STORE and cases it must QUARANTINE, so accuracy `1.0` requires the gate to
/// classify BOTH directions correctly — a degenerate always-store or
/// always-quarantine classifier cannot reach `1.0`. Each case's expected
/// disposition is derived directly from the
/// [`crate::fact_reliability::score_fact_reliability`] rubric
/// (grounding 0.5 + content ≤0.3 + concept 0.1, threshold 0.5):
fn corpus() -> Vec<Case> {
    vec![
        // ── Must STORE: grounding is present and content carries information ──
        // Grounded + ≥3 informative words + known concept → 0.9.
        Case {
            concept: "bug-pattern",
            content: "flaky test caused by shared global state across cases",
            grounded: true,
            expected_store: true,
        },
        // Grounded + ≥3 informative words + unknown concept → 0.8.
        Case {
            concept: "kafka backpressure",
            content: "consumer lag grows when partitions rebalance under load",
            grounded: true,
            expected_store: true,
        },
        // Grounded + known concept + only 1–2 informative words → 0.5+0.15+0.1 = 0.75.
        Case {
            concept: "lesson-learned",
            content: "idempotency matters",
            grounded: true,
            expected_store: true,
        },
        // Grounded + unknown concept + exactly 2 informative words → 0.5+0.15 = 0.65.
        Case {
            concept: "postgres vacuum",
            content: "autovacuum tuning",
            grounded: true,
            expected_store: true,
        },
        // ── Must QUARANTINE: ungrounded (unverifiable provenance) ────────────
        // Ungrounded + ≥3 words + known concept tops out at content+concept = 0.4.
        Case {
            concept: "pr-pattern",
            content: "small focused diffs review faster than sprawling ones",
            grounded: false,
            expected_store: false,
        },
        // Ungrounded + ≥3 words + unknown concept → 0.3.
        Case {
            concept: "redis eviction",
            content: "lru policy evicts the coldest keys first",
            grounded: false,
            expected_store: false,
        },
        // ── Must QUARANTINE: no-information content (hard gate → 0.0) ────────
        // Grounded but punctuation/symbol-only content carries no information.
        Case {
            concept: "bug-pattern",
            content: "... ... ...",
            grounded: true,
            expected_store: false,
        },
        // Grounded but empty/whitespace-only content.
        Case {
            concept: "lesson-learned",
            content: "   ",
            grounded: true,
            expected_store: false,
        },
    ]
}

/// The number of cases in the fixed corpus (surfaced as `samples` by the operator
/// command).
pub fn reliability_gate_corpus_size() -> usize {
    corpus().len()
}

/// `true` iff the frozen corpus is discriminating: it contains at least one case
/// the gate must STORE **and** at least one it must QUARANTINE.
///
/// This is the hollow-benchmark guard for a classifier accuracy score: only a
/// corpus with both labels forces accuracy `1.0` to mean "the gate discriminates
/// both directions correctly" rather than "a constant classifier happened to
/// match a single-label corpus".
pub fn corpus_is_discriminating() -> bool {
    let cases = corpus();
    cases.iter().any(|c| c.expected_store) && cases.iter().any(|c| !c.expected_store)
}

/// Score the fixed reliability-gate corpus: the deterministic fraction of cases
/// the gate ([`fact_passes_gate`]) classifies as the rubric prescribes.
///
/// `1.0` on a correct gate (the corpus encodes the rubric); a regression that
/// mis-scores any frozen case drops the fraction. The corpus is a non-empty
/// in-repo constant, so the score is reproducible and comparable across runs.
pub fn score_reliability_gate_corpus() -> f64 {
    let cases = corpus();
    if cases.is_empty() {
        return 0.0;
    }
    let correct = cases
        .iter()
        .filter(|c| fact_passes_gate(c.concept, c.content, c.grounded) == c.expected_store)
        .count();
    correct as f64 / cases.len() as f64
}

/// Run the benchmark and append one [`ScoreRecord`] to the shared gym history,
/// returning the recorded score. `commit_hash` stamps the record for lineage.
///
/// The record's `suite_id` / `scenario_id` are [`RELIABILITY_GATE_SUITE`] /
/// [`RELIABILITY_GATE_SCENARIO`], so the score flows through the **existing** gym
/// signal machinery ([`crate::gym_history::generate_signals`]) unchanged — a
/// benchmark regression raises the same `Regression` signal every other gym
/// scenario does.
pub fn run_reliability_gate_bench(
    history: &ScoreHistory,
    commit_hash: Option<String>,
) -> SimardResult<ScoreRecord> {
    let record = ScoreRecord {
        suite_id: RELIABILITY_GATE_SUITE.to_string(),
        scenario_id: RELIABILITY_GATE_SCENARIO.to_string(),
        score: score_reliability_gate_corpus(),
        timestamp: chrono::Utc::now().timestamp(),
        commit_hash,
    };
    history
        .record(&record)
        .map_err(|e| SimardError::GymHistoryDb {
            action: "record_reliability_gate".into(),
            reason: e.to_string(),
        })?;
    Ok(record)
}
