use super::verification::*;
use std::path::PathBuf;

use super::types::{
    AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
    GitCommitRequest, OpenIssueRequest, RepoInspection, SelectedEngineerAction,
    ShellCommandRequest, StructuredEditRequest,
};

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

fn make_selected(label: &str, kind: EngineerActionKind) -> SelectedEngineerAction {
    SelectedEngineerAction {
        label: label.to_string(),
        rationale: "test".to_string(),
        argv: vec!["test".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind,
    }
}

fn make_executed(
    label: &str,
    kind: EngineerActionKind,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: make_selected(label, kind),
        exit_code,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        changed_files: Vec::new(),
    }
}

// --- verify_cargo_test ---

#[test]
fn cargo_test_pass_with_result_line() {
    let action = make_executed(
        "cargo-test",
        EngineerActionKind::CargoTest,
        0,
        "test result: ok. 10 passed; 0 failed",
        "",
    );
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    assert!(checks.contains(&"cargo-test-result-present=true".to_string()));
    assert!(checks.contains(&"cargo-test-passed=true".to_string()));
}

#[test]
fn cargo_test_fail_with_result_line() {
    let action = make_executed(
        "cargo-test",
        EngineerActionKind::CargoTest,
        1,
        "test result: FAILED. 5 passed; 2 failed",
        "",
    );
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    assert!(checks.contains(&"cargo-test-passed=false".to_string()));
}

#[test]
fn cargo_test_result_in_stderr_also_detected() {
    let action = make_executed(
        "cargo-test",
        EngineerActionKind::CargoTest,
        0,
        "",
        "test result: ok. 3 passed",
    );
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    assert!(checks.contains(&"cargo-test-result-present=true".to_string()));
}

#[test]
fn cargo_test_no_output_exit_zero_still_passes() {
    let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 0, "", "");
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    assert!(checks.iter().any(|c| c.contains("cargo-test-passed=true")));
}

#[test]
fn cargo_test_no_output_nonzero_fails() {
    let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 1, "", "");
    let mut checks = Vec::new();
    let err = verify_cargo_test(&action, &mut checks).unwrap_err();
    assert!(err.to_string().contains("no recognizable test result"));
}

#[test]
fn cargo_test_exit_nonzero_with_result_marks_failed() {
    let action = make_executed(
        "cargo-test",
        EngineerActionKind::CargoTest,
        101,
        "test result: ok. 10 passed; 0 failed",
        "",
    );
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    // Non-zero exit code overrides "ok" in output
    assert!(checks.contains(&"cargo-test-passed=false".to_string()));
}

// --- verify_cargo_check ---

#[test]
fn cargo_check_pass() {
    let action = make_executed("cargo-check", EngineerActionKind::CargoCheck, 0, "", "");
    let mut checks = Vec::new();
    verify_cargo_check(&action, &mut checks);
    assert!(checks.contains(&"cargo-check-passed=true".to_string()));
}

#[test]
fn cargo_check_fail_counts_error_lines() {
    let action = make_executed(
        "cargo-check",
        EngineerActionKind::CargoCheck,
        1,
        "",
        "error[E0308]: mismatched types\nerror: aborting due to previous error",
    );
    let mut checks = Vec::new();
    verify_cargo_check(&action, &mut checks);
    assert!(checks[0].contains("cargo-check-passed=false"));
    assert!(checks[0].contains("errors=2"));
}

#[test]
fn cargo_check_fail_zero_errors_in_stderr() {
    let action = make_executed(
        "cargo-check",
        EngineerActionKind::CargoCheck,
        1,
        "",
        "warning: unused variable\n",
    );
    let mut checks = Vec::new();
    verify_cargo_check(&action, &mut checks);
    assert!(checks[0].contains("errors=0"));
}

// --- verify_open_issue ---

#[test]
fn open_issue_with_github_url() {
    let action = make_executed(
        "open-issue",
        EngineerActionKind::OpenIssue(OpenIssueRequest {
            title: "test".into(),
            body: String::new(),
            labels: Vec::new(),
        }),
        0,
        "https://github.com/user/repo/issues/42",
        "",
    );
    let mut checks = Vec::new();
    verify_open_issue(&action, &mut checks).unwrap();
    assert!(checks.contains(&"issue-url-present=true".to_string()));
}

#[test]
fn open_issue_with_github_dot_com() {
    let action = make_executed(
        "open-issue",
        EngineerActionKind::OpenIssue(OpenIssueRequest {
            title: "t".into(),
            body: String::new(),
            labels: Vec::new(),
        }),
        0,
        "Created issue at github.com/repo/issues/1",
        "",
    );
    let mut checks = Vec::new();
    verify_open_issue(&action, &mut checks).unwrap();
    assert!(checks.contains(&"issue-url-present=true".to_string()));
}

#[test]
fn open_issue_without_url_fails() {
    let action = make_executed(
        "open-issue",
        EngineerActionKind::OpenIssue(OpenIssueRequest {
            title: "t".into(),
            body: String::new(),
            labels: Vec::new(),
        }),
        0,
        "no url here",
        "",
    );
    let mut checks = Vec::new();
    let err = verify_open_issue(&action, &mut checks).unwrap_err();
    assert!(err.to_string().contains("did not return an issue URL"));
}

// --- build_verification_summary ---

#[test]
fn summary_read_only_scan() {
    let action = make_executed("my-scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
    let s = build_verification_summary(&action);
    assert!(s.contains("my-scan"));
    assert!(s.contains("Verified local-only"));
}

#[test]
fn summary_cargo_test_pass() {
    let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 0, "", "");
    assert!(build_verification_summary(&action).contains("passed"));
}

#[test]
fn summary_cargo_test_fail() {
    let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 1, "", "");
    assert!(build_verification_summary(&action).contains("failed"));
}

#[test]
fn summary_cargo_check_pass() {
    let action = make_executed("cargo-check", EngineerActionKind::CargoCheck, 0, "", "");
    assert!(build_verification_summary(&action).contains("succeeded"));
}

#[test]
fn summary_cargo_check_fail() {
    let action = make_executed("cargo-check", EngineerActionKind::CargoCheck, 1, "", "");
    assert!(build_verification_summary(&action).contains("failed"));
}

#[test]
fn summary_structured_text_replace_mentions_path() {
    let action = make_executed(
        "edit",
        EngineerActionKind::StructuredTextReplace(StructuredEditRequest {
            relative_path: "src/lib.rs".into(),
            search: "a".into(),
            replacement: "b".into(),
            verify_contains: "b".into(),
        }),
        0,
        "",
        "",
    );
    assert!(build_verification_summary(&action).contains("src/lib.rs"));
}

#[test]
fn summary_create_file_mentions_path() {
    let action = make_executed(
        "create-file",
        EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "foo.txt".into(),
            content: "c".into(),
        }),
        0,
        "",
        "",
    );
    assert!(build_verification_summary(&action).contains("foo.txt"));
}

#[test]
fn summary_append_to_file_mentions_path() {
    let action = make_executed(
        "append",
        EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "c".into(),
        }),
        0,
        "",
        "",
    );
    assert!(build_verification_summary(&action).contains("log.txt"));
}

#[test]
fn summary_run_shell_command() {
    let action = make_executed(
        "run",
        EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["cargo".into(), "fmt".into()],
        }),
        0,
        "",
        "",
    );
    assert!(build_verification_summary(&action).contains("RunShellCommand"));
}

#[test]
fn summary_git_commit() {
    let action = make_executed(
        "git-commit",
        EngineerActionKind::GitCommit(GitCommitRequest {
            message: "m".into(),
        }),
        0,
        "",
        "",
    );
    assert!(build_verification_summary(&action).contains("GitCommit"));
}

#[test]
fn summary_open_issue() {
    let action = make_executed(
        "open-issue",
        EngineerActionKind::OpenIssue(OpenIssueRequest {
            title: "t".into(),
            body: String::new(),
            labels: Vec::new(),
        }),
        0,
        "",
        "",
    );
    assert!(build_verification_summary(&action).contains("OpenIssue"));
}

// --- verify_engineer_action: non-zero exit code early rejection ---

#[test]
fn verify_action_nonzero_exit_code_rejected() {
    let inspection = make_inspection();
    let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 1, "", "");
    let state_root = tempfile::tempdir().unwrap();
    let err = verify_engineer_action(&inspection, &action, state_root.path()).unwrap_err();
    assert!(err.to_string().contains("exited with code 1"));
}

// --- verify_kind_specific ---

#[test]
fn kind_specific_read_only_unknown_label_rejected() {
    let action = make_executed("unknown-scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
    let mut checks = Vec::new();
    let err = verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap_err();
    assert!(err.to_string().contains("verification rules are missing"));
}

#[test]
fn kind_specific_git_tracked_file_scan_empty_fails() {
    let action = make_executed(
        "git-tracked-file-scan",
        EngineerActionKind::ReadOnlyScan,
        0,
        "",
        "",
    );
    let mut checks = Vec::new();
    let err = verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap_err();
    assert!(err.to_string().contains("no tracked files"));
}

#[test]
fn kind_specific_git_tracked_file_scan_with_files_ok() {
    let action = make_executed(
        "git-tracked-file-scan",
        EngineerActionKind::ReadOnlyScan,
        0,
        "README.md\nsrc/lib.rs\n",
        "",
    );
    let mut checks = Vec::new();
    verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap();
    assert!(checks.contains(&"tracked-files-present=true".to_string()));
}
