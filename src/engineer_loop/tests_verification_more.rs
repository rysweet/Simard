use super::types::{
    AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
    GitCommitRequest, OpenIssueRequest, RepoInspection, SelectedEngineerAction,
    ShellCommandRequest, StructuredEditRequest,
};
use super::verification::*;
use std::path::{Path, PathBuf};
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
#[test]
fn kind_specific_shell_command_records_exit_code() {
    let action = make_executed(
        "run-shell-command",
        EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["cargo".into(), "fmt".into()],
        }),
        0,
        "",
        "",
    );
    let mut checks = Vec::new();
    verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap();
    assert!(checks.contains(&"shell-command-exit-code=0".to_string()));
}
#[test]
fn kind_specific_git_commit_records_created() {
    let action = make_executed(
        "git-commit",
        EngineerActionKind::GitCommit(GitCommitRequest {
            message: "m".into(),
        }),
        0,
        "",
        "",
    );
    let mut checks = Vec::new();
    verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap();
    assert!(checks.contains(&"git-commit-created=true".to_string()));
}
// --- verify_create_file ---
#[test]
fn create_file_correct_content_passes() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = RepoInspection {
        repo_root: dir.path().to_path_buf(),
        ..make_inspection()
    };
    std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
    let req = CreateFileRequest {
        relative_path: "test.txt".into(),
        content: "hello".into(),
    };
    let mut checks = Vec::new();
    verify_create_file(&inspection, &req, &mut checks).unwrap();
    assert!(checks.contains(&"file-exists=test.txt".to_string()));
    assert!(checks.contains(&"file-content-matches=true".to_string()));
}
#[test]
fn create_file_missing_fails() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = RepoInspection {
        repo_root: dir.path().to_path_buf(),
        ..make_inspection()
    };
    let req = CreateFileRequest {
        relative_path: "nonexistent.txt".into(),
        content: "x".into(),
    };
    let mut checks = Vec::new();
    let err = verify_create_file(&inspection, &req, &mut checks).unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}
#[test]
fn create_file_content_mismatch_fails() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = RepoInspection {
        repo_root: dir.path().to_path_buf(),
        ..make_inspection()
    };
    std::fs::write(dir.path().join("test.txt"), "wrong").unwrap();
    let req = CreateFileRequest {
        relative_path: "test.txt".into(),
        content: "expected".into(),
    };
    let mut checks = Vec::new();
    let err = verify_create_file(&inspection, &req, &mut checks).unwrap_err();
    assert!(err.to_string().contains("content does not match"));
}
// --- verify_append_to_file ---
#[test]
fn append_to_file_contains_content_passes() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = RepoInspection {
        repo_root: dir.path().to_path_buf(),
        ..make_inspection()
    };
    std::fs::write(dir.path().join("log.txt"), "old\nappended text\n").unwrap();
    let req = AppendToFileRequest {
        relative_path: "log.txt".into(),
        content: "appended text".into(),
    };
    let mut checks = Vec::new();
    verify_append_to_file(&inspection, &req, &mut checks).unwrap();
    assert!(checks.contains(&"file-contains-appended=log.txt".to_string()));
}
#[test]
fn append_to_file_missing_content_fails() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = RepoInspection {
        repo_root: dir.path().to_path_buf(),
        ..make_inspection()
    };
    std::fs::write(dir.path().join("log.txt"), "only old\n").unwrap();
    let req = AppendToFileRequest {
        relative_path: "log.txt".into(),
        content: "missing text".into(),
    };
    let mut checks = Vec::new();
    let err = verify_append_to_file(&inspection, &req, &mut checks).unwrap_err();
    assert!(err.to_string().contains("does not contain the appended"));
}
#[test]
fn append_to_file_nonexistent_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = RepoInspection {
        repo_root: dir.path().to_path_buf(),
        ..make_inspection()
    };
    let req = AppendToFileRequest {
        relative_path: "missing.txt".into(),
        content: "x".into(),
    };
    let mut checks = Vec::new();
    let err = verify_append_to_file(&inspection, &req, &mut checks).unwrap_err();
    assert!(err.to_string().contains("could not read"));
}
// --- verify_worktree_state ---
#[test]
fn worktree_state_read_only_changed_rejected() {
    let inspection = make_inspection();
    let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
    let mut post = make_inspection();
    post.worktree_dirty = true;
    let mut checks = Vec::new();
    let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
    assert!(err.to_string().contains("worktree state changed"));
}
#[test]
fn worktree_state_read_only_stable_ok() {
    let inspection = make_inspection();
    let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
    let post = make_inspection();
    let mut checks = Vec::new();
    verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap();
    assert!(checks.iter().any(|c| c.contains("worktree-dirty=")));
}
#[test]
fn worktree_state_mutating_still_clean_rejected() {
    let inspection = make_inspection();
    let mut action = make_executed(
        "create-file",
        EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "foo.txt".into(),
            content: "c".into(),
        }),
        0,
        "",
        "",
    );
    action.selected.expected_changed_files = vec!["foo.txt".into()];
    action.changed_files = vec!["foo.txt".into()];
    let post = make_inspection(); // worktree_dirty=false
    let mut checks = Vec::new();
    let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
    assert!(err.to_string().contains("still appears clean"));
}
#[test]
fn worktree_state_mutating_unexpected_files_rejected() {
    let inspection = make_inspection();
    let mut action = make_executed(
        "create-file",
        EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "foo.txt".into(),
            content: "c".into(),
        }),
        0,
        "",
        "",
    );
    action.selected.expected_changed_files = vec!["foo.txt".into()];
    action.changed_files = vec!["foo.txt".into()];
    let mut post = make_inspection();
    post.worktree_dirty = true;
    post.changed_files = vec!["bar.txt".into()];
    let mut checks = Vec::new();
    let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
    assert!(err.to_string().contains("changed unexpected files"));
}
#[test]
fn worktree_state_mutating_action_reported_wrong_files() {
    let inspection = make_inspection();
    let mut action = make_executed(
        "create-file",
        EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "foo.txt".into(),
            content: "c".into(),
        }),
        0,
        "",
        "",
    );
    action.selected.expected_changed_files = vec!["foo.txt".into()];
    action.changed_files = vec!["other.txt".into()]; // mismatch
    let mut post = make_inspection();
    post.worktree_dirty = true;
    post.changed_files = vec!["foo.txt".into()];
    let mut checks = Vec::new();
    let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
    assert!(
        err.to_string()
            .contains("executed action reported changed files")
    );
}
#[test]
fn worktree_state_goals_changed_rejected() {
    use crate::goals::{GoalRecord, GoalStatus};
    use crate::session::{SessionId, SessionPhase};
    use uuid::Uuid;
    let inspection = make_inspection();
    let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
    let mut post = make_inspection();
    post.active_goals = vec![GoalRecord {
        slug: "test".into(),
        title: "Test".into(),
        rationale: "r".into(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "o".into(),
        source_session_id: SessionId::from_uuid(Uuid::nil()),
        updated_in: SessionPhase::Preparation,
    }];
    let mut checks = Vec::new();
    let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
    assert!(err.to_string().contains("active goal set changed"));
}
#[test]
fn worktree_state_meeting_decisions_changed_rejected() {
    let inspection = make_inspection();
    let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
    let mut post = make_inspection();
    post.carried_meeting_decisions = vec!["new decision".into()];
    let mut checks = Vec::new();
    let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
    assert!(
        err.to_string()
            .contains("carried meeting decision memory changed")
    );
}
#[test]
fn worktree_state_git_commit_records_dirty_status() {
    let inspection = make_inspection();
    let action = make_executed(
        "git-commit",
        EngineerActionKind::GitCommit(GitCommitRequest {
            message: "m".into(),
        }),
        0,
        "",
        "",
    );
    let post = make_inspection();
    let mut checks = Vec::new();
    verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c.contains("worktree-dirty-after-commit="))
    );
}
// --- verify_cargo_metadata ---
#[test]
fn cargo_metadata_invalid_json_fails() {
    let mut checks = Vec::new();
    let err =
        verify_cargo_metadata(Path::new("/fake"), "not json at all", &mut checks).unwrap_err();
    assert!(err.to_string().contains("not valid JSON"));
}
#[test]
fn cargo_metadata_missing_workspace_root_fails() {
    let json = r#"{"packages": []}"#;
    let mut checks = Vec::new();
    let err = verify_cargo_metadata(Path::new("/fake"), json, &mut checks).unwrap_err();
    assert!(err.to_string().contains("workspace_root"));
}
#[test]
fn cargo_metadata_missing_packages_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let json = format!(r#"{{"workspace_root": "{}"}}"#, root.display());
    let mut checks = Vec::new();
    let err = verify_cargo_metadata(&root, &json, &mut checks).unwrap_err();
    assert!(err.to_string().contains("packages"));
}
#[test]
fn cargo_metadata_empty_packages_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let json = format!(
        r#"{{"workspace_root": "{}", "packages": []}}"#,
        root.display()
    );
    let mut checks = Vec::new();
    let err = verify_cargo_metadata(&root, &json, &mut checks).unwrap_err();
    assert!(err.to_string().contains("empty package list"));
}
#[test]
fn cargo_metadata_valid_passes() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let json = format!(
        r#"{{"workspace_root": "{}", "packages": [{{"name": "test"}}]}}"#,
        root.display()
    );
    let mut checks = Vec::new();
    verify_cargo_metadata(&root, &json, &mut checks).unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c.contains("metadata-workspace-root="))
    );
    assert!(checks.iter().any(|c| c.contains("metadata-packages=1")));
}
#[test]
fn cargo_metadata_wrong_workspace_root_fails() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    let root1 = std::fs::canonicalize(dir1.path()).unwrap();
    let root2 = std::fs::canonicalize(dir2.path()).unwrap();
    let json = format!(
        r#"{{"workspace_root": "{}", "packages": [{{"name": "x"}}]}}"#,
        root2.display()
    );
    let mut checks = Vec::new();
    let err = verify_cargo_metadata(&root1, &json, &mut checks).unwrap_err();
    assert!(err.to_string().contains("instead of"));
}
