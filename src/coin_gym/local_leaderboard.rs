//! LOCAL leaderboard — the done-gate view (research doc Part 3.3, done-gate).
//!
//! Ranks the harness's **own** saved runs against each other so an operator can
//! see whether the multi-agent `team` arm has *climbed above* the single-model
//! `baseline` arm — the LOCAL comparison the whole COIN Gym exists to make. This
//! is distinct from [`super::leaderboard`], which diffs **one** run against
//! COIN's *published* board: that answers "is our harness calibrated?"; this
//! answers "does multiagent beat single-model *here*?".
//!
//! Runs are ranked by **reach** (COIN's headline metric) then **precision** (its
//! over-claim tiebreak), so the arm that reaches more — or reaches the same and
//! over-claims less — ranks higher. The [`BaselineVsTeam`] summary then compares
//! the best run of each arm and states plainly whether the team beats the
//! baseline.
//!
//! **LOCAL-ONLY, offline-scaffold.** Every input here is a locally saved run;
//! nothing is fetched or submitted externally. When any ranked run is an offline
//! mock-oracle run (Phase 4), the verdict is a **control-flow / precision-design**
//! demonstration, not a live-model capability result — a live grade needs
//! `coin evaluate` on the Phase-3 VM (issue #2823). The output labels this.

use serde::Serialize;

use super::scorer::score_run;
use super::types::{RunReport, Strategy};

/// Float slack when comparing reach/precision percentages (avoid FP jitter).
const PCT_EPS: f64 = 1e-9;

/// One ranked entry: a single saved run scored for the local standings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalStanding {
    /// 1-based rank in the local standings (1 == best).
    pub rank: usize,
    /// Profile the run was saved under.
    pub profile: String,
    /// The run's id.
    pub run_id: String,
    /// The model under test.
    pub model: String,
    /// The strategy arm (baseline vs. team).
    pub strategy: Strategy,
    /// reached / total, as a percentage.
    pub reach_pct: f64,
    /// reached / submitted, as a percentage.
    pub precision_pct: f64,
    /// Targets reached.
    pub reached: usize,
    /// Inputs submitted (precision denominator).
    pub submitted: usize,
    /// Total targets in the run.
    pub total: usize,
    /// `true` when the run came from a mock oracle (offline scaffold).
    pub offline_scaffold: bool,
}

/// Best-of-arm comparison: did the multi-agent `team` climb above the
/// single-model `baseline`? Present only when the standings contain **both**
/// arms (a one-arm board has nothing to compare).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BaselineVsTeam {
    /// Best baseline run id.
    pub baseline_run_id: String,
    /// Best baseline reach percentage.
    pub baseline_reach_pct: f64,
    /// Best baseline precision percentage.
    pub baseline_precision_pct: f64,
    /// Best team run id.
    pub team_run_id: String,
    /// Best team reach percentage.
    pub team_reach_pct: f64,
    /// Best team precision percentage.
    pub team_precision_pct: f64,
    /// team − baseline reach (percentage points).
    pub reach_delta_pct: f64,
    /// team − baseline precision (percentage points).
    pub precision_delta_pct: f64,
    /// `true` when the team strictly improves reach, or matches reach and
    /// strictly improves precision — i.e. climbs above the baseline.
    pub team_beats_baseline: bool,
    /// Human-readable verdict.
    pub verdict: String,
}

/// The full LOCAL leaderboard for a scope (one profile or all profiles).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalLeaderboard {
    /// Human-readable scope (e.g. `profile 'opus'` or `all profiles`).
    pub scope: String,
    /// Ranked standings, best first.
    pub standings: Vec<LocalStanding>,
    /// Best-baseline vs. best-team verdict, when both arms are present.
    pub baseline_vs_team: Option<BaselineVsTeam>,
    /// `true` when any ranked run is an offline scaffold run (⇒ the verdict is a
    /// design demonstration, not a live-model result).
    pub any_offline_scaffold: bool,
}

/// Build a LOCAL leaderboard from `(profile, report)` pairs.
///
/// Standings are ranked by **reach** then **precision** (both descending), with
/// `(profile, run_id)` as a deterministic final tiebreak so the ordering never
/// depends on input order or filesystem enumeration.
#[must_use]
pub fn build_local_leaderboard<I>(scope: impl Into<String>, runs: I) -> LocalLeaderboard
where
    I: IntoIterator<Item = (String, RunReport)>,
{
    let mut standings: Vec<LocalStanding> = runs
        .into_iter()
        .map(|(profile, report)| {
            let score = score_run(&report);
            LocalStanding {
                rank: 0,
                profile,
                run_id: report.run_id,
                model: report.model,
                strategy: report.strategy,
                reach_pct: score.overall.reach_pct(),
                precision_pct: score.overall.precision_pct(),
                reached: score.overall.reached,
                submitted: score.overall.submitted,
                total: score.overall.total,
                offline_scaffold: report.offline_scaffold,
            }
        })
        .collect();

    standings.sort_by(|a, b| {
        b.reach_pct
            .partial_cmp(&a.reach_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                b.precision_pct
                    .partial_cmp(&a.precision_pct)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.profile.cmp(&b.profile))
            .then(a.run_id.cmp(&b.run_id))
    });
    for (i, s) in standings.iter_mut().enumerate() {
        s.rank = i + 1;
    }

    let any_offline_scaffold = standings.iter().any(|s| s.offline_scaffold);
    let baseline_vs_team = baseline_vs_team(&standings);

    LocalLeaderboard {
        scope: scope.into(),
        standings,
        baseline_vs_team,
        any_offline_scaffold,
    }
}

/// Compare the best baseline run to the best team run. Standings are already
/// ranked best-first, so the first entry of each arm is its best run.
fn baseline_vs_team(standings: &[LocalStanding]) -> Option<BaselineVsTeam> {
    let baseline = standings
        .iter()
        .find(|s| s.strategy == Strategy::Baseline)?;
    let team = standings.iter().find(|s| s.strategy == Strategy::Team)?;

    let reach_delta = team.reach_pct - baseline.reach_pct;
    let precision_delta = team.precision_pct - baseline.precision_pct;
    // The team "climbs above" the baseline when it strictly reaches more, or
    // matches reach and over-claims strictly less (COIN's precision tiebreak).
    let team_beats_baseline =
        reach_delta > PCT_EPS || (reach_delta.abs() <= PCT_EPS && precision_delta > PCT_EPS);

    let verdict = if team_beats_baseline {
        format!(
            "multi-agent team CLIMBS ABOVE the single-model baseline \
             (reach {reach_delta:+.1} pts, precision {precision_delta:+.1} pts)"
        )
    } else if reach_delta < -PCT_EPS || (reach_delta.abs() <= PCT_EPS && precision_delta < -PCT_EPS)
    {
        format!(
            "multi-agent team is BELOW the single-model baseline \
             (reach {reach_delta:+.1} pts, precision {precision_delta:+.1} pts)"
        )
    } else {
        "multi-agent team is TIED with the single-model baseline (no reach or precision gain)"
            .to_string()
    };

    Some(BaselineVsTeam {
        baseline_run_id: baseline.run_id.clone(),
        baseline_reach_pct: baseline.reach_pct,
        baseline_precision_pct: baseline.precision_pct,
        team_run_id: team.run_id.clone(),
        team_reach_pct: team.reach_pct,
        team_precision_pct: team.precision_pct,
        reach_delta_pct: reach_delta,
        precision_delta_pct: precision_delta,
        team_beats_baseline,
        verdict,
    })
}
