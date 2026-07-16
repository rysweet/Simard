//! Baseline-vs-team A/B comparator (research doc Part 3.3, the central claim).
//!
//! The COIN Gym exists to answer one question: **does a multi-agent team beat a
//! single-model baseline on target-reachability?** [`super::run`] scores a single
//! arm and [`super::leaderboard`] diffs one arm against the published board, but
//! neither answers the head-to-head directly — an operator has to run both arms
//! and eyeball the deltas (see
//! `docs/research/coin-gym-baseline-vs-team-measurement.md`).
//!
//! This module makes that head-to-head a **first-class, single-call** result:
//! run the `baseline` and `team` strategies over the **same** target set, then
//! report each arm's reach/precision and the arm-to-arm deltas with a
//! Pareto-domination [`StrategyVerdict`]. It is the machine-readable form of the
//! measurement doc's reference table.
//!
//! **LOCAL-ONLY / offline scaffold.** Like the rest of the Gym (Phase 4) this
//! runs against a mock oracle so it is exercised in CI without a VM; the deltas
//! are a deterministic property of the harness's abstention design, not a live
//! model on a real COIN snapshot (that is Phase 3, issue #2823). Nothing here is
//! ever submitted externally or entered on any leaderboard.

use std::cmp::Ordering;

use serde::Serialize;

use super::execute_run;
use super::scorer::{Score, score_run};
use super::target_loader::DemoScenario;
use super::types::{CoinGymResult, RunReport, Strategy};

/// Slack when comparing reach/precision percentages (avoid FP jitter).
const DELTA_EPS: f64 = 1e-9;

/// Head-to-head verdict for the multi-agent team against the single-model
/// baseline, by **Pareto domination** over (reach, precision).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrategyVerdict {
    /// The team is at least as good on both metrics and strictly better on at
    /// least one — the multi-agent arm dominates.
    TeamWins,
    /// The baseline is at least as good on both metrics and strictly better on
    /// at least one — the single-model arm dominates.
    BaselineWins,
    /// The arms are equal on both metrics.
    Tie,
    /// A genuine trade-off: each arm is better on one metric and worse on the
    /// other, so neither dominates.
    Mixed,
}

impl StrategyVerdict {
    /// Uppercase label for CLI rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TeamWins => "TEAM-WINS",
            Self::BaselineWins => "BASELINE-WINS",
            Self::Tie => "TIE",
            Self::Mixed => "MIXED",
        }
    }

    /// Classify a verdict from the two arms' reach/precision deltas
    /// (team − baseline, in percentage points).
    #[must_use]
    pub fn classify(reach_delta_pct: f64, precision_delta_pct: f64) -> Self {
        let reach = sign(reach_delta_pct);
        let precision = sign(precision_delta_pct);
        match (reach, precision) {
            (Ordering::Equal, Ordering::Equal) => Self::Tie,
            // Team at least ties both and strictly wins at least one.
            (Ordering::Greater | Ordering::Equal, Ordering::Greater | Ordering::Equal) => {
                Self::TeamWins
            }
            // Baseline at least ties both and strictly wins at least one.
            (Ordering::Less | Ordering::Equal, Ordering::Less | Ordering::Equal) => {
                Self::BaselineWins
            }
            // One metric up, the other down — a real trade-off.
            _ => Self::Mixed,
        }
    }
}

/// Sign of a delta with a small dead-band so FP jitter counts as "equal".
fn sign(delta: f64) -> Ordering {
    if delta > DELTA_EPS {
        Ordering::Greater
    } else if delta < -DELTA_EPS {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// A head-to-head comparison of the single-model baseline against the
/// multi-agent team over one shared target set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StrategyComparison {
    /// Model under test (identical for both arms).
    pub model: String,
    /// Snapshot the shared targets were drawn from.
    pub snapshot: String,
    /// The single-model baseline arm's score.
    pub baseline: Score,
    /// The multi-agent team arm's score.
    pub team: Score,
    /// team − baseline overall reach (percentage points).
    pub reach_delta_pct: f64,
    /// team − baseline overall precision (percentage points).
    pub precision_delta_pct: f64,
    /// Pareto-domination verdict over (reach, precision).
    pub verdict: StrategyVerdict,
    /// `true` when both arms came from the offline mock oracle.
    pub offline_scaffold: bool,
    /// Human-readable interpretation of the verdict and its caveats.
    pub note: String,
}

impl StrategyComparison {
    /// Build a comparison from two already-graded arms.
    ///
    /// Both reports must be for the same `model`; the baseline report must be the
    /// `baseline` strategy and the team report the `team` strategy (the caller —
    /// [`compare_strategies`] and the `ab` CLI — guarantees this).
    #[must_use]
    pub fn from_reports(baseline: &RunReport, team: &RunReport) -> Self {
        let bscore = score_run(baseline);
        let tscore = score_run(team);
        let reach_delta_pct = tscore.overall.reach_pct() - bscore.overall.reach_pct();
        let precision_delta_pct = tscore.overall.precision_pct() - bscore.overall.precision_pct();
        let verdict = StrategyVerdict::classify(reach_delta_pct, precision_delta_pct);
        let offline_scaffold = baseline.offline_scaffold || team.offline_scaffold;
        let note = build_note(
            verdict,
            reach_delta_pct,
            precision_delta_pct,
            offline_scaffold,
        );
        Self {
            model: baseline.model.clone(),
            snapshot: baseline.snapshot.clone(),
            baseline: bscore,
            team: tscore,
            reach_delta_pct,
            precision_delta_pct,
            verdict,
            offline_scaffold,
            note,
        }
    }
}

/// Run both arms — single-model `baseline` and multi-agent `team` — over the
/// **same** `scenario` and return their head-to-head comparison.
///
/// # Errors
/// Propagates any executor/parse error from grading either arm.
pub fn compare_strategies(
    model: &str,
    scenario: &DemoScenario,
) -> CoinGymResult<StrategyComparison> {
    let baseline = execute_run(model, Strategy::Baseline, scenario)?;
    let team = execute_run(model, Strategy::Team, scenario)?;
    Ok(StrategyComparison::from_reports(&baseline, &team))
}

fn build_note(
    verdict: StrategyVerdict,
    reach_delta_pct: f64,
    precision_delta_pct: f64,
    offline_scaffold: bool,
) -> String {
    let head = match verdict {
        StrategyVerdict::TeamWins => format!(
            "multi-agent team dominates the single-model baseline \
             (reach {reach_delta_pct:+.1} pts, precision {precision_delta_pct:+.1} pts)"
        ),
        StrategyVerdict::BaselineWins => format!(
            "single-model baseline dominates the multi-agent team \
             (reach {reach_delta_pct:+.1} pts, precision {precision_delta_pct:+.1} pts)"
        ),
        StrategyVerdict::Tie => {
            "arms tie on both reach and precision on this target set".to_string()
        }
        StrategyVerdict::Mixed => format!(
            "trade-off: neither arm dominates \
             (reach {reach_delta_pct:+.1} pts, precision {precision_delta_pct:+.1} pts)"
        ),
    };
    if offline_scaffold {
        format!(
            "{head} — OFFLINE SCAFFOLD (mock oracle): a deterministic property of \
             the abstention design, not a live-model grade on a real COIN snapshot \
             (Phase 3, issue #2823). LOCAL-ONLY."
        )
    } else {
        head
    }
}
