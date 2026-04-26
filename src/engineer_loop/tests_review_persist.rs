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
fn optional_review_skips_read_only_scan() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_skips_cargo_test() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::CargoTest);
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_skips_cargo_check() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::CargoCheck);
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_skips_run_shell_command() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::RunShellCommand(
        super::types::ShellCommandRequest {
            argv: vec!["cargo".into(), "fmt".into()],
        },
    ));
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_skips_open_issue() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::OpenIssue(
        super::types::OpenIssueRequest {
            title: "t".into(),
            body: String::new(),
            labels: Vec::new(),
        },
    ));
    run_optional_review(&inspection, &action).unwrap();
}

// --- compute_diff_for_review: argument selection ---

#[test]
fn diff_for_review_git_commit_uses_head_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::GitCommit(super::types::GitCommitRequest {
        message: "test".into(),
    });
    // Won't succeed (not a git repo), but should return empty string gracefully
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty()); // no git repo → empty
}

#[test]
fn diff_for_review_non_commit_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::ReadOnlyScan;
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty()); // no git repo → empty
}

// --- PHILOSOPHY_REVIEW constant ---

#[test]
fn philosophy_review_is_not_empty() {
    assert!(!PHILOSOPHY_REVIEW.is_empty());
    assert!(PHILOSOPHY_REVIEW.contains("simplicity"));
}

// --- run_optional_review: mutating actions (ReviewSession returns None in tests) ---

#[test]
fn optional_review_mutating_structured_text_replace_succeeds_without_session() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::StructuredTextReplace(
        super::types::StructuredEditRequest {
            relative_path: "src/lib.rs".into(),
            search: "old".into(),
            replacement: "new".into(),
            verify_contains: "new".into(),
        },
    ));
    // No LLM session available → review is skipped, returns Ok
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_mutating_create_file_succeeds_without_session() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::CreateFile(
        super::types::CreateFileRequest {
            relative_path: "new.rs".into(),
            content: "fn main() {}".into(),
        },
    ));
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_mutating_append_to_file_succeeds_without_session() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::AppendToFile(
        super::types::AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "entry".into(),
        },
    ));
    run_optional_review(&inspection, &action).unwrap();
}

#[test]
fn optional_review_mutating_git_commit_succeeds_without_session() {
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::GitCommit(
        super::types::GitCommitRequest {
            message: "chore: test".into(),
        },
    ));
    run_optional_review(&inspection, &action).unwrap();
}

// --- compute_diff_for_review: action kind variants ---

#[test]
fn diff_for_review_create_file_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::CreateFile(super::types::CreateFileRequest {
        relative_path: "new.rs".into(),
        content: "content".into(),
    });
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty()); // not a git repo
}

#[test]
fn diff_for_review_append_to_file_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::AppendToFile(super::types::AppendToFileRequest {
        relative_path: "log.txt".into(),
        content: "entry".into(),
    });
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty());
}

#[test]
fn diff_for_review_structured_text_replace_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let kind = EngineerActionKind::StructuredTextReplace(super::types::StructuredEditRequest {
        relative_path: "src/lib.rs".into(),
        search: "old".into(),
        replacement: "new".into(),
        verify_contains: "new".into(),
    });
    let result = compute_diff_for_review(dir.path(), &kind);
    assert!(result.is_empty());
}

#[test]
fn diff_for_review_cargo_test_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let result = compute_diff_for_review(dir.path(), &EngineerActionKind::CargoTest);
    assert!(result.is_empty());
}

#[test]
fn diff_for_review_cargo_check_uses_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    let result = compute_diff_for_review(dir.path(), &EngineerActionKind::CargoCheck);
    assert!(result.is_empty());
}

// --- persist_engineer_loop_artifacts ---

#[test]
fn persist_artifacts_creates_files_in_state_root() {
    let state_dir = tempfile::tempdir().unwrap();
    let inspection = make_inspection();
    let action = make_executed(EngineerActionKind::ReadOnlyScan);
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "all checks ok".to_string(),
        checks: vec!["check1".to_string()],
    };
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "test objective",
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
    // Memory and evidence files should be created
    assert!(state_dir.path().join("memory_records.json").exists());
    assert!(state_dir.path().join("evidence_records.json").exists());
}

#[test]
fn persist_artifacts_with_nonempty_inspection_fields() {
    let state_dir = tempfile::tempdir().unwrap();
    let mut inspection = make_inspection();
    inspection.worktree_dirty = true;
    inspection.changed_files = vec!["src/main.rs".to_string(), "Cargo.toml".to_string()];
    inspection.carried_meeting_decisions = vec!["decision-1".to_string()];
    inspection.architecture_gap_summary = "some gap summary".to_string();
    let mut action = make_executed(EngineerActionKind::ReadOnlyScan);
    action.selected.verification_steps = vec!["step1".to_string(), "step2".to_string()];
    action.changed_files = vec!["src/main.rs".to_string()];
    let verification = super::types::VerificationReport {
        status: "passed".to_string(),
        summary: "verification complete".to_string(),
        checks: vec![],
    };
    let result = persist_engineer_loop_artifacts(
        state_dir.path(),
        RuntimeTopology::SingleProcess,
        "complex objective with details",
        &inspection,
        &action,
        &verification,
        None,
    );
    assert!(result.is_ok());
}

#[test]
fn persist_artifacts_with_different_topologies() {
    for topology in [
        RuntimeTopology::SingleProcess,
        RuntimeTopology::MultiProcess,
        RuntimeTopology::Distributed,
    ] {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::types::VerificationReport {
            status: "ok".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            topology,
            "test",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok(), "failed for topology {:?}", topology);
    }
}

// --- make_inspection / make_executed helper validation ---
