//! LOCAL leaderboard (research doc Part 3.3, the harness's done-gate).
//!
//! The [`leaderboard`](compare_to_leaderboard) comparator diffs *one* run against
//! COIN's **published** board. This module instead materialises the **LOCAL**
//! leaderboard the objective is graded on: it ranks every locally-saved run
//! (across profiles) by reach/precision and renders the **single-model baseline
//! vs. multi-agent team** head-to-head that decides whether the multiagent
//! pattern has "measurably climbed the local leaderboard above the single-model
//! baseline".
//!
//! **LOCAL-ONLY.** Nothing here contacts COIN or posts a result; it reads the
//! gym's own persisted runs and computes a ranking. Offline scaffold runs are
//! labelled so a mock-oracle A/B is never mistaken for a real capability result.
//!
//! ## Ranking
//! Rows are ordered by reach percentage (desc), then precision percentage
//! (desc), then earliest start (a stable, reproducible tie-break), then run-id.
//!
//! ## Head-to-head verdict
//! For every model that has **both** a baseline and a team run, the *best* run of
//! each strategy (same ordering as the ranking) is compared. Because COIN
//! precision punishes over-claiming, "team beats baseline" is defined the way the
//! benchmark rewards: **strictly more reached, or equal reach with strictly
//! higher precision**. A team/baseline pair drawn from *different* snapshots is
//! flagged `cross-snapshot` and excluded from the aggregate verdict — an A/B is
//! only meaningful on the same pinned target set.

use std::path::Path;

use serde::Serialize;

use super::profiles::{list_profiles, list_runs};
use super::scorer::score_run;
use super::types::{CoinGymResult, Strategy};

/// One ranked row of the LOCAL leaderboard: a scored, persisted run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalRunRow {
    /// 1-based rank in the overall ranking.
    pub rank: usize,
    /// The run identifier.
    pub run_id: String,
    /// Model under test.
    pub model: String,
    /// Strategy under test (baseline vs. team).
    pub strategy: Strategy,
    /// Profile the run is stored under.
    pub profile: String,
    /// Snapshot the targets were drawn from.
    pub snapshot: String,
    /// Overall reach percentage.
    pub reach_pct: f64,
    /// Overall precision percentage.
    pub precision_pct: f64,
    /// Targets reached.
    pub reached: usize,
    /// Inputs submitted (precision denominator).
    pub submitted: usize,
    /// Total targets in scope.
    pub total: usize,
    /// Rendered `R/W/A/T/N/E` histogram.
    pub histogram: String,
    /// Whether the run graded against a mock oracle (offline scaffold).
    pub offline_scaffold: bool,
    /// Wall-clock start (unix epoch milliseconds); the ranking tie-break.
    pub started_at_unix_ms: u128,
}

/// A single-model **baseline vs. team** head-to-head over the best run of each
/// strategy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HeadToHead {
    /// The model both runs share.
    pub model: String,
    /// Best baseline run for this model.
    pub baseline: LocalRunRow,
    /// Best team run for this model.
    pub team: LocalRunRow,
    /// local team − baseline reach (percentage points).
    pub reach_delta_pct: f64,
    /// local team − baseline precision (percentage points).
    pub precision_delta_pct: f64,
    /// `true` iff the team beats the baseline: strictly more reached, or equal
    /// reach with strictly higher precision.
    pub team_beats_baseline: bool,
    /// `true` iff the baseline strictly beats the team (the inverse test); a
    /// regression the aggregate verdict must veto.
    pub baseline_beats_team: bool,
    /// `true` when the two runs used different snapshots ⇒ not a fair A/B, so the
    /// pair is excluded from the aggregate verdict.
    pub cross_snapshot: bool,
    /// Human-readable interpretation.
    pub verdict: String,
}

/// The full LOCAL leaderboard: a ranking plus the baseline-vs-team verdict.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalLeaderboard {
    /// Ranked rows (best first).
    pub rows: Vec<LocalRunRow>,
    /// Per-model baseline-vs-team head-to-heads (only models with both).
    pub head_to_head: Vec<HeadToHead>,
    /// `true` if any ranked run is an offline scaffold (mock oracle) result.
    pub any_offline: bool,
    /// Aggregate done-gate verdict: at least one **comparable** (same-snapshot)
    /// head-to-head has the team beating the baseline, and **no** comparable
    /// head-to-head has the baseline beating the team.
    pub multiagent_beats_baseline: bool,
    /// Human-readable summary of the aggregate verdict.
    pub summary: String,
}

/// Build the LOCAL leaderboard from persisted runs under `home`.
///
/// When `profile_filter` is `Some`, only that profile's runs are ranked;
/// otherwise every profile is aggregated.
///
/// # Errors
/// Propagates any I/O or parse error from enumerating profiles / runs.
pub fn build_local_leaderboard(
    home: &Path,
    profile_filter: Option<&str>,
) -> CoinGymResult<LocalLeaderboard> {
    let mut rows: Vec<LocalRunRow> = Vec::new();

    let profiles = list_profiles(home)?;
    for profile in profiles {
        if let Some(filter) = profile_filter
            && profile.name != filter
        {
            continue;
        }
        for run in list_runs(home, &profile.name)? {
            let score = score_run(&run.report);
            rows.push(LocalRunRow {
                rank: 0,
                run_id: run.report.run_id.clone(),
                model: run.report.model.clone(),
                strategy: run.report.strategy,
                profile: profile.name.clone(),
                snapshot: run.report.snapshot.clone(),
                reach_pct: score.overall.reach_pct(),
                precision_pct: score.overall.precision_pct(),
                reached: score.overall.reached,
                submitted: score.overall.submitted,
                total: score.overall.total,
                histogram: score.histogram.render(),
                offline_scaffold: run.report.offline_scaffold,
                started_at_unix_ms: run.report.started_at_unix_ms,
            });
        }
    }

    rows.sort_by_key(run_order);
    for (i, row) in rows.iter_mut().enumerate() {
        row.rank = i + 1;
    }

    let any_offline = rows.iter().any(|r| r.offline_scaffold);
    let head_to_head = build_head_to_head(&rows);
    let (multiagent_beats_baseline, summary) = aggregate_verdict(&rows, &head_to_head);

    Ok(LocalLeaderboard {
        rows,
        head_to_head,
        any_offline,
        multiagent_beats_baseline,
        summary,
    })
}

/// Sort key for the ranking: reach desc, precision desc, earliest start, run-id.
///
/// Percentages are bucketed to whole basis points before ordering so tiny
/// floating-point noise never reorders otherwise-tied runs; the sign is flipped
/// so a larger percentage sorts first under an ascending `cmp`.
fn run_order(r: &LocalRunRow) -> (i64, i64, u128, String) {
    (
        -pct_key(r.reach_pct),
        -pct_key(r.precision_pct),
        r.started_at_unix_ms,
        r.run_id.clone(),
    )
}

/// Quantise a percentage to integer basis points for a total, noise-free order.
fn pct_key(pct: f64) -> i64 {
    (pct * 100.0).round() as i64
}

/// Whether `team` beats `baseline` the way COIN rewards: strictly more reached,
/// or equal reach with strictly higher precision.
fn beats(team: &LocalRunRow, baseline: &LocalRunRow) -> bool {
    let (tr, br) = (pct_key(team.reach_pct), pct_key(baseline.reach_pct));
    let (tp, bp) = (pct_key(team.precision_pct), pct_key(baseline.precision_pct));
    tr > br || (tr == br && tp > bp)
}

/// Pair the best baseline and best team run of every model that has both, in
/// leaderboard-rank order (so the head-to-head list mirrors the ranking).
fn build_head_to_head(rows: &[LocalRunRow]) -> Vec<HeadToHead> {
    let mut models: Vec<String> = Vec::new();
    for r in rows {
        if !models.contains(&r.model) {
            models.push(r.model.clone());
        }
    }

    let mut out = Vec::new();
    for model in models {
        // Rows are already sorted best-first, so the first match per strategy is
        // the best run of that strategy for this model.
        let baseline = rows
            .iter()
            .find(|r| r.model == model && r.strategy == Strategy::Baseline);
        let team = rows
            .iter()
            .find(|r| r.model == model && r.strategy == Strategy::Team);
        let (Some(baseline), Some(team)) = (baseline, team) else {
            continue;
        };

        let reach_delta_pct = team.reach_pct - baseline.reach_pct;
        let precision_delta_pct = team.precision_pct - baseline.precision_pct;
        let team_beats_baseline = beats(team, baseline);
        let baseline_beats_team = beats(baseline, team);
        let cross_snapshot = team.snapshot != baseline.snapshot;

        let verdict = if cross_snapshot {
            format!(
                "cross-snapshot ({} vs {}) — not a fair A/B; excluded from the aggregate verdict",
                baseline.snapshot, team.snapshot
            )
        } else if team_beats_baseline {
            format!(
                "team BEATS baseline (Δreach {reach_delta_pct:+.1} pts, Δprecision {precision_delta_pct:+.1} pts)"
            )
        } else if baseline_beats_team {
            format!(
                "baseline beats team (Δreach {reach_delta_pct:+.1} pts, Δprecision {precision_delta_pct:+.1} pts) — regression"
            )
        } else {
            "tie (same reach and precision)".to_string()
        };

        out.push(HeadToHead {
            model,
            baseline: baseline.clone(),
            team: team.clone(),
            reach_delta_pct,
            precision_delta_pct,
            team_beats_baseline,
            baseline_beats_team,
            cross_snapshot,
            verdict,
        });
    }
    out
}

/// Compute the aggregate done-gate verdict and its human-readable summary.
fn aggregate_verdict(rows: &[LocalRunRow], h2h: &[HeadToHead]) -> (bool, String) {
    if rows.is_empty() {
        return (
            false,
            "no local runs yet — run `coin-gym run <model> --strategy baseline|team` first"
                .to_string(),
        );
    }
    let comparable: Vec<&HeadToHead> = h2h.iter().filter(|h| !h.cross_snapshot).collect();
    if comparable.is_empty() {
        return (
            false,
            "no comparable baseline-vs-team pair on a shared snapshot yet — \
             run both strategies on the same targets to A/B them"
                .to_string(),
        );
    }
    let any_win = comparable.iter().any(|h| h.team_beats_baseline);
    let any_regression = comparable.iter().any(|h| h.baseline_beats_team);
    let beats = any_win && !any_regression;

    let wins = comparable.iter().filter(|h| h.team_beats_baseline).count();
    let summary = if beats {
        format!(
            "MULTIAGENT BEATS SINGLE-MODEL BASELINE — team wins {wins}/{} comparable A/B(s), \
             no regressions",
            comparable.len()
        )
    } else if any_regression {
        "multiagent does NOT clearly beat the baseline — at least one comparable A/B is a \
         regression (baseline beats team)"
            .to_string()
    } else {
        "multiagent does not yet beat the baseline — comparable A/B(s) tie".to_string()
    };
    (beats, summary)
}
