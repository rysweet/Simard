//! Head-to-head baseline-vs-team duel (research doc Part 3.3 — the harness's
//! central question, made a first-class command).
//!
//! The Gym exists to answer one thing empirically: **does a multi-agent team
//! beat a single-model baseline on COIN target reachability?** `run` measures
//! one arm at a time; this module runs **both arms over the identical target
//! set** and decides a verdict on COIN's own metric ordering — **reach first,
//! precision second** (the published targeted track sorts by reach; precision is
//! the over-claim penalty). It composes the existing [`super::scorer`] output;
//! it never re-implements scoring.
//!
//! Like the rest of the Phase-4 scaffold this compares two **offline** runs
//! graded by the same mock oracle: it makes the baseline-vs-team *trade-off*
//! observable and reproducible, not a live-model capability result (Phase 3,
//! issue #2823). **LOCAL-ONLY**: nothing here is ever submitted externally.

use serde::Serialize;

use super::scorer::{Score, score_run};
use super::types::{RunReport, Strategy};

/// Float slack when comparing reach/precision percentages (avoid FP jitter),
/// matching the tolerance used elsewhere in the Gym.
const PCT_EPS: f64 = 1e-9;

/// Which arm won the duel, on COIN's metric ordering (reach, then precision).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DuelVerdict {
    /// The multi-agent team is the better arm.
    TeamWins,
    /// The single-model baseline is the better arm.
    BaselineWins,
    /// Neither arm is measurably better on this target set.
    Tie,
}

impl DuelVerdict {
    /// Uppercase label for CLI output (`TEAM WINS` / `BASELINE WINS` / `TIE`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TeamWins => "TEAM WINS",
            Self::BaselineWins => "BASELINE WINS",
            Self::Tie => "TIE",
        }
    }

    /// `true` when the multi-agent team is the better arm (the goal the harness
    /// is built to test for).
    #[must_use]
    pub fn team_wins(self) -> bool {
        matches!(self, Self::TeamWins)
    }
}

/// One arm's headline numbers, denormalised for rendering and serialisation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArmSummary {
    /// The strategy this arm ran.
    pub strategy: Strategy,
    /// The run id (so both persisted runs are traceable from the report).
    pub run_id: String,
    /// Reach percentage (reached / total).
    pub reach_pct: f64,
    /// Precision percentage (reached / submitted).
    pub precision_pct: f64,
    /// Targets reached.
    pub reached: usize,
    /// Total targets evaluated.
    pub total: usize,
    /// Inputs submitted (precision denominator).
    pub submitted: usize,
    /// Compact `R/W/A/T/N/E` histogram.
    pub histogram: String,
}

impl ArmSummary {
    fn build(report: &RunReport, score: &Score) -> Self {
        Self {
            strategy: report.strategy,
            run_id: report.run_id.clone(),
            reach_pct: score.overall.reach_pct(),
            precision_pct: score.overall.precision_pct(),
            reached: score.overall.reached,
            total: score.overall.total,
            submitted: score.overall.submitted,
            histogram: score.histogram.render(),
        }
    }
}

/// The full result of a baseline-vs-team duel over one target set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DuelReport {
    /// Model both arms ran.
    pub model: String,
    /// Snapshot the targets were drawn from.
    pub snapshot: String,
    /// Number of pinned targets each arm evaluated.
    pub targets: usize,
    /// The single-model baseline arm.
    pub baseline: ArmSummary,
    /// The multi-agent team arm.
    pub team: ArmSummary,
    /// team − baseline reach (percentage points).
    pub reach_delta_pct: f64,
    /// team − baseline precision (percentage points).
    pub precision_delta_pct: f64,
    /// Which arm won on COIN's metric ordering.
    pub verdict: DuelVerdict,
    /// Human-readable justification for the verdict.
    pub reason: String,
    /// `true` when both arms were graded by the offline mock oracle.
    pub offline_scaffold: bool,
}

/// Decide a duel between a `baseline` run and a `team` run evaluated over the
/// **same** target set.
///
/// Scores both runs, then applies COIN's targeted-track ordering: **higher reach
/// wins**; on a reach tie, **higher precision wins**; identical on both is a
/// [`Tie`]. A precision-only win at equal reach is a genuine result — the team
/// removed over-claims (wrong submissions) without giving up any reach.
///
/// The two reports must be the two strategies of the same model over the same
/// targets; the `duel` CLI guarantees that by producing them together.
///
/// [`Tie`]: DuelVerdict::Tie
#[must_use]
pub fn decide(baseline: &RunReport, team: &RunReport) -> DuelReport {
    let b_score = score_run(baseline);
    let t_score = score_run(team);

    let b_reach = b_score.overall.reach_pct();
    let t_reach = t_score.overall.reach_pct();
    let b_prec = b_score.overall.precision_pct();
    let t_prec = t_score.overall.precision_pct();
    let reach_delta = t_reach - b_reach;
    let precision_delta = t_prec - b_prec;

    let (verdict, reason) = if greater(t_reach, b_reach) {
        (
            DuelVerdict::TeamWins,
            format!(
                "team reached more targets ({t_reach:.1}% vs {b_reach:.1}%, \
                 {reach_delta:+.1} pts) — a strictly better result on COIN's primary \
                 reach metric"
            ),
        )
    } else if greater(b_reach, t_reach) {
        (
            DuelVerdict::BaselineWins,
            format!(
                "the single-model baseline reached more targets ({b_reach:.1}% vs \
                 {t_reach:.1}%) — the team's abstention gate cost reach on this set"
            ),
        )
    } else if greater(t_prec, b_prec) {
        (
            DuelVerdict::TeamWins,
            format!(
                "reach tied at {t_reach:.1}%, but the team's abstention gate lifted \
                 precision {precision_delta:+.1} pts ({t_prec:.1}% vs {b_prec:.1}%) — \
                 fewer over-claims for the same reach"
            ),
        )
    } else if greater(b_prec, t_prec) {
        (
            DuelVerdict::BaselineWins,
            format!(
                "reach tied at {t_reach:.1}%, but the baseline held higher precision \
                 ({b_prec:.1}% vs {t_prec:.1}%)"
            ),
        )
    } else {
        (
            DuelVerdict::Tie,
            format!(
                "both arms reached {t_reach:.1}% at {t_prec:.1}% precision — no \
                 measurable difference on this target set"
            ),
        )
    };

    DuelReport {
        model: baseline.model.clone(),
        snapshot: baseline.snapshot.clone(),
        targets: b_score.overall.total,
        baseline: ArmSummary::build(baseline, &b_score),
        team: ArmSummary::build(team, &t_score),
        reach_delta_pct: reach_delta,
        precision_delta_pct: precision_delta,
        verdict,
        reason,
        offline_scaffold: baseline.offline_scaffold || team.offline_scaffold,
    }
}

/// `a` is meaningfully greater than `b` (beyond [`PCT_EPS`]).
fn greater(a: f64, b: f64) -> bool {
    a - b > PCT_EPS
}
