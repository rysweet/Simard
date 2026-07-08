//! Head-to-head **baseline vs. team** benchmark (research doc Part 3.3 — the
//! goal's headline claim, "measure a single-model baseline vs. a multi-agent
//! team").
//!
//! [`run`](super::dispatch_with_home) and `score` already evaluate one strategy
//! at a time, but the goal's **done-gate** is a single, honest verdict: on the
//! *same* target set, does the multi-agent **team** *measurably* beat the
//! single-model **baseline**? This module answers exactly that. It runs both
//! strategies over one scenario, diffs COIN's two headline metrics — **reach**
//! and **precision** — and classifies the result against a material [`margin`]
//! so a sub-noise wiggle is never mistaken for a capability win.
//!
//! Because COIN **precision** punishes over-claiming, the interesting win is
//! usually on precision: the team's skeptic/abstain gate declines low-confidence
//! (wrong) inputs the baseline would submit, lifting precision at equal reach.
//! On the bundled sample that is precisely the outcome — reach ties at 60%,
//! precision climbs 60% → 100% — so the verdict is [`Verdict::TeamBeatsBaseline`].
//!
//! The [`HeadToHead`] value serialises to JSON as the **Signal milestone-report
//! payload**, and [`HeadToHead::signal_line`] renders a one-line headline for a
//! Signal post. Like the rest of the Gym (Phase 4) the underlying grade is an
//! **offline mock oracle**; a real baseline-vs-team result needs a `coin
//! evaluate` grade on a provisioned host (Phase 3, #2823). **LOCAL-ONLY**:
//! nothing here is submitted externally.
//!
//! [`margin`]: HeadToHead::margin_pct

use serde::Serialize;

use super::scorer::{Score, score_run};
use super::types::RunReport;

/// Default material margin (percentage points) a headline metric must move to
/// count as a **measurable** difference rather than run-to-run noise.
pub const DEFAULT_MARGIN_PCT: f64 = 1.0;

/// How one headline metric moved from baseline to team, relative to the margin.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricMove {
    /// Team improved the metric by more than the margin.
    Improved,
    /// Team regressed the metric by more than the margin.
    Regressed,
    /// Team stayed within the margin (no measurable change).
    Flat,
}

impl MetricMove {
    /// Classify a `team − baseline` delta (percentage points) against `margin`.
    #[must_use]
    pub fn classify(delta_pct: f64, margin_pct: f64) -> Self {
        if delta_pct > margin_pct {
            Self::Improved
        } else if delta_pct < -margin_pct {
            Self::Regressed
        } else {
            Self::Flat
        }
    }
}

/// The head-to-head verdict on whether the multi-agent team beats the baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Team **measurably beats** baseline: it improved at least one headline
    /// metric beyond the margin and regressed neither beyond it.
    TeamBeatsBaseline,
    /// Baseline **measurably beats** team — the multi-agent scaffold is a net
    /// regression here (a signal to re-examine the team's gate).
    BaselineBeatsTeam,
    /// Both headline metrics stayed within the margin — no measurable difference.
    Tie,
    /// Team improved one headline metric beyond the margin but regressed the
    /// other beyond it — a reach/precision trade-off with no dominant winner.
    MixedTradeoff,
}

impl Verdict {
    /// Classify from each metric's [`MetricMove`].
    #[must_use]
    pub fn from_moves(reach: MetricMove, precision: MetricMove) -> Self {
        let improves = reach == MetricMove::Improved || precision == MetricMove::Improved;
        let regresses = reach == MetricMove::Regressed || precision == MetricMove::Regressed;
        match (improves, regresses) {
            (true, false) => Self::TeamBeatsBaseline,
            (false, true) => Self::BaselineBeatsTeam,
            (true, true) => Self::MixedTradeoff,
            (false, false) => Self::Tie,
        }
    }

    /// Uppercase, stable label used in CLI output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TeamBeatsBaseline => "TEAM-BEATS-BASELINE",
            Self::BaselineBeatsTeam => "BASELINE-BEATS-TEAM",
            Self::Tie => "TIE",
            Self::MixedTradeoff => "MIXED-TRADEOFF",
        }
    }

    /// Whether the team measurably beat the baseline (the done-gate condition).
    #[must_use]
    pub fn is_team_win(self) -> bool {
        matches!(self, Self::TeamBeatsBaseline)
    }
}

/// A complete baseline-vs-team head-to-head over one target set.
///
/// Serialises to JSON as the **Signal milestone-report payload**.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HeadToHead {
    /// Model evaluated under both strategies.
    pub model: String,
    /// Snapshot the targets were drawn from.
    pub snapshot: String,
    /// Material margin (percentage points) used to classify a measurable move.
    pub margin_pct: f64,
    /// Run id of the baseline run.
    pub baseline_run_id: String,
    /// Run id of the team run.
    pub team_run_id: String,
    /// Full baseline score (carries the per-family split + histogram).
    pub baseline: Score,
    /// Full team score.
    pub team: Score,
    /// team − baseline overall reach (percentage points).
    pub reach_delta_pct: f64,
    /// team − baseline overall precision (percentage points).
    pub precision_delta_pct: f64,
    /// How reach moved.
    pub reach_move: MetricMove,
    /// How precision moved.
    pub precision_move: MetricMove,
    /// The head-to-head verdict.
    pub verdict: Verdict,
    /// `true` when both runs were graded by a mock oracle (offline scaffold).
    pub offline_scaffold: bool,
    /// Human-readable interpretation + guardrail note.
    pub note: String,
}

impl HeadToHead {
    /// Build a head-to-head from a baseline and a team [`RunReport`] over the
    /// **same** target set, classifying the result against `margin_pct` (negative
    /// margins are clamped to `0.0`).
    ///
    /// The two reports are expected to share a `model` and `snapshot`; the
    /// baseline's are recorded. Grading is inherited from the reports, so an
    /// offline scaffold in/offline scaffold out.
    #[must_use]
    pub fn from_reports(
        baseline_report: &RunReport,
        team_report: &RunReport,
        margin_pct: f64,
    ) -> Self {
        let margin_pct = margin_pct.max(0.0);
        let baseline = score_run(baseline_report);
        let team = score_run(team_report);
        let reach_delta = team.overall.reach_pct() - baseline.overall.reach_pct();
        let precision_delta = team.overall.precision_pct() - baseline.overall.precision_pct();
        let reach_move = MetricMove::classify(reach_delta, margin_pct);
        let precision_move = MetricMove::classify(precision_delta, margin_pct);
        let verdict = Verdict::from_moves(reach_move, precision_move);
        let offline_scaffold = baseline_report.offline_scaffold || team_report.offline_scaffold;
        let note = build_note(
            verdict,
            offline_scaffold,
            reach_delta,
            precision_delta,
            margin_pct,
        );
        Self {
            model: baseline_report.model.clone(),
            snapshot: baseline_report.snapshot.clone(),
            margin_pct,
            baseline_run_id: baseline_report.run_id.clone(),
            team_run_id: team_report.run_id.clone(),
            baseline,
            team,
            reach_delta_pct: reach_delta,
            precision_delta_pct: precision_delta,
            reach_move,
            precision_move,
            verdict,
            offline_scaffold,
            note,
        }
    }

    /// A single-line headline suitable for a **Signal milestone** post.
    #[must_use]
    pub fn signal_line(&self) -> String {
        let scope = if self.offline_scaffold {
            " (offline scaffold)"
        } else {
            ""
        };
        format!(
            "COIN Gym{scope}: {} — team vs baseline on {} → {} (Δreach {:+.1}, Δprecision {:+.1} pts)",
            self.model,
            self.snapshot,
            self.verdict.label(),
            self.reach_delta_pct,
            self.precision_delta_pct,
        )
    }

    /// Render the full human-readable head-to-head table.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("model:     {}\n", self.model));
        out.push_str(&format!("snapshot:  {}\n", self.snapshot));
        out.push_str(&format!(
            "baseline:  reach {:.1}%  precision {:.1}%  {}   [{}]\n",
            self.baseline.overall.reach_pct(),
            self.baseline.overall.precision_pct(),
            self.baseline.histogram.render(),
            self.baseline_run_id,
        ));
        out.push_str(&format!(
            "team:      reach {:.1}%  precision {:.1}%  {}   [{}]\n",
            self.team.overall.reach_pct(),
            self.team.overall.precision_pct(),
            self.team.histogram.render(),
            self.team_run_id,
        ));
        out.push_str(&format!(
            "delta:     reach {:+.1} pts ({})   precision {:+.1} pts ({})   [margin {:.1} pts]\n",
            self.reach_delta_pct,
            move_label(self.reach_move),
            self.precision_delta_pct,
            move_label(self.precision_move),
            self.margin_pct,
        ));
        out.push_str(&format!("verdict:   {}\n", self.verdict.label()));
        out.push_str(&format!("signal:    {}\n", self.signal_line()));
        out.push_str(&format!("note:      {}", self.note));
        out
    }
}

fn move_label(m: MetricMove) -> &'static str {
    match m {
        MetricMove::Improved => "improved",
        MetricMove::Regressed => "regressed",
        MetricMove::Flat => "flat",
    }
}

fn build_note(
    verdict: Verdict,
    offline_scaffold: bool,
    reach_delta: f64,
    precision_delta: f64,
    margin_pct: f64,
) -> String {
    let interpretation = match verdict {
        Verdict::TeamBeatsBaseline => format!(
            "multi-agent team measurably beats the single-model baseline \
             (Δreach {reach_delta:+.1}, Δprecision {precision_delta:+.1} pts; margin {margin_pct:.1})"
        ),
        Verdict::BaselineBeatsTeam => format!(
            "single-model baseline measurably beats the team — the multi-agent scaffold is a net \
             regression here (Δreach {reach_delta:+.1}, Δprecision {precision_delta:+.1} pts; \
             margin {margin_pct:.1})"
        ),
        Verdict::Tie => format!(
            "no measurable difference — both headline metrics stayed within ±{margin_pct:.1} pts"
        ),
        Verdict::MixedTradeoff => format!(
            "reach/precision trade-off — the team improves one headline metric beyond the margin \
             but regresses the other (Δreach {reach_delta:+.1}, Δprecision {precision_delta:+.1} \
             pts; margin {margin_pct:.1}); no dominant winner"
        ),
    };
    if offline_scaffold {
        format!(
            "{interpretation}. OFFLINE SCAFFOLD (mock oracle) — this exercises the head-to-head \
             measurement contract; a real baseline-vs-team result needs a `coin evaluate` grade on \
             a provisioned host (Phase 3, #2823). LOCAL-ONLY."
        )
    } else {
        interpretation
    }
}
