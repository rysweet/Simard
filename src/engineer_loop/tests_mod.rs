use super::execution::execute_engineer_action;
use super::execution::parse_status_paths;
use super::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind,
    SelectedEngineerAction, ShellCommandRequest, analyze_objective, parse_structured_edit_request,
    validate_repo_relative_path,
};

#[test]
fn git_status_paths_strip_status_prefixes() {
    let paths = parse_status_paths(" M src/lib.rs\nA  tests/engineer_loop.rs\n?? docs/index.md\n");
    assert_eq!(
        paths,
        vec![
            "src/lib.rs".to_string(),
            "tests/engineer_loop.rs".to_string(),
            "docs/index.md".to_string()
        ]
    );
}

#[test]
fn structured_edit_request_requires_complete_directives() {
    let error = parse_structured_edit_request("edit-file: docs/demo.txt\nreplace: before\n")
        .expect_err("incomplete structured edit directives should fail");
    assert!(
        error
            .to_string()
            .contains("structured edit objectives must include non-empty"),
        "error should explain the missing directives: {error}"
    );
}

#[test]
fn structured_edit_paths_must_stay_repo_relative() {
    let error = validate_repo_relative_path("../outside.txt")
        .expect_err("parent escapes should be rejected");
    assert!(
        error.to_string().contains("must not escape"),
        "error should explain the rejected path: {error}"
    );
}

// ---- analyze_objective keyword mapping tests ----

#[test]
fn analyze_objective_create_file() {
    assert_eq!(
        analyze_objective("create a new config file"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_new_file() {
    assert_eq!(
        analyze_objective("new file for the project"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_add_file() {
    assert_eq!(
        analyze_objective("add file to the project"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_append() {
    assert_eq!(
        analyze_objective("append log entry"),
        AnalyzedAction::AppendToFile
    );
}

#[test]
fn analyze_objective_add_to() {
    assert_eq!(
        analyze_objective("add to the changelog"),
        AnalyzedAction::AppendToFile
    );
}

#[test]
fn analyze_objective_run_shell_command() {
    assert_eq!(
        analyze_objective("run cargo fmt"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_objective_execute_command() {
    assert_eq!(
        analyze_objective("execute rustfmt on main.rs"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_objective_git_commit() {
    assert_eq!(
        analyze_objective("commit the changes"),
        AnalyzedAction::GitCommit
    );
}

#[test]
fn analyze_objective_save_changes() {
    assert_eq!(
        analyze_objective("save changes to the repo"),
        AnalyzedAction::GitCommit
    );
}

#[test]
fn analyze_objective_open_issue() {
    assert_eq!(
        analyze_objective("open an issue for the bug"),
        AnalyzedAction::OpenIssue
    );
}

#[test]
fn analyze_objective_bug_report() {
    assert_eq!(
        analyze_objective("file a bug report"),
        AnalyzedAction::OpenIssue
    );
}

#[test]
fn analyze_objective_feature_request() {
    assert_eq!(
        analyze_objective("submit a feature request"),
        AnalyzedAction::OpenIssue
    );
}

#[test]
fn analyze_objective_fix_maps_to_structured_edit() {
    assert_eq!(
        analyze_objective("fix the typo in README"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_objective_update_maps_to_structured_edit() {
    assert_eq!(
        analyze_objective("update the version number"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_objective_cargo_test() {
    assert_eq!(
        analyze_objective("test the parser module"),
        AnalyzedAction::CargoTest
    );
}

#[test]
fn analyze_objective_run_tests_maps_to_cargo_test() {
    assert_eq!(
        analyze_objective("run tests for the project"),
        AnalyzedAction::CargoTest
    );
}

#[test]
fn analyze_objective_default_behavior() {
    assert_eq!(
        analyze_objective("unknown gibberish"),
        AnalyzedAction::ReadOnlyScan
    );
}

#[test]
fn analyze_objective_is_case_insensitive() {
    assert_eq!(
        analyze_objective("CREATE a New File"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("RUN cargo fmt"),
        AnalyzedAction::RunShellCommand
    );
}

// ---- CreateFile path validation tests ----

#[test]
fn create_file_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "create-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-create-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "../../../etc/passwd".to_string(),
            content: "malicious".to_string(),
        }),
    };
    let error = execute_engineer_action(dir.path(), selected)
        .expect_err("path traversal should be rejected");
    assert!(
        error.to_string().contains("must not escape"),
        "error should mention traversal: {error}"
    );
}

#[test]
fn create_file_rejects_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("existing.txt"), "content").unwrap();
    let selected = SelectedEngineerAction {
        label: "create-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-create-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "existing.txt".to_string(),
            content: "new".to_string(),
        }),
    };
    let error = execute_engineer_action(dir.path(), selected)
        .expect_err("overwriting existing file should be rejected");
    assert!(
        error.to_string().contains("already exists"),
        "error should explain the rejection: {error}"
    );
}

#[test]
fn create_file_succeeds_with_valid_path() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "create-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-create-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["src/new.rs".to_string()],
        kind: EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "src/new.rs".to_string(),
            content: "fn main() {}".to_string(),
        }),
    };
    let result = execute_engineer_action(dir.path(), selected).unwrap();
    assert_eq!(result.exit_code, 0);
    let written = std::fs::read_to_string(dir.path().join("src/new.rs")).unwrap();
    assert_eq!(written, "fn main() {}");
}

// ---- AppendToFile validation tests ----

#[test]
fn append_to_file_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "append-to-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-append-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path: "../../../etc/shadow".to_string(),
            content: "malicious".to_string(),
        }),
    };
    let error = execute_engineer_action(dir.path(), selected)
        .expect_err("path traversal should be rejected");
    assert!(
        error.to_string().contains("must not escape"),
        "error should mention traversal: {error}"
    );
}

#[test]
fn append_to_file_rejects_nonexistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "append-to-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-append-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path: "missing.txt".to_string(),
            content: "append this".to_string(),
        }),
    };
    let error = execute_engineer_action(dir.path(), selected)
        .expect_err("appending to nonexistent file should fail");
    assert!(
        error.to_string().contains("does not exist"),
        "error should explain: {error}"
    );
}

#[test]
fn append_to_file_succeeds_with_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("log.txt"), "line1\n").unwrap();
    let selected = SelectedEngineerAction {
        label: "append-to-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-append-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["log.txt".to_string()],
        kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path: "log.txt".to_string(),
            content: "line2\n".to_string(),
        }),
    };
    let result = execute_engineer_action(dir.path(), selected).unwrap();
    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(dir.path().join("log.txt")).unwrap();
    assert!(content.contains("line1\n"));
    assert!(content.contains("line2\n"));
}

// ---- RunShellCommand allowlist tests ----

#[test]
fn run_shell_command_rejects_non_allowlisted_command() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "run-shell-command".to_string(),
        rationale: "test".to_string(),
        argv: vec!["rm".to_string(), "-rf".to_string(), "/".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["rm".to_string(), "-rf".to_string(), "/".to_string()],
        }),
    };
    let error = execute_engineer_action(dir.path(), selected)
        .expect_err("non-allowlisted command should be rejected");
    assert!(
        error.to_string().contains("allowlist"),
        "error should mention allowlist: {error}"
    );
}

#[test]
fn run_shell_command_rejects_empty_argv() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "run-shell-command".to_string(),
        rationale: "test".to_string(),
        argv: Vec::new(),
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest { argv: Vec::new() }),
    };
    let error =
        execute_engineer_action(dir.path(), selected).expect_err("empty argv should be rejected");
    assert!(
        error.to_string().contains("empty"),
        "error should explain: {error}"
    );
}

// ---- is_meeting_decision_record tests ----
