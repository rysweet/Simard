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
#[derive(Clone, Debug, PartialEq)]
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

    /// Extracts findings from any variant that carries them.
    pub fn findings(&self) -> &[ReviewFinding] {
        match self {
            Self::Applied { findings }
            | Self::ReviewBlocked { findings }
            | Self::CommitFailed { findings, .. } => findings,
            Self::PlanFailed { .. } => &[],
        }
    }
}

impl std::fmt::Display for ApplyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Applied { findings } => {
                write!(f, "applied ({} findings)", findings.len())
            }
            Self::ReviewBlocked { findings } => {
                write!(f, "review-blocked ({} findings)", findings.len())
            }
            Self::PlanFailed { reason } => {
                write!(f, "plan-failed: {reason}")
            }
            Self::CommitFailed { reason, findings } => {
                write!(f, "commit-failed: {reason} ({} findings)", findings.len())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_finding(severity: Severity) -> ReviewFinding {
        ReviewFinding {
            category: crate::review_pipeline::FindingCategory::Bug,
            severity,
            description: "test".to_string(),
            file_path: "test.rs".to_string(),
            line_range: None,
        }
    }

    #[test]
    fn apply_result_applied_is_applied() {
        let r = ApplyResult::Applied { findings: vec![] };
        assert!(r.is_applied());
    }

    #[test]
    fn apply_result_review_blocked_not_applied() {
        let r = ApplyResult::ReviewBlocked { findings: vec![] };
        assert!(!r.is_applied());
    }

    #[test]
    fn apply_result_plan_failed_not_applied() {
        let r = ApplyResult::PlanFailed {
            reason: "err".to_string(),
        };
        assert!(!r.is_applied());
    }

    #[test]
    fn apply_result_commit_failed_not_applied() {
        let r = ApplyResult::CommitFailed {
            reason: "err".to_string(),
            findings: vec![],
        };
        assert!(!r.is_applied());
    }

    #[test]
    fn has_critical_true_when_critical_finding() {
        let r = ApplyResult::Applied {
            findings: vec![make_finding(Severity::Critical)],
        };
        assert!(r.has_critical());
    }

    #[test]
    fn has_critical_false_when_no_critical() {
        let r = ApplyResult::Applied {
            findings: vec![make_finding(Severity::Low)],
        };
        assert!(!r.has_critical());
    }

    #[test]
    fn has_critical_false_for_plan_failed() {
        let r = ApplyResult::PlanFailed {
            reason: "err".to_string(),
        };
        assert!(!r.has_critical());
    }

    #[test]
    fn findings_empty_for_plan_failed() {
        let r = ApplyResult::PlanFailed {
            reason: "err".to_string(),
        };
        assert!(r.findings().is_empty());
    }

    #[test]
    fn findings_returns_findings_for_applied() {
        let r = ApplyResult::Applied {
            findings: vec![make_finding(Severity::High)],
        };
        assert_eq!(r.findings().len(), 1);
    }

    #[test]
    fn display_applied() {
        let r = ApplyResult::Applied {
            findings: vec![make_finding(Severity::Low), make_finding(Severity::Medium)],
        };
        assert_eq!(format!("{r}"), "applied (2 findings)");
    }

    #[test]
    fn display_review_blocked() {
        let r = ApplyResult::ReviewBlocked { findings: vec![] };
        assert_eq!(format!("{r}"), "review-blocked (0 findings)");
    }

    #[test]
    fn display_plan_failed() {
        let r = ApplyResult::PlanFailed {
            reason: "oops".to_string(),
        };
        assert_eq!(format!("{r}"), "plan-failed: oops");
    }

    #[test]
    fn display_commit_failed() {
        let r = ApplyResult::CommitFailed {
            reason: "git err".to_string(),
            findings: vec![make_finding(Severity::Low)],
        };
        assert_eq!(format!("{r}"), "commit-failed: git err (1 findings)");
    }
}
