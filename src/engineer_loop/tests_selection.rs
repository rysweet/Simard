use super::selection::*;
use super::types::{EngineerActionKind, RepoInspection, StructuredEditRequest};
use std::path::PathBuf;

fn make_inspection(dirty: bool, decisions: Vec<String>) -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/fake/workspace"),
        repo_root: PathBuf::from("/fake/repo"),
        branch: "main".to_string(),
        head: "abc123".to_string(),
        worktree_dirty: dirty,
        changed_files: Vec::new(),
        active_goals: Vec::new(),
        carried_meeting_decisions: decisions,
        architecture_gap_summary: String::new(),
    }
}

// --- carry_forward_note ---

#[test]
fn carry_forward_note_empty_decisions() {
    let inspection = make_inspection(false, vec![]);
    assert_eq!(carry_forward_note(&inspection), "");
}

#[test]
fn carry_forward_note_one_decision_singular() {
    let inspection = make_inspection(false, vec!["decision1".to_string()]);
    let note = carry_forward_note(&inspection);
    assert!(note.contains("1 meeting decision record"));
    // Should use singular (no trailing "s")
    assert!(!note.contains("records"));
}

#[test]
fn carry_forward_note_multiple_decisions_plural() {
    let inspection = make_inspection(false, vec!["d1".into(), "d2".into()]);
    let note = carry_forward_note(&inspection);
    assert!(note.contains("2 meeting decision records"));
}

// --- extract_content_body ---

#[test]
fn extract_content_body_with_lines_after_content_directive() {
    let obj = "some header\ncontent:\nline one\nline two";
    assert_eq!(extract_content_body(obj), "line one\nline two");
}

#[test]
fn extract_content_body_no_content_directive() {
    assert_eq!(extract_content_body("no directive here"), "");
}

#[test]
fn extract_content_body_content_at_end_with_nothing_after() {
    assert_eq!(extract_content_body("content:"), "");
}

#[test]
fn extract_content_body_case_insensitive_prefix() {
    let obj = "Content: ignored value\nactual body";
    assert_eq!(extract_content_body(obj), "actual body");
}

// --- select_shell_command ---

#[test]
fn select_shell_command_allowlisted_cargo() {
    let result = select_shell_command("run cargo fmt", "");
    assert!(result.is_some());
    let action = result.unwrap();
    assert_eq!(action.label, "run-shell-command");
    assert_eq!(action.argv[0], "cargo");
}

#[test]
fn select_shell_command_allowlisted_git() {
    let result = select_shell_command("run git status", "");
    assert!(result.is_some());
    let action = result.unwrap();
    assert_eq!(action.argv[0], "git");
}

#[test]
fn select_shell_command_not_allowlisted_returns_none() {
    let result = select_shell_command("run rm -rf /", "");
    assert!(result.is_none());
}

#[test]
fn select_shell_command_no_run_keyword_returns_none() {
    assert!(select_shell_command("please do something", "").is_none());
}

#[test]
fn select_shell_command_preserves_note_in_rationale() {
    let result = select_shell_command("run cargo fmt", " [note]").unwrap();
    assert!(result.rationale.contains("[note]"));
}

#[test]
fn select_shell_command_execute_keyword() {
    let result = select_shell_command("execute git log", "");
    assert!(result.is_some());
    assert_eq!(result.unwrap().argv[0], "git");
}

// --- select_git_commit ---

#[test]
fn select_git_commit_extracts_message_after_commit() {
    let action = select_git_commit("commit fix typo in README", "");
    assert_eq!(action.label, "git-commit");
    match &action.kind {
        EngineerActionKind::GitCommit(req) => {
            assert_eq!(req.message, "fix typo in README");
        }
        _ => panic!("expected GitCommit kind"),
    }
}

#[test]
fn select_git_commit_uses_full_objective_when_no_commit_keyword() {
    let action = select_git_commit("save the work", "");
    match &action.kind {
        EngineerActionKind::GitCommit(req) => {
            assert_eq!(req.message, "save the work");
        }
        _ => panic!("expected GitCommit kind"),
    }
}

#[test]
fn select_git_commit_argv_contains_git_commit_m() {
    let action = select_git_commit("commit hello world", "");
    assert_eq!(action.argv[0], "git");
    assert_eq!(action.argv[1], "commit");
    assert_eq!(action.argv[2], "-m");
    assert_eq!(action.argv[3], "hello world");
}

// --- select_open_issue ---

#[test]
fn select_open_issue_builds_action_with_title() {
    let action = select_open_issue("fix the login page", "").unwrap();
    assert_eq!(action.label, "open-issue");
    assert!(action.argv.contains(&"gh".to_string()));
    match &action.kind {
        EngineerActionKind::OpenIssue(req) => {
            assert_eq!(req.title, "fix the login page");
            assert!(req.body.is_empty());
            assert!(req.labels.is_empty());
        }
        _ => panic!("expected OpenIssue kind"),
    }
}

#[test]
fn select_open_issue_note_appears_in_rationale() {
    let action = select_open_issue("test", " [extra]").unwrap();
    assert!(action.rationale.contains("[extra]"));
}

// --- select_cargo_action ---

#[test]
fn select_cargo_action_cargo_test_keyword() {
    let action = select_cargo_action("cargo test the project", "");
    assert_eq!(action.label, "cargo-test");
    assert!(matches!(action.kind, EngineerActionKind::CargoTest));
}

#[test]
fn select_cargo_action_run_tests() {
    assert_eq!(
        select_cargo_action("run tests please", "").label,
        "cargo-test"
    );
}

#[test]
fn select_cargo_action_test_suite() {
    assert_eq!(
        select_cargo_action("test suite verification", "").label,
        "cargo-test"
    );
}

#[test]
fn select_cargo_action_run_the_tests() {
    assert_eq!(
        select_cargo_action("run the tests now", "").label,
        "cargo-test"
    );
}

#[test]
fn select_cargo_action_cargo_check() {
    let action = select_cargo_action("cargo check the build", "");
    assert_eq!(action.label, "cargo-check");
    assert!(matches!(action.kind, EngineerActionKind::CargoCheck));
}

#[test]
fn select_cargo_action_cargo_build() {
    assert_eq!(
        select_cargo_action("cargo build the project", "").label,
        "cargo-check"
    );
}

#[test]
fn select_cargo_action_compilation_check() {
    assert_eq!(
        select_cargo_action("compilation check", "").label,
        "cargo-check"
    );
}

#[test]
fn select_cargo_action_check_compilation() {
    assert_eq!(
        select_cargo_action("check compilation errors", "").label,
        "cargo-check"
    );
}

#[test]
fn select_cargo_action_default_falls_to_metadata_scan() {
    let action = select_cargo_action("inspect the workspace", "");
    assert_eq!(action.label, "cargo-metadata-scan");
    assert!(matches!(action.kind, EngineerActionKind::ReadOnlyScan));
}

#[test]
fn select_cargo_action_metadata_has_no_deps_flag() {
    let action = select_cargo_action("what packages", "");
    assert!(action.argv.contains(&"--no-deps".to_string()));
}

// --- select_create_file ---

#[test]
fn select_create_file_with_valid_path_and_content() {
    let result = select_create_file("create src/lib.rs\ncontent:\nfn main() {}", "");
    assert!(result.is_some());
    let action = result.unwrap().unwrap();
    assert_eq!(action.label, "create-file");
    match &action.kind {
        EngineerActionKind::CreateFile(req) => {
            assert_eq!(req.relative_path, "src/lib.rs");
            assert_eq!(req.content, "fn main() {}");
        }
        _ => panic!("expected CreateFile kind"),
    }
}

#[test]
fn select_create_file_no_path_returns_none() {
    assert!(select_create_file("create something", "").is_none());
}

#[test]
fn select_create_file_absolute_path_returns_error() {
    let result = select_create_file("create /etc/passwd\ncontent:\nbad", "");
    assert!(result.is_some());
    let err = result.unwrap().unwrap_err();
    assert!(err.to_string().contains("must stay relative"));
}

// --- select_append_to_file ---

#[test]
fn select_append_to_file_with_valid_path() {
    let result = select_append_to_file("append to src/lib.rs\ncontent:\nnew line", "");
    assert!(result.is_some());
    let action = result.unwrap().unwrap();
    assert_eq!(action.label, "append-to-file");
    match &action.kind {
        EngineerActionKind::AppendToFile(req) => {
            assert_eq!(req.relative_path, "src/lib.rs");
        }
        _ => panic!("expected AppendToFile kind"),
    }
}

#[test]
fn select_append_to_file_no_path_returns_none() {
    assert!(select_append_to_file("append something", "").is_none());
}

// --- select_structured_edit ---

#[test]
fn select_structured_edit_dirty_worktree_rejected() {
    let inspection = make_inspection(true, vec![]);
    let edit = StructuredEditRequest {
        relative_path: "src/lib.rs".to_string(),
        search: "old".to_string(),
        replacement: "new".to_string(),
        verify_contains: "new".to_string(),
    };
    let err = select_structured_edit(&inspection, edit, "").unwrap_err();
    assert!(err.to_string().contains("clean git worktree"));
}

#[test]
fn select_structured_edit_clean_worktree_succeeds() {
    let inspection = make_inspection(false, vec![]);
    let edit = StructuredEditRequest {
        relative_path: "src/lib.rs".to_string(),
        search: "old".to_string(),
        replacement: "new".to_string(),
        verify_contains: "new".to_string(),
    };
    let action = select_structured_edit(&inspection, edit, "").unwrap();
    assert_eq!(action.label, "structured-text-replace");
    assert_eq!(action.expected_changed_files, vec!["src/lib.rs"]);
}

#[test]
fn select_structured_edit_absolute_path_rejected() {
    let inspection = make_inspection(false, vec![]);
    let edit = StructuredEditRequest {
        relative_path: "/etc/passwd".to_string(),
        search: "old".to_string(),
        replacement: "new".to_string(),
        verify_contains: "new".to_string(),
    };
    let err = select_structured_edit(&inspection, edit, "").unwrap_err();
    assert!(err.to_string().contains("must stay relative"));
}

#[test]
fn select_structured_edit_parent_traversal_rejected() {
    let inspection = make_inspection(false, vec![]);
    let edit = StructuredEditRequest {
        relative_path: "../escape.txt".to_string(),
        search: "old".to_string(),
        replacement: "new".to_string(),
        verify_contains: "new".to_string(),
    };
    let err = select_structured_edit(&inspection, edit, "").unwrap_err();
    assert!(err.to_string().contains("must not escape"));
}

#[test]
fn select_structured_edit_verification_steps_mention_file_and_verify() {
    let inspection = make_inspection(false, vec![]);
    let edit = StructuredEditRequest {
        relative_path: "readme.md".to_string(),
        search: "old".to_string(),
        replacement: "new".to_string(),
        verify_contains: "new text".to_string(),
    };
    let action = select_structured_edit(&inspection, edit, "").unwrap();
    assert!(
        action
            .verification_steps
            .iter()
            .any(|s| s.contains("readme.md"))
    );
    assert!(
        action
            .verification_steps
            .iter()
            .any(|s| s.contains("new text"))
    );
}
