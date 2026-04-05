use std::path::Path;

use super::git_ops::rollback;
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
        Err(SimardError::PlanningUnavailable { .. }) => {
            // Any PlanningUnavailable is correct — whether from open() or run_turn().
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
    // A plan with only no-op steps produces no diff.
    // Use a real temp git repo so `git diff HEAD` works.
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

    let step = PlanStep {
        action: AnalyzedAction::ReadOnlyScan,
        target: ".".to_string(),
        expected_outcome: "ok".to_string(),
        verification_command: "true".to_string(),
    };
    let patch = make_patch(vec![step]);
    let result = apply_and_review(&patch, dir.path());
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

#[test]
fn rollback_cleans_untracked_files() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let ws = tmp.path();

    // Initialise a git repo with one committed file.
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(ws)
        .output()
        .expect("git init");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(ws)
        .output()
        .expect("git config email");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(ws)
        .output()
        .expect("git config name");
    std::fs::write(ws.join("committed.txt"), "original").expect("write");
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(ws)
        .output()
        .expect("git add");
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(ws)
        .output()
        .expect("git commit");

    // Simulate plan-created artefacts: modify tracked file + create untracked file.
    std::fs::write(ws.join("committed.txt"), "modified").expect("write");
    std::fs::write(ws.join("untracked.txt"), "new file").expect("write");

    rollback(ws).expect("rollback should succeed");

    // Tracked file restored.
    let contents = std::fs::read_to_string(ws.join("committed.txt")).expect("read");
    assert_eq!(contents, "original");

    // Untracked file removed.
    assert!(
        !ws.join("untracked.txt").exists(),
        "rollback should remove untracked files"
    );
}

#[test]
fn apply_result_display_applied() {
    let r = ApplyResult::Applied {
        findings: vec![finding(FindingCategory::Bug, Severity::Low)],
    };
    assert_eq!(r.to_string(), "applied (1 findings)");
}

#[test]
fn apply_result_display_review_blocked() {
    let r = ApplyResult::ReviewBlocked {
        findings: Vec::new(),
    };
    assert_eq!(r.to_string(), "review-blocked (0 findings)");
}

#[test]
fn apply_result_display_plan_failed() {
    let r = ApplyResult::PlanFailed {
        reason: "boom".into(),
    };
    assert_eq!(r.to_string(), "plan-failed: boom");
}

#[test]
fn apply_result_display_commit_failed() {
    let r = ApplyResult::CommitFailed {
        reason: "git error".into(),
        findings: vec![
            finding(FindingCategory::Bug, Severity::High),
            finding(FindingCategory::Security, Severity::Critical),
        ],
    };
    assert_eq!(r.to_string(), "commit-failed: git error (2 findings)");
}

#[test]
fn generate_patch_deduplicates_target_files() {
    // Mirror generate_patch's target-file extraction with dedup.
    let steps = vec![
        step("src/lib.rs", "true"),
        step("src/lib.rs", "true"),
        step("src/main.rs", "true"),
        step(".", "true"),
        step("", "true"),
    ];
    let plan = Plan::new(steps);
    let mut files: Vec<String> = plan
        .steps()
        .iter()
        .map(|s| s.target.clone())
        .filter(|t| t != "." && !t.is_empty())
        .collect();
    files.dedup();
    // "." and "" are filtered; consecutive "src/lib.rs" deduped
    assert_eq!(files, vec!["src/lib.rs", "src/main.rs"]);
}
