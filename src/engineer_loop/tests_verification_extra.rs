use super::verification::*;
use std::path::{Path, PathBuf};

use super::types::{
    AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
    GitCommitRequest, OpenIssueRequest, RepoInspection, SelectedEngineerAction,
    ShellCommandRequest, StructuredEditRequest,
};

fn make_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/workspace"),
        repo_root: PathBuf::from("/workspace"),
        branch: "main".to_string(),
        head: "abc123".to_string(),
        worktree_dirty: false,
        changed_files: Vec::new(),
        active_goals: Vec::new(),
        carried_meeting_decisions: Vec::new(),
        architecture_gap_summary: String::new(),
    }
}

fn make_selected(kind: EngineerActionKind, label: &str) -> SelectedEngineerAction {
    SelectedEngineerAction {
        label: label.to_string(),
        rationale: "test rationale".to_string(),
        argv: vec![],
        plan_summary: "test plan".to_string(),
        verification_steps: vec![],
        expected_changed_files: vec![],
        kind,
    }
}

fn make_executed(kind: EngineerActionKind, label: &str, exit_code: i32) -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: make_selected(kind, label),
        exit_code,
        stdout: String::new(),
        stderr: String::new(),
        changed_files: vec![],
    }
}

#[test]
fn test_verify_engineer_action_nonzero_exit() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::CargoTest, "cargo-test", 1);
    let result = verify_engineer_action(&inspection, &action, Path::new("/state"));
    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("exited with code 1"));
}

#[test]
fn test_verify_worktree_state_non_mutating_unchanged() {
    let inspection = make_inspection();
    let post = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan, "scan", 0);
    let mut checks = Vec::new();

    let result = verify_worktree_state(&inspection, &action, &post, &mut checks);
    assert!(result.is_ok());
    assert!(checks.iter().any(|c| c.contains("worktree-dirty=")));
}

#[test]
fn test_verify_worktree_state_non_mutating_changed_files_error() {
    let inspection = make_inspection();
    let mut post = make_inspection();
    post.changed_files = vec!["unexpected.rs".to_string()];
    let action = make_executed(EngineerActionKind::CargoCheck, "cargo-check", 0);
    let mut checks = Vec::new();

    let result = verify_worktree_state(&inspection, &action, &post, &mut checks);
    assert!(result.is_err());
}

#[test]
fn test_verify_worktree_state_mutating_not_dirty_error() {
    let inspection = make_inspection();
    let post = make_inspection(); // still clean
    let action = make_executed(
        EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "new.rs".to_string(),
            content: "fn main() {}".to_string(),
        }),
        "create-file",
        0,
    );
    let mut checks = Vec::new();

    let result = verify_worktree_state(&inspection, &action, &post, &mut checks);
    assert!(result.is_err());
}

#[test]
fn test_verify_worktree_state_active_goals_changed_error() {
    let inspection = make_inspection();
    let mut post = make_inspection();
    post.active_goals = vec![crate::goals::GoalRecord {
        slug: "new-goal".to_string(),
        title: "New Goal".to_string(),
        rationale: "test".to_string(),
        status: crate::goals::GoalStatus::Active,
        priority: 1,
        owner_identity: "test".to_string(),
        source_session_id: crate::session::SessionId::parse(
            "session-00000000-0000-0000-0000-000000000001",
        )
        .unwrap(),
        updated_in: crate::session::SessionPhase::Intake,
    }];
    let action = make_executed(EngineerActionKind::CargoTest, "cargo-test", 0);
    let mut checks = Vec::new();

    let result = verify_worktree_state(&inspection, &action, &post, &mut checks);
    assert!(result.is_err());
}

#[test]
fn test_verify_worktree_state_meeting_decisions_changed_error() {
    let inspection = make_inspection();
    let mut post = make_inspection();
    post.carried_meeting_decisions = vec!["new decision".to_string()];
    let action = make_executed(EngineerActionKind::CargoTest, "cargo-test", 0);
    let mut checks = Vec::new();

    let result = verify_worktree_state(&inspection, &action, &post, &mut checks);
    assert!(result.is_err());
}

#[test]
fn test_verify_kind_specific_shell_command() {
    let inspection = make_inspection();
    let action = make_executed(
        EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["echo".to_string(), "hello".to_string()],
        }),
        "run-shell",
        0,
    );
    let mut checks = Vec::new();

    let result = verify_kind_specific(&inspection, &action, &mut checks);
    assert!(result.is_ok());
    assert!(
        checks
            .iter()
            .any(|c| c.starts_with("shell-command-exit-code="))
    );
}

#[test]
fn test_verify_kind_specific_git_commit() {
    let inspection = make_inspection();
    let action = make_executed(
        EngineerActionKind::GitCommit(GitCommitRequest {
            message: "fix: something".to_string(),
        }),
        "git-commit",
        0,
    );
    let mut checks = Vec::new();

    let result = verify_kind_specific(&inspection, &action, &mut checks);
    assert!(result.is_ok());
    assert!(checks.iter().any(|c| c.contains("git-commit-created=true")));
}

#[test]
fn test_verify_kind_specific_read_only_scan_missing_label() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan, "unknown-scan", 0);
    let mut checks = Vec::new();

    let result = verify_kind_specific(&inspection, &action, &mut checks);
    assert!(result.is_err());
}

#[test]
fn test_verify_kind_specific_git_tracked_file_scan_empty_output() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan, "git-tracked-file-scan", 0);
    let mut checks = Vec::new();

    let result = verify_kind_specific(&inspection, &action, &mut checks);
    assert!(result.is_err());
}

#[test]
fn test_verify_kind_specific_git_tracked_file_scan_with_output() {
    let inspection = make_inspection();
    let mut action = make_executed(EngineerActionKind::ReadOnlyScan, "git-tracked-file-scan", 0);
    action.stdout = "src/main.rs\nsrc/lib.rs\n".to_string();
    let mut checks = Vec::new();

    let result = verify_kind_specific(&inspection, &action, &mut checks);
    assert!(result.is_ok());
    assert!(
        checks
            .iter()
            .any(|c| c.contains("tracked-files-present=true"))
    );
}

// === Issue #1209: branch-rename within engineer/* namespace ===

#[test]
fn rename_within_engineer_allows_engineer_to_engineer() {
    // engineer/foo -> engineer/bar should be legitimate (the engineer LLM
    // sometimes does `git checkout -b engineer/<better-name>` mid-cycle).
    assert!(rename_within_engineer_namespace(
        "engineer/foo-1234",
        "engineer/expand-gym-scenarios-wave7"
    ));
}

#[test]
fn rename_within_engineer_rejects_engineer_to_main() {
    // engineer/foo -> main is a real failure (escaped the sandbox).
    assert!(!rename_within_engineer_namespace(
        "engineer/foo-1234",
        "main"
    ));
}

#[test]
fn rename_within_engineer_rejects_main_to_engineer() {
    // main -> engineer/foo means we weren't on an engineer branch to begin
    // with; the strict checks should still apply.
    assert!(!rename_within_engineer_namespace(
        "main",
        "engineer/foo-1234"
    ));
}

#[test]
fn rename_within_engineer_allows_same_branch() {
    // engineer/foo -> engineer/foo (no rename) trivially passes.
    assert!(rename_within_engineer_namespace(
        "engineer/foo-1234",
        "engineer/foo-1234"
    ));
}

#[test]
fn rename_within_engineer_rejects_unrelated_branches() {
    assert!(!rename_within_engineer_namespace("feature/x", "develop"));
}
