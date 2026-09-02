//! Scorer (research doc Part 3.3, component 4).
//!
//! Computes COIN's two headline metrics — **reach rate** and **precision** —
//! overall and split by family (frontier vs. non-trivial reachable), plus the
//! `R/W/A/T/N/E` outcome histogram.
//!
//! - **reach rate** = reached / total targets.
//! - **precision** = reached / *submitted* inputs (abstain and no-submission do
//!   not count toward the denominator). This exposes over-claiming.

use serde::Serialize;

use super::types::{Outcome, OutcomeCode, RunReport, TargetFamily};

/// Count of each `R/W/A/T/N/E` outcome.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OutcomeHistogram {
    /// `R` — reached.
    pub reached: usize,
    /// `W` — wrong input submitted.
    pub wrong_input: usize,
    /// `A` — abstained.
    pub abstained: usize,
    /// `T` — timed out.
    pub timed_out: usize,
    /// `N` — no submission.
    pub no_submission: usize,
    /// `E` — error.
    pub error: usize,
}

impl OutcomeHistogram {
    /// Tally outcomes into a histogram.
    #[must_use]
    pub fn tally<'a>(outcomes: impl IntoIterator<Item = &'a Outcome>) -> Self {
        let mut h = Self::default();
        for o in outcomes {
            match o.code {
                OutcomeCode::Reached => h.reached += 1,
                OutcomeCode::WrongInput => h.wrong_input += 1,
                OutcomeCode::Abstained => h.abstained += 1,
                OutcomeCode::TimedOut => h.timed_out += 1,
                OutcomeCode::NoSubmission => h.no_submission += 1,
                OutcomeCode::Error => h.error += 1,
            }
        }
        h
    }

    /// Total across all outcome codes.
    #[must_use]
    pub fn total(&self) -> usize {
        self.reached
            + self.wrong_input
            + self.abstained
            + self.timed_out
            + self.no_submission
            + self.error
    }

    /// Compact `R:_/W:_/A:_/T:_/N:_/E:_` rendering.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "R:{}/W:{}/A:{}/T:{}/N:{}/E:{}",
            self.reached,
            self.wrong_input,
            self.abstained,
            self.timed_out,
            self.no_submission,
            self.error
        )
    }
}

/// Reach/precision for one family (or overall).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ReachPrecision {
    /// Targets reached.
    pub reached: usize,
    /// Inputs submitted (precision denominator).
    pub submitted: usize,
    /// Total targets in scope.
    pub total: usize,
    /// reached / total (0.0 when total == 0).
    pub reach_rate: f64,
    /// reached / submitted (0.0 when submitted == 0).
    pub precision: f64,
}

impl ReachPrecision {
    /// Compute reach/precision over a set of outcomes.
    #[must_use]
    pub fn compute<'a>(outcomes: impl IntoIterator<Item = &'a Outcome>) -> Self {
        let mut reached = 0usize;
        let mut submitted = 0usize;
        let mut total = 0usize;
        for o in outcomes {
            total += 1;
            if o.reached() {
                reached += 1;
            }
            if o.submitted() {
                submitted += 1;
            }
        }
        Self {
            reached,
            submitted,
            total,
            reach_rate: ratio(reached, total),
            precision: ratio(reached, submitted),
        }
    }

    /// reach/precision as percentages, for leaderboard comparison.
    #[must_use]
    pub fn reach_pct(&self) -> f64 {
        self.reach_rate * 100.0
    }

    /// Precision as a percentage.
    #[must_use]
    pub fn precision_pct(&self) -> f64 {
        self.precision * 100.0
    }
}

/// Reach/precision for a single family.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FamilyScore {
    /// The family scored.
    pub family: TargetFamily,
    /// Reach/precision within the family.
    pub score: ReachPrecision,
}

/// The full score for a run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Score {
    /// The run this score is for.
    pub run_id: String,
    /// The model under test.
    pub model: String,
    /// Overall reach/precision.
    pub overall: ReachPrecision,
    /// Per-family reach/precision (frontier first, then non-trivial reachable).
    pub by_family: Vec<FamilyScore>,
    /// `R/W/A/T/N/E` histogram.
    pub histogram: OutcomeHistogram,
    /// Whether the run used an offline mock oracle.
    pub offline_scaffold: bool,
}

/// Score a run: overall + per-family reach/precision + outcome histogram.
#[must_use]
pub fn score_run(report: &RunReport) -> Score {
    let overall = ReachPrecision::compute(&report.outcomes);
    let by_family = [TargetFamily::Frontier, TargetFamily::NonTrivialReachable]
        .into_iter()
        .filter_map(|family| {
            let members: Vec<&Outcome> = report
                .outcomes
                .iter()
                .filter(|o| o.family == family)
                .collect();
            if members.is_empty() {
                None
            } else {
                Some(FamilyScore {
                    family,
                    score: ReachPrecision::compute(members),
                })
            }
        })
        .collect();
    Score {
        run_id: report.run_id.clone(),
        model: report.model.clone(),
        overall,
        by_family,
        histogram: OutcomeHistogram::tally(&report.outcomes),
        offline_scaffold: report.offline_scaffold,
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}
