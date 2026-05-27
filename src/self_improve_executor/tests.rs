use std::path::Path;

use super::*;
use crate::error::SimardError;
use crate::review_pipeline::{FindingCategory, Severity};

fn make_patch(outcome_summary: &str) -> ImprovementPatch {
    ImprovementPatch {
        description: "test improvement".into(),
        target_files: vec!["src/lib.rs".into()],
        outcome_summary: outcome_summary.to_string(),
        review_findings: Vec::new(),
    }
}

fn finding(category: FindingCategory, severity: Severity) -> crate::review_pipeline::ReviewFinding {
    crate::review_pipeline::ReviewFinding {
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

    let commit_failed = ApplyResult::CommitFailed {
        reason: "git error".into(),
        findings: Vec::new(),
    };
    assert!(!commit_failed.is_applied());
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
fn apply_result_commit_failed_with_critical() {
    let result = ApplyResult::CommitFailed {
        reason: "git error".into(),
        findings: vec![finding(FindingCategory::Bug, Severity::Critical)],
    };
    assert!(result.has_critical());
}

#[test]
fn apply_result_commit_failed_without_critical() {
    let result = ApplyResult::CommitFailed {
        reason: "git error".into(),
        findings: vec![finding(FindingCategory::Bug, Severity::Low)],
    };
    assert!(!result.has_critical());
}

#[test]
fn improvement_patch_construction() {
    let patch = make_patch("agent completed successfully");
    assert_eq!(patch.description, "test improvement");
    assert_eq!(patch.target_files, vec!["src/lib.rs"]);
    assert_eq!(patch.outcome_summary, "agent completed successfully");
    assert!(patch.review_findings.is_empty());
}

#[test]
fn generate_patch_without_api_key_returns_unavailable() {
    // Force RustyClawd provider without ANTHROPIC_API_KEY → session may open
    // but run_turn will fail.
    unsafe {
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::set_var("SIMARD_LLM_PROVIDER", "rustyclawd");
    };
    let inspection = test_inspection();
    let result = generate_patch("improve error handling", &inspection);
    unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };
    match result {
        Err(SimardError::ActionExecutionFailed { .. }) => {
            // Any ActionExecutionFailed is correct — whether from open() or run_turn().
        }
        other => panic!("expected ActionExecutionFailed, got: {other:?}"),
    }
}

#[test]
#[serial_test::serial]
fn apply_and_review_empty_diff_is_applied() {
    // The agent ran but produced no diff → Applied with no findings.
    let dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Configure git user for CI environments where no global config exists.
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let commit_out = std::process::Command::new("git")
        .args(["commit", "--allow-empty", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        commit_out.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    let patch = make_patch("agent ran no-op");
    let result = apply_and_review(&patch, dir.path());
    assert!(result.is_applied());
}

#[test]
fn run_autonomous_improvement_empty_proposals() {
    let inspection = test_inspection();
    let results = run_autonomous_improvement(
        &[],
        Path::new("/tmp"),
        &inspection,
        &ApprovalPolicy::AutoApproveWithAuditTrail {
            justification: "test".to_string(),
        },
    );
    assert!(results.is_empty());
}

#[test]
fn run_autonomous_improvement_planning_unavailable() {
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    let inspection = test_inspection();
    let proposals = vec!["improve X".to_string(), "improve Y".to_string()];
    let results = run_autonomous_improvement(
        &proposals,
        Path::new("/tmp"),
        &inspection,
        &ApprovalPolicy::AutoApproveWithAuditTrail {
            justification: "test".to_string(),
        },
    );
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
    let results = run_autonomous_improvement(
        &proposals,
        Path::new("/tmp"),
        &inspection,
        &ApprovalPolicy::AutoApproveWithAuditTrail {
            justification: "test".to_string(),
        },
    );
    // All three should be attempted (PlanFailed is not a critical ReviewBlocked)
    assert_eq!(results.len(), 3);
}

fn test_inspection() -> crate::engineer_loop::RepoInspection {
    use crate::goals::{GoalRecord, GoalStatus};
    use crate::session::{SessionId, SessionPhase};
    crate::engineer_loop::RepoInspection {
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

// ── Approval gate tests (spec lines 695/700, issue #2097) ───────

#[test]
fn approval_gate_blocks_unapproved_execution() {
    let inspection = test_inspection();
    let proposals = vec!["improve X".to_string(), "improve Y".to_string()];
    let results = run_autonomous_improvement(
        &proposals,
        Path::new("/tmp"),
        &inspection,
        &ApprovalPolicy::RequireOperatorApproval,
    );
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(
            matches!(r, ApplyResult::ApprovalRequired { .. }),
            "default policy must block execution, got: {r:?}"
        );
    }
}

#[test]
fn approval_gate_default_policy_is_require_approval() {
    assert_eq!(
        ApprovalPolicy::default(),
        ApprovalPolicy::RequireOperatorApproval,
        "default ApprovalPolicy must require operator approval per spec"
    );
}

#[test]
fn approval_required_is_not_applied() {
    let r = ApplyResult::ApprovalRequired {
        proposal: "test".to_string(),
    };
    assert!(!r.is_applied());
}

#[test]
fn approval_required_has_no_critical_findings() {
    let r = ApplyResult::ApprovalRequired {
        proposal: "test".to_string(),
    };
    assert!(!r.has_critical());
}

#[test]
fn approval_required_has_empty_findings() {
    let r = ApplyResult::ApprovalRequired {
        proposal: "test".to_string(),
    };
    assert!(r.findings().is_empty());
}

#[test]
fn approval_required_display() {
    let r = ApplyResult::ApprovalRequired {
        proposal: "fix cache".to_string(),
    };
    assert_eq!(r.to_string(), "approval-required: fix cache");
}

#[test]
fn approval_policy_display_require() {
    assert_eq!(
        ApprovalPolicy::RequireOperatorApproval.to_string(),
        "require-operator-approval"
    );
}

#[test]
fn approval_policy_display_auto() {
    let p = ApprovalPolicy::AutoApproveWithAuditTrail {
        justification: "sandbox test".to_string(),
    };
    assert_eq!(p.to_string(), "auto-approve (justification: sandbox test)");
}

#[test]
fn auto_approve_policy_allows_execution() {
    // When auto-approve is set, proposals should NOT get ApprovalRequired.
    // They may still fail for other reasons (no LLM key), but the gate should pass.
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    let inspection = test_inspection();
    let proposals = vec!["improve Z".to_string()];
    let results = run_autonomous_improvement(
        &proposals,
        Path::new("/tmp"),
        &inspection,
        &ApprovalPolicy::AutoApproveWithAuditTrail {
            justification: "test run".to_string(),
        },
    );
    assert_eq!(results.len(), 1);
    // Should be PlanFailed (no LLM) — NOT ApprovalRequired
    assert!(
        matches!(&results[0], ApplyResult::PlanFailed { .. }),
        "auto-approve policy should allow execution past the gate, got: {:?}",
        results[0]
    );
}
