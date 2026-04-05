//! Auto-apply executor for self-improvement proposals.
//!
//! Closes the loop on [`crate::self_improve`] by generating plans from
//! improvement proposals, executing them, running LLM review, and committing
//! or rolling back based on review outcomes.

use std::path::Path;
use std::process::Command;

use crate::engineer_loop::RepoInspection;
use crate::engineer_plan::{Plan, PlanExecutionResult, execute_plan, plan_objective};
use crate::error::SimardResult;
use crate::review_pipeline::{ReviewFinding, ReviewSession, Severity, review_diff, should_commit};

/// Philosophy guidelines passed to the LLM reviewer.
const PHILOSOPHY_REVIEW: &str = "Ruthless simplicity. No unnecessary abstractions. \
    Modules under 400 lines. Every public function tested. \
    Clippy clean. No panics in library code.";

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
}

impl ApplyResult {
    /// Returns `true` if the result is [`ApplyResult::Applied`].
    pub fn is_applied(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    /// Returns `true` if any finding has [`Severity::Critical`].
    pub fn has_critical(&self) -> bool {
        let findings = match self {
            Self::Applied { findings } | Self::ReviewBlocked { findings } => findings,
            Self::PlanFailed { .. } => return false,
        };
        findings.iter().any(|f| f.severity == Severity::Critical)
    }
}

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
    let target_files: Vec<String> = plan
        .steps()
        .iter()
        .map(|s| s.target.clone())
        .filter(|t| t != "." && !t.is_empty())
        .collect();

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
/// [`ApplyResult::ReviewBlocked`] if the review gates the commit, or
/// [`ApplyResult::PlanFailed`] if plan execution fails.
pub fn apply_and_review(patch: &ImprovementPatch, workspace_path: &Path) -> ApplyResult {
    let exec_result: PlanExecutionResult = execute_plan(&patch.plan, workspace_path);

    if exec_result.stopped_early {
        let reason = exec_result
            .completed
            .last()
            .map(|r| format!("step '{}' failed: {}", r.step.target, r.stderr))
            .unwrap_or_else(|| "plan execution stopped early".to_string());
        rollback(workspace_path);
        return ApplyResult::PlanFailed { reason };
    }

    let diff_text = git_diff(workspace_path);
    if diff_text.is_empty() {
        return ApplyResult::Applied {
            findings: Vec::new(),
        };
    }

    let findings = run_review(&diff_text);

    if should_commit(&findings) {
        git_commit(workspace_path, &patch.description);
        ApplyResult::Applied { findings }
    } else {
        rollback(workspace_path);
        ApplyResult::ReviewBlocked { findings }
    }
}

/// Process multiple improvement proposals sequentially.
///
/// Stops early on the first [`ApplyResult::ReviewBlocked`] that contains a
/// [`Severity::Critical`] finding. Returns results for all attempted proposals.
pub fn run_autonomous_improvement(
    proposals: &[String],
    workspace: &Path,
    inspection: &RepoInspection,
) -> Vec<ApplyResult> {
    let mut results = Vec::with_capacity(proposals.len());

    for proposal in proposals {
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
        let should_stop =
            matches!(&result, ApplyResult::ReviewBlocked { .. }) && result.has_critical();
        results.push(result);

        if should_stop {
            break;
        }
    }

    results
}

fn git_diff(workspace: &Path) -> String {
    match Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(workspace)
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => String::new(),
    }
}

fn git_commit(workspace: &Path, message: &str) {
    let _ = Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace)
        .output();
    let _ = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(workspace)
        .output();
}

fn rollback(workspace: &Path) {
    let _ = Command::new("git")
        .args(["checkout", "--", "."])
        .current_dir(workspace)
        .output();
}

fn run_review(diff_text: &str) -> Vec<ReviewFinding> {
    let mut session = match ReviewSession::open() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let findings = review_diff(&mut session, diff_text, PHILOSOPHY_REVIEW).unwrap_or_default();
    let _ = session.close();
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineer_loop::AnalyzedAction;
    use crate::engineer_plan::{Plan, PlanStep};
    use crate::error::SimardError;
    use crate::review_pipeline::{FindingCategory, Severity};

    fn make_patch(steps: Vec<PlanStep>) -> ImprovementPatch {
        ImprovementPatch {
            description: "test improvement".into(),
            target_files: vec!["src/lib.rs".into()],
            plan: Plan::new(steps),
            review_findings: Vec::new(),
        }
    }

    fn passing_step() -> PlanStep {
        step("src/lib.rs", "true")
    }

    fn failing_step() -> PlanStep {
        step("src/fail.rs", "false")
    }

    fn step(target: &str, cmd: &str) -> PlanStep {
        PlanStep {
            action: AnalyzedAction::RunShellCommand,
            target: target.into(),
            expected_outcome: "ok".into(),
            verification_command: cmd.into(),
        }
    }

    fn finding(category: FindingCategory, severity: Severity) -> ReviewFinding {
        ReviewFinding {
            category,
            severity,
            description: "test finding".into(),
            file_path: "src/test.rs".into(),
            line_range: None,
        }
    }

    #[test]
    fn apply_result_is_applied() {
        let applied = ApplyResult::Applied {
            findings: Vec::new(),
        };
        assert!(applied.is_applied());

        let blocked = ApplyResult::ReviewBlocked {
            findings: Vec::new(),
        };
        assert!(!blocked.is_applied());

        let failed = ApplyResult::PlanFailed {
            reason: "oops".into(),
        };
        assert!(!failed.is_applied());
    }

    #[test]
    fn apply_result_has_critical_applied() {
        let result = ApplyResult::Applied {
            findings: vec![finding(FindingCategory::Bug, Severity::Critical)],
        };
        assert!(result.has_critical());
    }

    #[test]
    fn apply_result_has_critical_blocked() {
        let result = ApplyResult::ReviewBlocked {
            findings: vec![finding(FindingCategory::Security, Severity::Critical)],
        };
        assert!(result.has_critical());
    }

    #[test]
    fn apply_result_no_critical() {
        let result = ApplyResult::Applied {
            findings: vec![finding(FindingCategory::Bug, Severity::High)],
        };
        assert!(!result.has_critical());
    }

    #[test]
    fn apply_result_plan_failed_never_critical() {
        let result = ApplyResult::PlanFailed {
            reason: "boom".into(),
        };
        assert!(!result.has_critical());
    }

    #[test]
    fn improvement_patch_construction() {
        let patch = make_patch(vec![passing_step()]);
        assert_eq!(patch.description, "test improvement");
        assert_eq!(patch.target_files, vec!["src/lib.rs"]);
        assert_eq!(patch.plan.len(), 1);
        assert!(patch.review_findings.is_empty());
    }

    #[test]
    fn improvement_patch_empty_plan() {
        let patch = make_patch(Vec::new());
        assert!(patch.plan.is_empty());
        assert_eq!(patch.plan.len(), 0);
    }

    #[test]
    fn generate_patch_without_api_key_returns_unavailable() {
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::set_var("_SIMARD_NO_COPILOT_FALLBACK", "1");
        };
        let inspection = test_inspection();
        let result = generate_patch("improve error handling", &inspection);
        unsafe { std::env::remove_var("_SIMARD_NO_COPILOT_FALLBACK") };
        match result {
            Err(SimardError::PlanningUnavailable { reason }) => {
                assert!(reason.contains("no LLM session available"));
            }
            other => panic!("expected PlanningUnavailable, got: {other:?}"),
        }
    }

    #[test]
    fn apply_and_review_plan_failed_on_bad_step() {
        let patch = make_patch(vec![failing_step()]);
        let result = apply_and_review(&patch, Path::new("/tmp"));
        match &result {
            ApplyResult::PlanFailed { reason } => {
                assert!(reason.contains("failed"));
            }
            other => panic!("expected PlanFailed, got: {other:?}"),
        }
    }

    #[test]
    fn apply_and_review_empty_diff_is_applied() {
        // A plan with only no-op steps produces no diff
        let step = PlanStep {
            action: AnalyzedAction::ReadOnlyScan,
            target: ".".to_string(),
            expected_outcome: "ok".to_string(),
            verification_command: "true".to_string(),
        };
        let patch = make_patch(vec![step]);
        let result = apply_and_review(&patch, Path::new("/tmp"));
        assert!(result.is_applied());
    }

    #[test]
    fn run_autonomous_improvement_empty_proposals() {
        let inspection = test_inspection();
        let results = run_autonomous_improvement(&[], Path::new("/tmp"), &inspection);
        assert!(results.is_empty());
    }

    #[test]
    fn run_autonomous_improvement_planning_unavailable() {
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        let inspection = test_inspection();
        let proposals = vec!["improve X".to_string(), "improve Y".to_string()];
        let results = run_autonomous_improvement(&proposals, Path::new("/tmp"), &inspection);
        // Both should fail with PlanFailed since no LLM is available
        assert_eq!(results.len(), 2);
        for r in &results {
            assert!(matches!(r, ApplyResult::PlanFailed { .. }));
        }
    }

    #[test]
    fn run_autonomous_improvement_continues_on_non_critical_plan_failure() {
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        let inspection = test_inspection();
        let proposals = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let results = run_autonomous_improvement(&proposals, Path::new("/tmp"), &inspection);
        // All three should be attempted (PlanFailed is not a critical ReviewBlocked)
        assert_eq!(results.len(), 3);
    }

    fn test_inspection() -> RepoInspection {
        use crate::goals::{GoalRecord, GoalStatus};
        use crate::session::{SessionId, SessionPhase};
        RepoInspection {
            workspace_root: "/tmp/test-ws".into(),
            repo_root: "/tmp/test-repo".into(),
            branch: "main".into(),
            head: "abc1234".into(),
            worktree_dirty: false,
            changed_files: Vec::new(),
            active_goals: vec![GoalRecord {
                slug: "g".into(),
                title: "Self-improvement".into(),
                rationale: "needed".into(),
                status: GoalStatus::Active,
                priority: 1,
                owner_identity: "test".into(),
                source_session_id: SessionId::from_uuid(uuid::Uuid::nil()),
                updated_in: SessionPhase::Execution,
            }],
            carried_meeting_decisions: Vec::new(),
            architecture_gap_summary: String::new(),
        }
    }
}
