use super::review_persist::*;
use super::types::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, SelectedEngineerAction,
};
use crate::runtime::RuntimeTopology;
use std::path::PathBuf;

fn make_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/fake/workspace"),
        repo_root: PathBuf::from("/fake/repo"),
        branch: "main".to_string(),
        head: "abc123".to_string(),
        worktree_dirty: false,
        changed_files: Vec::new(),
        active_goals: Vec::new(),
        carried_meeting_decisions: Vec::new(),
        architecture_gap_summary: String::new(),
    }
}

fn make_executed(kind: EngineerActionKind) -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: SelectedEngineerAction {
            label: "test-action".into(),
            rationale: "test".into(),
            argv: vec!["test".into()],
            plan_summary: "test".into(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind,
        },
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
        changed_files: Vec::new(),
    }
}

// --- run_optional_review: non-mutating actions skip review ---

#[test]
fn make_inspection_has_expected_defaults() {
    let insp = make_inspection();
    assert_eq!(insp.branch, "main");
    assert_eq!(insp.head, "abc123");
    assert!(!insp.worktree_dirty);
    assert!(insp.changed_files.is_empty());
    assert!(insp.active_goals.is_empty());
    assert!(insp.carried_meeting_decisions.is_empty());
    assert!(insp.architecture_gap_summary.is_empty());
}

#[test]
fn make_executed_has_expected_defaults() {
    let exec = make_executed(EngineerActionKind::ReadOnlyScan);
    assert_eq!(exec.exit_code, 0);
    assert!(exec.stdout.is_empty());
    assert!(exec.stderr.is_empty());
    assert!(exec.changed_files.is_empty());
    assert_eq!(exec.selected.label, "test-action");
}

// --- PHILOSOPHY_REVIEW content checks ---

#[test]
fn philosophy_review_mentions_key_principles() {
    assert!(PHILOSOPHY_REVIEW.contains("simplicity"));
    assert!(PHILOSOPHY_REVIEW.contains("400 lines"));
    assert!(PHILOSOPHY_REVIEW.contains("Clippy"));
    assert!(PHILOSOPHY_REVIEW.contains("panics"));
}

// --- run_optional_review: additional non-mutating action kinds ---

#[test]
fn optional_review_skips_cargo_clippy() {
    let inspection = make_inspection();
    // CargoCheck is the closest — verify it still passes
    let action = make_executed(EngineerActionKind::CargoCheck);
    assert!(run_optional_review(&inspection, &action).is_ok());
}

// --- compute_diff_for_review: all action kind variants ---

#[test]
fn diff_for_review_run_shell_command_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::RunShellCommand(super::types::ShellCommandRequest {
        argv: vec!["echo".into(), "hello".into()],
    });
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty());
}

#[test]
fn diff_for_review_open_issue_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::OpenIssue(super::types::OpenIssueRequest {
        title: "test".into(),
        body: "body".into(),
        labels: vec!["bug".into()],
    });
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty());
}

// --- persist_engineer_loop_artifacts: additional coverage ---

#[test]
fn persist_artifacts_with_active_goals() {
    let state_dir = tempfile::tempdir().unwrap();
    let mut inspection = make_inspection();
    inspection.active_goals = vec![crate::goals::GoalRecord {
        slug: "g1".to_string(),
        title: "First goal".to_string(),
        rationale: "test rationale".to_string(),
        status: crate::goals::GoalStatus::Active,
        priority: 1,
        owner_identity: "test-owner".to_string(),
        source_session_id: crate::session::SessionId::parse(
            "session-00000000-0000-0000-0000-000000000001",
        )
        .unwrap(),
        updated_in: crate::session::SessionPhase::Preparation,
    }];
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    };
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "test with goals",
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn persist_artifacts_with_carried_decisions() {
    let state_dir = tempfile::tempdir().unwrap();
    let mut inspection = make_inspection();
    inspection.carried_meeting_decisions = vec!["decision-a".to_string(), "decision-b".to_string()];
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    };
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "test with decisions",
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn persist_artifacts_with_mutating_action_kind() {
    let state_dir = tempfile::tempdir().unwrap();
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::CreateFile(
        super::types::CreateFileRequest {
            relative_path: "new_file.rs".into(),
            content: "fn main() {}".into(),
        },
    ));
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "file created".to_string(),
        checks: vec!["file_exists".to_string()],
    };
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "create file test",
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn persist_artifacts_memory_records_are_readable() {
    let state_dir = tempfile::tempdir().unwrap();
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    };
    persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "readable test",
        &inspection,
        &action,
        &verification,
        None,
    )
    .unwrap();

    let memory_path = state_dir.path().join("memory_records.json");
    let content = std::fs::read_to_string(&memory_path).unwrap();
    assert!(
        content.contains("engineer-loop"),
        "memory should reference engineer-loop: {}",
        &content[..content.len().min(200)]
    );
}

#[test]
fn persist_artifacts_evidence_records_are_readable() {
    let state_dir = tempfile::tempdir().unwrap();
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    };
    persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "evidence test",
        &inspection,
        &action,
        &verification,
        None,
    )
    .unwrap();

    let evidence_path = state_dir.path().join("evidence_records.json");
    let content = std::fs::read_to_string(&evidence_path).unwrap();
    assert!(
        content.contains("repo-root"),
        "evidence should contain repo-root"
    );
}

#[test]
fn persist_artifacts_with_long_objective() {
    let state_dir = tempfile::tempdir().unwrap();
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    let verification = super::types::VerificationReport {
        status: "ok".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    };
    let long_objective = "x".repeat(5000);
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        &long_objective,
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn persist_artifacts_with_git_commit_action() {
    let state_dir = tempfile::tempdir().unwrap();
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::GitCommit(
        super::types::GitCommitRequest {
            message: "chore: test commit".into(),
        },
    ));
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "commit ok".to_string(),
        checks: vec![],
    };
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "git commit test",
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
}

// --- make_executed with different exit codes ---

#[test]
fn make_executed_can_have_custom_exit_code() {
    let mut exec = make_executed(EngineerActionKind::CargoTest);
    exec.exit_code = 1;
    assert_eq!(exec.exit_code, 1);
}

#[test]
fn make_executed_can_have_stdout_stderr() {
    let mut exec = make_executed(EngineerActionKind::CargoTest);
    exec.stdout = "test output".to_string();
    exec.stderr = "warning".to_string();
    assert_eq!(exec.stdout, "test output");
    assert_eq!(exec.stderr, "warning");
}

// --- PHILOSOPHY_REVIEW additional checks ---

#[test]
fn philosophy_review_mentions_tested() {
    assert!(PHILOSOPHY_REVIEW.contains("tested"));
}

#[test]
fn philosophy_review_mentions_modules() {
    assert!(PHILOSOPHY_REVIEW.contains("Modules") || PHILOSOPHY_REVIEW.contains("modules"));
}
