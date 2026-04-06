//! Types for the self-improvement loop.

use crate::gym_scoring::{GymSuiteScore, Regression};

/// Phases of a single self-improvement cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImprovementPhase {
    /// Run the gym suite to establish a baseline score.
    Eval,
    /// Analyze the baseline results for weak dimensions.
    Analyze,
    /// Research possible changes that could address weaknesses.
    Research,
    /// Apply the proposed changes (in a sandbox / canary environment).
    Improve,
    /// Re-run the gym suite against the changed version.
    ReEval,
    /// Compare baseline and post-change scores and decide.
    Decide,
}

impl std::fmt::Display for ImprovementPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Eval => "eval",
            Self::Analyze => "analyze",
            Self::Research => "research",
            Self::Improve => "improve",
            Self::ReEval => "re-eval",
            Self::Decide => "decide",
        };
        f.write_str(label)
    }
}

/// A single proposed change to prompts, policies, or orchestration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposedChange {
    /// Path to the file that would be changed.
    pub file_path: String,
    /// Human-readable description of the change.
    pub description: String,
    /// Why this change is expected to help.
    pub expected_impact: String,
}

/// The outcome of the decision phase.
#[derive(Clone, Debug, PartialEq)]
pub enum ImprovementDecision {
    /// The changes should be committed.
    Commit {
        /// Net overall improvement as a fraction (e.g. 0.05 = 5%).
        net_improvement: f64,
    },
    /// The changes should be reverted.
    Revert {
        /// Why the changes were rejected.
        reason: String,
    },
}

impl std::fmt::Display for ImprovementDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Commit { net_improvement } => {
                write!(f, "commit (net +{:.1}%)", net_improvement * 100.0)
            }
            Self::Revert { reason } => write!(f, "revert: {reason}"),
        }
    }
}

/// Configuration for an improvement cycle.
#[derive(Clone, Debug)]
pub struct ImprovementConfig {
    /// The gym suite to evaluate against.
    pub suite_id: String,
    /// Minimum net improvement required to commit (fraction, e.g. 0.02 = 2%).
    pub min_net_improvement: f64,
    /// Maximum allowed regression on any single dimension (fraction, e.g. 0.05 = 5%).
    pub max_single_regression: f64,
    /// Proposed changes to evaluate.
    pub proposed_changes: Vec<ProposedChange>,
    /// Whether to auto-apply improvements via the plan+review pipeline.
    pub auto_apply: bool,
    /// Dimensions scoring below this threshold are considered "weak" (default 0.6).
    pub weak_threshold: f64,
}

impl Default for ImprovementConfig {
    fn default() -> Self {
        Self {
            suite_id: "progressive".to_string(),
            min_net_improvement: 0.02,
            max_single_regression: 0.05,
            proposed_changes: Vec::new(),
            auto_apply: false,
            weak_threshold: 0.6,
        }
    }
}

/// A complete improvement cycle record with full provenance.
#[derive(Clone, Debug)]
pub struct ImprovementCycle {
    /// The baseline score established in the Eval phase.
    pub baseline: GymSuiteScore,
    /// Changes that were proposed during the Research/Improve phases.
    pub proposed_changes: Vec<ProposedChange>,
    /// The post-change score from the ReEval phase (None if ReEval was skipped).
    pub post_score: Option<GymSuiteScore>,
    /// Regressions detected during the Decide phase.
    pub regressions: Vec<Regression>,
    /// The final decision (None if the cycle was aborted before Decide).
    pub decision: Option<ImprovementDecision>,
    /// The phase the cycle reached before completing or aborting.
    pub final_phase: ImprovementPhase,
    /// Dimensions that scored below the weak threshold during Analyze.
    pub weak_dimensions: Vec<String>,
}

impl std::fmt::Display for ImprovementCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&super::cycle::summarize_cycle(self))
    }
}
