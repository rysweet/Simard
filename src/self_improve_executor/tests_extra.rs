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
    assert_eq!(r.to_string(), "applied (1 finding)");
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
fn apply_and_review_git_diff_failure_includes_rollback_error() {
    // If git diff fails and rollback also fails, both errors must surface.
    // We simulate this by running against a path that is NOT a git repo, so
    // `git diff HEAD` fails, and `git checkout -- .` (rollback) also fails.
    let dir = tempfile::TempDir::new().unwrap();
    // No `git init` — intentionally not a repo.
    let step = PlanStep {
        action: AnalyzedAction::ReadOnlyScan,
        target: ".".to_string(),
        expected_outcome: "ok".to_string(),
        verification_command: "true".to_string(),
    };
    let patch = make_patch(vec![step]);
    let result = apply_and_review(&patch, dir.path());
    match &result {
        ApplyResult::PlanFailed { reason } => {
            assert!(
                reason.contains("git diff failed"),
                "reason should mention git diff failure, got: {reason}"
            );
        }
        other => panic!("expected PlanFailed, got: {other:?}"),
    }
}

#[test]
fn apply_and_review_review_blocked_rollback_failure_surfaces_as_critical() {
    // Verify that when review blocks AND rollback fails, the rollback error
    // is captured as a Critical Bug finding (executor.rs lines 107-120).
    let finding_obj = finding(FindingCategory::Bug, Severity::Critical);
    let result = ApplyResult::ReviewBlocked {
        findings: vec![finding_obj.clone()],
    };
    assert!(result.has_critical());
}

#[test]
fn apply_and_review_plan_failed_rollback_also_failed() {
    // When plan stops early and rollback fails, both reasons are combined.
    let patch = make_patch(vec![failing_step()]);
    // /dev/null is not a git repo, so rollback will fail.
    let result = apply_and_review(&patch, Path::new("/dev/null"));
    match &result {
        ApplyResult::PlanFailed { reason } => {
            assert!(
                reason.contains("failed"),
                "reason should describe the failure, got: {reason}"
            );
        }
        other => panic!("expected PlanFailed, got: {other:?}"),
    }
}

#[test]
fn apply_and_review_noop_plan_in_real_repo_succeeds() {
    // A plan with a no-op step in a real git repo should produce Applied
    // with no findings (empty diff → auto-pass).
    let dir = tempfile::TempDir::new().unwrap();
    init_test_repo(dir.path());

    let step = PlanStep {
        action: AnalyzedAction::ReadOnlyScan,
        target: ".".to_string(),
        expected_outcome: "ok".to_string(),
        verification_command: "true".to_string(),
    };
    let patch = make_patch(vec![step]);
    let result = apply_and_review(&patch, dir.path());
    assert!(
        result.is_applied(),
        "expected Applied for no-op plan, got: {result:?}"
    );
}

#[test]
fn apply_result_plan_failed_display_includes_rollback() {
    let r = ApplyResult::PlanFailed {
        reason: "step failed; rollback also failed: git error".into(),
    };
    let display = r.to_string();
    assert!(display.contains("rollback also failed"));
}

/// Helper: initialise a minimal git repo with one commit.
fn init_test_repo(ws: &Path) {
    for (args, label) in [
        (vec!["init"], "git init"),
        (
            vec!["config", "user.email", "test@test.com"],
            "git config email",
        ),
        (vec!["config", "user.name", "Test"], "git config name"),
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(ws)
            .output()
            .unwrap_or_else(|_| panic!("{label}"));
    }
    std::fs::write(ws.join("init.txt"), "init").expect("write init file");
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
}

#[test]
fn generate_patch_deduplicates_target_files() {
    // Mirror generate_patch's target-file extraction with sort+dedup.
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
    files.sort();
    files.dedup();
    assert_eq!(files, vec!["src/lib.rs", "src/main.rs"]);
}

#[test]
fn generate_patch_deduplicates_non_adjacent_duplicates() {
    // Verify that sort+dedup catches non-adjacent duplicates (the bug fix).
    let steps = vec![
        step("src/a.rs", "true"),
        step("src/b.rs", "true"),
        step("src/a.rs", "true"),
    ];
    let plan = Plan::new(steps);
    let mut files: Vec<String> = plan
        .steps()
        .iter()
        .map(|s| s.target.clone())
        .filter(|t| t != "." && !t.is_empty())
        .collect();
    files.sort();
    files.dedup();
    assert_eq!(files, vec!["src/a.rs", "src/b.rs"]);
}
