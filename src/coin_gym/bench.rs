//! Consolidated single-model-baseline vs. multi-agent-team benchmark verdict.
//!
//! `coin-gym bench <model>` runs **both** arms of the harness over the *same*
//! pinned targets — the single-model `baseline` and the multi-agent `team` — and
//! emits one reproducible verdict answering the question the harness exists to
//! answer: **does the multi-agent team measurably beat the single-model baseline
//! on the LOCAL COIN target-reachability task?**
//!
//! This composes the existing per-arm [`Score`]s; it invents no new grading. The
//! verdict is a pure function of the two scores so it is unit-testable without
//! touching disk.
//!
//! **LOCAL-ONLY:** nothing here is ever submitted externally or entered on any
//! leaderboard, and (Phase 4) both arms grade against the offline mock oracle.

use serde::Serialize;

use super::scorer::Score;

/// Percentage tolerance below which two headline metrics are treated as equal
/// (guards floating-point noise from the reach/precision ratios).
const EPSILON_PCT: f64 = 1e-9;

/// Which arm the consolidated benchmark favours.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum BenchOutcome {
    /// The multi-agent team strictly improves at least one headline metric
    /// (reach or precision) and regresses neither.
    MultiagentWins,
    /// The arms tie on both headline metrics.
    Tie,
    /// The multi-agent team regresses at least one headline metric.
    Regression,
}

impl BenchOutcome {
    /// Short uppercase label for CLI output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            BenchOutcome::MultiagentWins => "MULTIAGENT WINS",
            BenchOutcome::Tie => "TIE",
            BenchOutcome::Regression => "REGRESSION",
        }
    }
}

/// A consolidated baseline-vs-team benchmark result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchVerdict {
    /// Model under test (both arms).
    pub model: String,
    /// Single-model baseline reach percentage.
    pub baseline_reach_pct: f64,
    /// Multi-agent team reach percentage.
    pub team_reach_pct: f64,
    /// team − baseline reach (percentage points).
    pub reach_delta_pct: f64,
    /// Single-model baseline precision percentage.
    pub baseline_precision_pct: f64,
    /// Multi-agent team precision percentage.
    pub team_precision_pct: f64,
    /// team − baseline precision (percentage points).
    pub precision_delta_pct: f64,
    /// The overall verdict.
    pub outcome: BenchOutcome,
    /// Whether either arm graded against the offline mock oracle.
    pub offline_scaffold: bool,
    /// Human-readable interpretation (always LOCAL-ONLY).
    pub note: String,
}

/// Build the consolidated verdict from the two arms' [`Score`]s.
///
/// The team **wins** when it strictly improves reach or precision without
/// regressing the other; it is a **regression** when either metric drops; a
/// **tie** otherwise. Reach and precision are the two COIN headline metrics —
/// precision (reached / *submitted*) is what exposes over-claiming, so an
/// abstention-gated team can climb precision at equal reach.
#[must_use]
pub fn bench_verdict(baseline: &Score, team: &Score) -> BenchVerdict {
    let baseline_reach = baseline.overall.reach_pct();
    let team_reach = team.overall.reach_pct();
    let reach_delta = team_reach - baseline_reach;
    let baseline_precision = baseline.overall.precision_pct();
    let team_precision = team.overall.precision_pct();
    let precision_delta = team_precision - baseline_precision;

    let regressed = reach_delta < -EPSILON_PCT || precision_delta < -EPSILON_PCT;
    let improved = reach_delta > EPSILON_PCT || precision_delta > EPSILON_PCT;
    let outcome = if regressed {
        BenchOutcome::Regression
    } else if improved {
        BenchOutcome::MultiagentWins
    } else {
        BenchOutcome::Tie
    };

    let verdict_sentence = match outcome {
        BenchOutcome::MultiagentWins => format!(
            "multi-agent team beats the single-model baseline \
             (reach {reach_delta:+.1} pts, precision {precision_delta:+.1} pts)"
        ),
        BenchOutcome::Tie => {
            "multi-agent team ties the single-model baseline on both headline metrics".to_string()
        }
        BenchOutcome::Regression => format!(
            "multi-agent team regresses vs the single-model baseline \
             (reach {reach_delta:+.1} pts, precision {precision_delta:+.1} pts)"
        ),
    };

    let offline_scaffold = baseline.offline_scaffold || team.offline_scaffold;
    let note = if offline_scaffold {
        format!(
            "LOCAL-ONLY offline scaffold (mock oracle, Phase 4): {verdict_sentence}; a real grade \
             delegates to `coin evaluate` on a Docker host (Phase 3, issue #2823) and is never \
             submitted externally"
        )
    } else {
        format!("LOCAL-ONLY: {verdict_sentence}")
    };

    BenchVerdict {
        model: baseline.model.clone(),
        baseline_reach_pct: baseline_reach,
        team_reach_pct: team_reach,
        reach_delta_pct: reach_delta,
        baseline_precision_pct: baseline_precision,
        team_precision_pct: team_precision,
        precision_delta_pct: precision_delta,
        outcome,
        offline_scaffold,
        note,
    }
}
