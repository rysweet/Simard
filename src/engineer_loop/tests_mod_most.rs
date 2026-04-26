use super::execution::execute_engineer_action;
use super::execution::parse_status_paths;
use super::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind,
    SelectedEngineerAction, ShellCommandRequest, analyze_objective, parse_structured_edit_request,
    validate_repo_relative_path,
};
use crate::PhaseOutcome;

#[test]
fn parse_status_paths_multiple_mixed_statuses() {
    let paths =
        parse_status_paths(" M modified.rs\nA  added.rs\n?? untracked.txt\n D deleted.rs\n");
    assert_eq!(paths.len(), 4);
    assert!(paths.contains(&"modified.rs".to_string()));
    assert!(paths.contains(&"added.rs".to_string()));
    assert!(paths.contains(&"untracked.txt".to_string()));
    assert!(paths.contains(&"deleted.rs".to_string()));
}

// ---- extract_command_from_objective additional tests ----

#[test]
fn extract_command_with_multiple_args() {
    let cmd = super::types::extract_command_from_objective("run cargo test --lib").unwrap();
    assert_eq!(cmd[0], "cargo");
    assert!(cmd.len() >= 2);
}

#[test]
fn extract_command_case_insensitive() {
    let cmd = super::types::extract_command_from_objective("RUN cargo fmt").unwrap();
    assert_eq!(cmd[0], "cargo");
}

// ---- extract_file_path_from_objective additional tests ----

#[test]
fn extract_file_path_nested_directory() {
    let path =
        super::types::extract_file_path_from_objective("create src/engineer_loop/types.rs now")
            .unwrap();
    assert!(path.contains('/'));
}

#[test]
fn extract_file_path_toml_extension() {
    let path = super::types::extract_file_path_from_objective("update Cargo.toml").unwrap();
    assert_eq!(path, "Cargo.toml");
}

// ---- validate_repo_relative_path additional tests ----

#[test]
fn validate_repo_relative_path_nested_dirs() {
    let result = validate_repo_relative_path("src/engineer_loop/mod.rs").unwrap();
    assert_eq!(result, "src/engineer_loop/mod.rs");
}

#[test]
fn validate_repo_relative_path_double_dot_mid_path_rejected() {
    let err = validate_repo_relative_path("src/../../../etc/passwd")
        .expect_err("parent traversal should be rejected");
    assert!(err.to_string().contains("must not escape"));
}

#[test]
fn validate_repo_relative_path_with_dot_prefix() {
    let result = validate_repo_relative_path("./src/main.rs").unwrap();
    assert_eq!(result, "src/main.rs");
}

// ---- RunShellCommand allowlisted commands ----

#[test]
fn run_shell_command_cargo_fmt_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    // Initialize a minimal git repo so git commands work
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output();
    let selected = SelectedEngineerAction {
        label: "run-shell-command".to_string(),
        rationale: "test".to_string(),
        argv: vec!["cargo".to_string(), "version".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["cargo".to_string(), "version".to_string()],
        }),
    };
    // This may succeed or fail depending on whether cargo is available,
    // but it should NOT be rejected by the allowlist.
    let result = execute_engineer_action(dir.path(), selected);
    // Either succeeds or fails for cargo-specific reason, NOT allowlist
    if let Err(e) = &result {
        assert!(
            !e.to_string().contains("allowlist"),
            "cargo should be allowlisted: {e}"
        );
    }
}

#[test]
fn run_shell_command_git_status_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output();
    let selected = SelectedEngineerAction {
        label: "run-shell-command".to_string(),
        rationale: "test".to_string(),
        argv: vec!["git".to_string(), "status".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["git".to_string(), "status".to_string()],
        }),
    };
    let result = execute_engineer_action(dir.path(), selected);
    assert!(
        result.is_ok(),
        "git should be allowlisted: {:?}",
        result.err()
    );
}

// ---- analyze_objective: additional keywords ----

#[test]
fn analyze_objective_edit_falls_through_to_readonly_scan() {
    // "edit" is not a recognized keyword — falls to default ReadOnlyScan
    assert_eq!(
        analyze_objective("edit the config file"),
        AnalyzedAction::ReadOnlyScan
    );
}

#[test]
fn analyze_objective_replace_maps_to_structured_edit() {
    assert_eq!(
        analyze_objective("replace the old text"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_objective_check_maps_to_run_shell_command() {
    // "check" matches the run/execute/check branch → RunShellCommand
    assert_eq!(
        analyze_objective("check the build"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_objective_mixed_case_create() {
    assert_eq!(
        analyze_objective("Create A NEW config.yaml"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_issue_maps_to_open_issue() {
    assert_eq!(
        analyze_objective("file an issue about the crash"),
        AnalyzedAction::OpenIssue
    );
}

// ---- parse_status_paths: renamed files ----

#[test]
fn parse_status_paths_renamed_file() {
    let paths = parse_status_paths("R  old.rs -> new.rs\n");
    // Should produce at least one path
    assert!(!paths.is_empty());
}

// ---- architecture_gap_summary: probe with both keywords ----

#[test]
fn architecture_gap_summary_with_architecture_and_probe_and_contracts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
    std::fs::write(dir.path().join("Specs/ProductArchitecture.md"), "# Arch").unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "terminal-run" }"#,
    )
    .unwrap();
    let docs_dir = dir.path().join("docs/reference");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("terminal-run"));
    assert!(result.contains("runtime contracts docs mention"));
}

// ---- is_meeting_decision_record: near-miss cases ----

#[test]
fn is_meeting_decision_record_missing_decisions() {
    let value = "agenda=a updates=b risks=c next_steps=d open_questions=e goals=f";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_risks() {
    let value = "agenda=a updates=b decisions=c next_steps=d open_questions=e goals=f";
    assert!(!super::is_meeting_decision_record(value));
}

// ---- PhaseTrace/PhaseOutcome coverage ----

#[test]
fn phase_outcome_success_debug() {
    let outcome = PhaseOutcome::Success;
    let debug = format!("{:?}", outcome);
    assert!(debug.contains("Success"));
}

#[test]
fn phase_outcome_failed_debug() {
    let outcome = PhaseOutcome::Failed("test error".to_string());
    let debug = format!("{:?}", outcome);
    assert!(debug.contains("test error"));
}

// ---- CreateFile: nested directory creation ----

#[test]
fn create_file_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "create-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-create-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["deep/nested/dir/file.txt".to_string()],
        kind: EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "deep/nested/dir/file.txt".to_string(),
            content: "deep content".to_string(),
        }),
    };
    let result = execute_engineer_action(dir.path(), selected).unwrap();
    assert_eq!(result.exit_code, 0);
    let written = std::fs::read_to_string(dir.path().join("deep/nested/dir/file.txt")).unwrap();
    assert_eq!(written, "deep content");
}

// ---- AppendToFile: appends correctly ----

#[test]
fn append_to_file_preserves_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("data.txt"), "original\n").unwrap();
    let selected = SelectedEngineerAction {
        label: "append-to-file".to_string(),
        rationale: "test".to_string(),
        argv: vec!["simard-append-file".to_string()],
        plan_summary: "test".to_string(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["data.txt".to_string()],
        kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path: "data.txt".to_string(),
            content: "appended\n".to_string(),
        }),
    };
    let result = execute_engineer_action(dir.path(), selected).unwrap();
    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(dir.path().join("data.txt")).unwrap();
    assert!(content.starts_with("original\n"));
    assert!(content.ends_with("appended\n"));
}

// ---- validate_repo_relative_path: more edge cases ----

#[test]
fn validate_repo_relative_path_with_trailing_slash_accepted() {
    // Trailing slashes are accepted (path normalization strips them)
    let result = validate_repo_relative_path("src/");
    assert!(
        result.is_ok(),
        "trailing slash should be accepted: {result:?}"
    );
}

#[test]
fn validate_repo_relative_path_with_multiple_dots() {
    let err = validate_repo_relative_path("../../outside")
        .expect_err("double parent traversal should be rejected");
    assert!(err.to_string().contains("must not escape"));
}

// ---- constants: additional validation ----

#[test]
fn shell_command_allowlist_does_not_contain_shell() {
    for cmd in &["sh", "bash", "zsh", "python", "python3", "node"] {
        assert!(
            !super::SHELL_COMMAND_ALLOWLIST.contains(cmd),
            "allowlist should not contain interpreter {cmd}"
        );
    }
}

#[test]
fn cleared_git_env_vars_all_start_with_git() {
    for var in super::CLEARED_GIT_ENV_VARS {
        assert!(
            var.starts_with("GIT_"),
            "cleared env var should start with GIT_: {var}"
        );
    }
}

#[test]
fn shell_command_allowlist_contains_cargo() {
    assert!(super::SHELL_COMMAND_ALLOWLIST.contains(&"cargo"));
}

#[test]
fn shell_command_allowlist_contains_git() {
    assert!(super::SHELL_COMMAND_ALLOWLIST.contains(&"git"));
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn max_carried_meeting_decisions_is_positive() {
    const { assert!(super::MAX_CARRIED_MEETING_DECISIONS > 0) };
    const { assert!(super::MAX_CARRIED_MEETING_DECISIONS <= 10) };
}

// ---- is_meeting_decision_record ----

#[test]
fn is_meeting_decision_record_full_match() {
    let value = "agenda=sprint review updates=done decisions=ship risks=none next_steps=deploy open_questions=none goals=release";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_field_v2() {
    let value = "agenda=sprint updates=done decisions=ship risks=none";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_empty_v2() {
    assert!(!super::is_meeting_decision_record(""));
}

#[test]
fn is_meeting_decision_record_partial_fragments() {
    let value = "agenda= updates= decisions=";
    assert!(!super::is_meeting_decision_record(value));
}

// ---- constants: identity and base type ----

#[test]
fn engineer_identity_is_nonempty() {
    assert!(!super::ENGINEER_IDENTITY.is_empty());
}

#[test]
fn engineer_base_type_is_nonempty() {
    assert!(!super::ENGINEER_BASE_TYPE.is_empty());
}

#[test]
fn execution_scope_is_local_only() {
    assert_eq!(super::EXECUTION_SCOPE, "local-only");
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn cargo_timeout_exceeds_git_timeout() {
    const { assert!(super::CARGO_COMMAND_TIMEOUT_SECS >= super::GIT_COMMAND_TIMEOUT_SECS) };
}
