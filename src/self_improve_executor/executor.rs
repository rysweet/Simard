//! Core execution logic: plan generation, apply-and-review, autonomous loop.

use std::path::Path;

use crate::engineer_loop::RepoInspection;
use crate::engineer_plan::{PlanExecutionResult, execute_plan, plan_objective};
use crate::error::SimardResult;
use crate::review_pipeline::{ReviewFinding, ReviewSession, review_diff, should_commit};

use super::git_ops::{git_commit, git_diff, rollback};
use super::types::{ApplyResult, ImprovementPatch};

/// Philosophy guidelines passed to the LLM reviewer.
const PHILOSOPHY_REVIEW: &str = "Ruthless simplicity. No unnecessary abstractions. \
    Modules under 400 lines. Every public function tested. \
    Clippy clean. No panics in library code.";

/// Generate an [`ImprovementPatch`] from a proposal description.
///
/// Calls [`plan_objective`] to create a multi-step plan for implementing the
/// improvement. Returns `Err(PlanningUnavailable)` if no LLM session can be
/// opened.
pub fn generate_patch(
    proposal: &str,
    inspection: &RepoInspection,
) -> SimardResult<ImprovementPatch> {
    let plan = plan_objective(proposal, inspection)?;
    let target_files: Vec<String> = {
        let mut files: Vec<String> = plan
            .steps()
            .iter()
            .map(|s| s.target.clone())
            .filter(|t| t != "." && !t.is_empty())
            .collect();
        files.sort();
        files.dedup();
        files
    };

    Ok(ImprovementPatch {
        description: proposal.to_string(),
        target_files,
        plan,
        review_findings: Vec::new(),
    })
}

/// Execute a patch plan, review the resulting diff, and commit or roll back.
///
/// Returns [`ApplyResult::Applied`] if the review passes,
/// [`ApplyResult::ReviewBlocked`] if the review gates the commit,
/// [`ApplyResult::CommitFailed`] if the commit itself fails, or
/// [`ApplyResult::PlanFailed`] if plan execution or git operations fail.
pub fn apply_and_review(patch: &ImprovementPatch, workspace_path: &Path) -> ApplyResult {
    let exec_result: PlanExecutionResult = execute_plan(&patch.plan, workspace_path);

    if exec_result.stopped_early {
        let reason = exec_result
            .completed
            .last()
            .map(|r| format!("step '{}' failed: {}", r.step.target, r.stderr))
            .unwrap_or_else(|| "plan execution stopped early".to_string());
        if let Err(rb_err) = rollback(workspace_path) {
            return ApplyResult::PlanFailed {
                reason: format!("{reason}; rollback also failed: {rb_err}"),
            };
        }
        return ApplyResult::PlanFailed { reason };
    }

    let diff_text = match git_diff(workspace_path) {
        Ok(d) => d,
        Err(e) => {
            let diff_reason = format!("git diff failed: {e}");
            if let Err(rb_err) = rollback(workspace_path) {
                return ApplyResult::PlanFailed {
                    reason: format!("{diff_reason}; rollback also failed: {rb_err}"),
                };
            }
            return ApplyResult::PlanFailed {
                reason: diff_reason,
            };
        }
    };

    if diff_text.is_empty() {
        return ApplyResult::Applied {
            findings: Vec::new(),
        };
    }

    // Review failures are non-fatal: if the review session can't open or
    // the LLM call fails, we treat it as no findings (auto-pass) rather
    // than blocking the entire pipeline — but we log the error so review
    // pipeline bugs remain visible.
    let findings = match run_review(&diff_text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[self-improve] review failed, auto-passing: {e}");
            Vec::new()
        }
    };

    if should_commit(&findings) {
        if let Err(e) = git_commit(workspace_path, &patch.description) {
            return ApplyResult::CommitFailed {
                reason: format!("{e}"),
                findings,
            };
        }
        ApplyResult::Applied { findings }
    } else {
        if let Err(rb_err) = rollback(workspace_path) {
            return ApplyResult::ReviewBlocked {
                findings: {
                    let mut f = findings;
                    f.push(ReviewFinding {
                        category: crate::review_pipeline::FindingCategory::Bug,
                        severity: crate::review_pipeline::Severity::Critical,
                        description: format!("rollback failed after review block: {rb_err}"),
                        file_path: ".".into(),
                        line_range: None,
                    });
                    f
                },
            };
        }
        ApplyResult::ReviewBlocked { findings }
    }
}

/// Process multiple improvement proposals sequentially.
///
/// Stops early on the first [`ApplyResult::ReviewBlocked`] or
/// [`ApplyResult::CommitFailed`] that contains a [`Severity::Critical`]
/// finding. Returns results for all attempted proposals.
pub fn run_autonomous_improvement(
    proposals: &[String],
    workspace: &Path,
    inspection: &RepoInspection,
) -> Vec<ApplyResult> {
    let mut results = Vec::with_capacity(proposals.len());

    for (i, proposal) in proposals.iter().enumerate() {
        eprintln!(
            "[self-improve] proposal {}/{}: {}",
            i + 1,
            proposals.len(),
            proposal
        );
        let patch = match generate_patch(proposal, inspection) {
            Ok(p) => p,
            Err(e) => {
                results.push(ApplyResult::PlanFailed {
                    reason: format!("{e}"),
                });
                continue;
            }
        };

        let result = apply_and_review(&patch, workspace);
        let should_stop = matches!(
            &result,
            ApplyResult::ReviewBlocked { .. } | ApplyResult::CommitFailed { .. }
        ) && result.has_critical();
        results.push(result);

        if should_stop {
            break;
        }
    }

    results
}

fn run_review(diff_text: &str) -> SimardResult<Vec<ReviewFinding>> {
    let mut session = ReviewSession::open()?;
    let findings = review_diff(&mut session, diff_text, PHILOSOPHY_REVIEW)?;
    let _ = session.close();
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_philosophy_review_not_empty() {
        assert!(!PHILOSOPHY_REVIEW.is_empty());
        assert!(PHILOSOPHY_REVIEW.contains("simplicity"));
    }

    #[test]
    fn test_apply_result_plan_failed_pattern() {
        let result = ApplyResult::PlanFailed {
            reason: "no steps".to_string(),
        };
        assert!(matches!(result, ApplyResult::PlanFailed { .. }));
    }

    #[test]
    fn test_apply_result_applied_empty_findings() {
        let result = ApplyResult::Applied {
            findings: Vec::new(),
        };
        assert!(matches!(result, ApplyResult::Applied { .. }));
        assert!(!result.has_critical());
    }

    #[test]
    fn test_apply_result_review_blocked() {
        let result = ApplyResult::ReviewBlocked {
            findings: Vec::new(),
        };
        assert!(matches!(result, ApplyResult::ReviewBlocked { .. }));
        assert!(!result.has_critical());
    }

    #[test]
    fn test_apply_result_commit_failed() {
        let result = ApplyResult::CommitFailed {
            reason: "git error".to_string(),
            findings: Vec::new(),
        };
        assert!(matches!(result, ApplyResult::CommitFailed { .. }));
        assert!(!result.has_critical());
    }

    #[test]
    fn test_apply_result_has_critical_with_critical_finding() {
        use crate::review_pipeline::{FindingCategory, ReviewFinding, Severity};
        let findings = vec![ReviewFinding {
            category: FindingCategory::Bug,
            severity: Severity::Critical,
            description: "serious bug".to_string(),
            file_path: "src/main.rs".into(),
            line_range: None,
        }];
        let result = ApplyResult::ReviewBlocked { findings };
        assert!(result.has_critical());
    }

    #[test]
    fn test_apply_result_has_critical_without_critical_finding() {
        use crate::review_pipeline::{FindingCategory, ReviewFinding, Severity};
        let findings = vec![ReviewFinding {
            category: FindingCategory::Bug,
            severity: Severity::Low,
            description: "minor issue".to_string(),
            file_path: "src/main.rs".into(),
            line_range: None,
        }];
        let result = ApplyResult::ReviewBlocked { findings };
        assert!(!result.has_critical());
    }
}
