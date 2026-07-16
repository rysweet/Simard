//! Head-to-head **baseline (single-model) vs. team (multi-agent)** matchup.
//!
//! This answers the COIN Gym's central question directly: do multi-agent
//! patterns beat single-model execution on the LOCAL leaderboard? The
//! [`decide_matchup`] function compares two [`Score`]s produced over the **same**
//! pinned targets and yields a [`StrategyMatchup`] carrying the reach/precision
//! deltas and a [`MatchupVerdict`].
//!
//! **Metric priority** mirrors COIN's targeted track: *reach rate* is the
//! headline capability metric, so it decides the winner first; *precision*
//! (which penalises over-claiming) breaks reach ties. A small epsilon keeps the
//! floating-point comparison robust.
//!
//! LOCAL-ONLY: this is a local comparison only; nothing is ever submitted
//! externally or posted to any leaderboard.

use serde::Serialize;

use super::scorer::Score;

/// Epsilon below which two percentage figures are treated as equal.
const EPS: f64 = 1e-9;

/// Which strategy won the head-to-head.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum MatchupVerdict {
    /// The multi-agent team strictly beat the single-model baseline.
    TeamWins,
    /// The single-model baseline strictly beat the multi-agent team.
    BaselineWins,
    /// Neither strategy dominated on reach or precision.
    Tie,
}

impl MatchupVerdict {
    /// Stable, uppercase label for CLI/report rendering.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TeamWins => "TEAM-WINS",
            Self::BaselineWins => "BASELINE-WINS",
            Self::Tie => "TIE",
        }
    }
}

/// Head-to-head result of a baseline-vs-team matchup over identical targets.
///
/// Deltas are `team − baseline` in percentage points, so a positive delta means
/// the multi-agent team improved on the single-model baseline.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StrategyMatchup {
    /// The model both strategies drove.
    pub model: String,
    /// Persisted run-id of the single-model baseline run.
    pub baseline_run_id: String,
    /// Persisted run-id of the multi-agent team run.
    pub team_run_id: String,
    /// Baseline overall reach percentage.
    pub baseline_reach_pct: f64,
    /// Team overall reach percentage.
    pub team_reach_pct: f64,
    /// `team − baseline` reach delta in percentage points.
    pub reach_delta_pp: f64,
    /// Baseline overall precision percentage.
    pub baseline_precision_pct: f64,
    /// Team overall precision percentage.
    pub team_precision_pct: f64,
    /// `team − baseline` precision delta in percentage points.
    pub precision_delta_pp: f64,
    /// Number of pinned targets both strategies were scored against.
    pub targets: usize,
    /// The head-to-head verdict.
    pub verdict: MatchupVerdict,
    /// Whether either run graded against the offline mock oracle.
    pub offline_scaffold: bool,
}

/// Decide a matchup from a baseline score and a team score computed over the
/// **same** target set.
///
/// Reach rate is the primary COIN metric and decides the winner; precision
/// breaks reach ties (it penalises over-claiming). Both are compared with an
/// epsilon so floating-point equality is robust.
#[must_use]
pub fn decide_matchup(baseline: &Score, team: &Score) -> StrategyMatchup {
    let baseline_reach_pct = baseline.overall.reach_pct();
    let team_reach_pct = team.overall.reach_pct();
    let baseline_precision_pct = baseline.overall.precision_pct();
    let team_precision_pct = team.overall.precision_pct();
    let reach_delta_pp = team_reach_pct - baseline_reach_pct;
    let precision_delta_pp = team_precision_pct - baseline_precision_pct;

    let verdict = if reach_delta_pp > EPS {
        MatchupVerdict::TeamWins
    } else if reach_delta_pp < -EPS {
        MatchupVerdict::BaselineWins
    } else if precision_delta_pp > EPS {
        MatchupVerdict::TeamWins
    } else if precision_delta_pp < -EPS {
        MatchupVerdict::BaselineWins
    } else {
        MatchupVerdict::Tie
    };

    StrategyMatchup {
        model: team.model.clone(),
        baseline_run_id: baseline.run_id.clone(),
        team_run_id: team.run_id.clone(),
        baseline_reach_pct,
        team_reach_pct,
        reach_delta_pp,
        baseline_precision_pct,
        team_precision_pct,
        precision_delta_pp,
        targets: team.overall.total,
        verdict,
        offline_scaffold: baseline.offline_scaffold || team.offline_scaffold,
    }
}
