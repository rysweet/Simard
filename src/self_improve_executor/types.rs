//! Data types for self-improvement patches and results.

use crate::engineer_plan::Plan;
use crate::review_pipeline::{ReviewFinding, Severity};

/// A planned improvement patch ready for execution and review.
#[derive(Clone, Debug)]
pub struct ImprovementPatch {
    /// Human-readable description of the improvement.
    pub description: String,
    /// Files expected to be affected.
    pub target_files: Vec<String>,
    /// The LLM-generated execution plan.
    pub plan: Plan,
    /// Review findings (populated after review).
    pub review_findings: Vec<ReviewFinding>,
}

/// Outcome of attempting to apply a single improvement patch.
#[derive(Clone, Debug)]
pub enum ApplyResult {
    /// The patch was applied, reviewed, and committed.
    Applied { findings: Vec<ReviewFinding> },
    /// The review blocked the commit; changes were rolled back.
    ReviewBlocked { findings: Vec<ReviewFinding> },
    /// The plan failed to execute; changes were rolled back.
    PlanFailed { reason: String },
    /// The review passed but the git commit failed; changes remain staged.
    CommitFailed {
        reason: String,
        findings: Vec<ReviewFinding>,
    },
}

impl ApplyResult {
    /// Returns `true` if the result is [`ApplyResult::Applied`].
    pub fn is_applied(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    /// Returns `true` if any finding has [`Severity::Critical`].
    pub fn has_critical(&self) -> bool {
        let findings = match self {
            Self::Applied { findings }
            | Self::ReviewBlocked { findings }
            | Self::CommitFailed { findings, .. } => findings,
            Self::PlanFailed { .. } => return false,
        };
        findings.iter().any(|f| f.severity == Severity::Critical)
    }
}
