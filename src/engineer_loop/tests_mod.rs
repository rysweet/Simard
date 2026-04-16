use super::execution::execute_engineer_action;
use super::execution::parse_status_paths;
use super::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind,
    SelectedEngineerAction, ShellCommandRequest, analyze_objective, parse_structured_edit_request,
    validate_repo_relative_path,
};
use crate::PhaseOutcome;

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
fn analyze_objective_default_fallback() {
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

#[test]
fn is_meeting_decision_record_positive() {
    let value = "agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_field() {
    // Missing "goals="
    let value =
        "agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_empty_string() {
    assert!(!super::is_meeting_decision_record(""));
}

#[test]
fn is_meeting_decision_record_partial_match() {
    let value = "agenda=stuff decisions=yes";
    assert!(!super::is_meeting_decision_record(value));
}

// ---- Additional types tests ----

#[test]
fn validate_repo_relative_path_absolute_rejected() {
    let err =
        validate_repo_relative_path("/etc/passwd").expect_err("absolute paths should be rejected");
    assert!(err.to_string().contains("must stay relative"));
}

#[test]
fn validate_repo_relative_path_empty_rejected() {
    let err = validate_repo_relative_path("").expect_err("empty paths should be rejected");
    assert!(err.to_string().contains("must identify a file"));
}

#[test]
fn validate_repo_relative_path_dot_only_rejected() {
    let err = validate_repo_relative_path(".").expect_err("dot-only paths should be rejected");
    assert!(err.to_string().contains("must identify a file"));
}

#[test]
fn validate_repo_relative_path_normalizes_dot_segments() {
    let result = validate_repo_relative_path("./src/./lib.rs").unwrap();
    assert_eq!(result, "src/lib.rs");
}

#[test]
fn validate_repo_relative_path_simple_valid() {
    let result = validate_repo_relative_path("src/main.rs").unwrap();
    assert_eq!(result, "src/main.rs");
}

// ---- parse_structured_edit_request tests ----

#[test]
fn structured_edit_complete_request_parses() {
    let obj = "edit-file: src/lib.rs\nreplace: old_text\nwith: new_text\nverify-contains: new_text";
    let result = parse_structured_edit_request(obj).unwrap().unwrap();
    assert_eq!(result.relative_path, "src/lib.rs");
    assert_eq!(result.search, "old_text");
    assert_eq!(result.replacement, "new_text");
    assert_eq!(result.verify_contains, "new_text");
}

#[test]
fn structured_edit_no_directives_returns_none() {
    let obj = "just a regular objective with no edit directives";
    assert!(parse_structured_edit_request(obj).unwrap().is_none());
}

#[test]
fn structured_edit_empty_field_value_rejected() {
    let obj = "edit-file:   \nreplace: old\nwith: new\nverify-contains: new";
    let err = parse_structured_edit_request(obj).unwrap_err();
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn structured_edit_unescape_newlines_and_tabs() {
    let obj = "edit-file: f.rs\nreplace: a\\nb\nwith: c\\td\nverify-contains: c\\td";
    let result = parse_structured_edit_request(obj).unwrap().unwrap();
    assert_eq!(result.search, "a\nb");
    assert_eq!(result.replacement, "c\td");
}

// ---- extract_command_from_objective tests ----

#[test]
fn extract_command_run_keyword() {
    let cmd = super::types::extract_command_from_objective("run cargo fmt").unwrap();
    assert_eq!(cmd, vec!["cargo", "fmt"]);
}

#[test]
fn extract_command_execute_keyword() {
    let cmd = super::types::extract_command_from_objective("execute git status").unwrap();
    assert_eq!(cmd, vec!["git", "status"]);
}

#[test]
fn extract_command_no_keyword_returns_none() {
    assert!(super::types::extract_command_from_objective("please do something").is_none());
}

#[test]
fn extract_command_empty_after_keyword_returns_none() {
    assert!(super::types::extract_command_from_objective("run   ").is_none());
}

// ---- extract_file_path_from_objective tests ----

#[test]
fn extract_file_path_with_slash() {
    let path = super::types::extract_file_path_from_objective("create src/lib.rs now").unwrap();
    assert_eq!(path, "src/lib.rs");
}

#[test]
fn extract_file_path_with_dot_extension() {
    let path = super::types::extract_file_path_from_objective("modify config.toml please").unwrap();
    assert_eq!(path, "config.toml");
}

#[test]
fn extract_file_path_no_path_returns_none() {
    assert!(super::types::extract_file_path_from_objective("do something").is_none());
}

#[test]
fn extract_file_path_short_dot_word_skipped() {
    // Words like "a." are too short (len <= 2) to be considered paths
    assert!(super::types::extract_file_path_from_objective("fix a bug").is_none());
}

// ---- constants tests ----

#[test]
fn engineer_identity_constant() {
    assert_eq!(super::ENGINEER_IDENTITY, "simard-engineer");
}

#[test]
fn engineer_base_type_constant() {
    assert_eq!(super::ENGINEER_BASE_TYPE, "terminal-shell");
}

#[test]
fn execution_scope_constant() {
    assert_eq!(super::EXECUTION_SCOPE, "local-only");
}

#[test]
fn max_carried_meeting_decisions_is_reasonable() {
    let m = super::MAX_CARRIED_MEETING_DECISIONS;
    assert!(m > 0, "must be positive, got {m}");
    assert!(m <= 10, "must be <= 10, got {m}");
}

#[test]
fn shell_command_allowlist_contains_expected_commands() {
    for cmd in &["cargo", "git", "gh", "rustfmt", "clippy"] {
        assert!(
            super::SHELL_COMMAND_ALLOWLIST.contains(cmd),
            "allowlist should contain {cmd}"
        );
    }
}

#[test]
fn shell_command_allowlist_excludes_dangerous_commands() {
    for cmd in &["rm", "sudo", "chmod", "chown", "dd", "mkfs"] {
        assert!(
            !super::SHELL_COMMAND_ALLOWLIST.contains(cmd),
            "allowlist should not contain {cmd}"
        );
    }
}

#[test]
fn cleared_git_env_vars_is_nonempty() {
    assert!(!super::CLEARED_GIT_ENV_VARS.is_empty());
    assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_DIR"));
    assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_WORK_TREE"));
    assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_INDEX_FILE"));
}

#[test]
fn git_command_timeout_is_reasonable() {
    let t = super::GIT_COMMAND_TIMEOUT_SECS;
    assert!(t >= 10, "git timeout too low: {t}");
    assert!(t <= 300, "git timeout too high: {t}");
}

#[test]
fn cargo_command_timeout_is_reasonable() {
    let t = super::CARGO_COMMAND_TIMEOUT_SECS;
    assert!(t >= 30, "cargo timeout too low: {t}");
    assert!(t <= 600, "cargo timeout too high: {t}");
}

// ---- architecture_gap_summary tests ----

#[test]
fn architecture_gap_summary_no_architecture_file() {
    let dir = tempfile::tempdir().unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("missing Specs/ProductArchitecture.md"));
}

#[test]
fn architecture_gap_summary_with_architecture_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
    std::fs::write(
        dir.path().join("Specs/ProductArchitecture.md"),
        "# Architecture",
    )
    .unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("Specs/ProductArchitecture.md"));
    assert!(result.contains("engineer mode"));
}

#[test]
fn architecture_gap_summary_with_probe_engineer_loop_run() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "engineer-loop-run" }"#,
    )
    .unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("engineer-loop-run"));
}

#[test]
fn architecture_gap_summary_with_probe_terminal_run() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "terminal-run" }"#,
    )
    .unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("terminal-run"));
}

#[test]
fn architecture_gap_summary_with_probe_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(bin_dir.join("simard_operator_probe.rs"), "fn main() {}").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("does not yet expose"));
}

#[test]
fn architecture_gap_summary_with_runtime_contracts_docs() {
    let dir = tempfile::tempdir().unwrap();
    let docs_dir = dir.path().join("docs/reference");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("runtime contracts docs mention"));
}

#[test]
fn architecture_gap_summary_without_runtime_contracts_docs() {
    let dir = tempfile::tempdir().unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("runtime contracts docs are absent"));
}

#[test]
fn architecture_gap_summary_all_files_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
    std::fs::write(dir.path().join("Specs/ProductArchitecture.md"), "# Arch").unwrap();
    let bin_dir = dir.path().join("src/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("simard_operator_probe.rs"),
        r#"fn main() { "engineer-loop-run" }"#,
    )
    .unwrap();
    let docs_dir = dir.path().join("docs/reference");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
    let result = super::architecture_gap_summary(dir.path()).unwrap();
    assert!(result.contains("Specs/ProductArchitecture.md"));
    assert!(result.contains("engineer-loop-run"));
    assert!(result.contains("runtime contracts docs mention"));
}

// ---- is_meeting_decision_record additional tests ----

#[test]
fn is_meeting_decision_record_fields_in_different_order() {
    let value = "goals=win open_questions=none next_steps=go risks=low decisions=yes updates=things agenda=stuff";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_with_extra_content() {
    let value = "prefix agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win suffix";
    assert!(super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_missing_agenda() {
    let value =
        "updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win";
    assert!(!super::is_meeting_decision_record(value));
}

#[test]
fn is_meeting_decision_record_only_agenda() {
    assert!(!super::is_meeting_decision_record("agenda=stuff"));
}

// ---- parse_status_paths additional tests ----

#[test]
fn parse_status_paths_empty_input() {
    let paths = parse_status_paths("");
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_whitespace_only() {
    let paths = parse_status_paths("   \n  \n");
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_single_modification() {
    let paths = parse_status_paths(" M src/main.rs\n");
    assert_eq!(paths, vec!["src/main.rs"]);
}

#[test]
fn parse_status_paths_untracked_files() {
    let paths = parse_status_paths("?? new_file.txt\n");
    assert_eq!(paths, vec!["new_file.txt"]);
}

#[test]
fn parse_status_paths_added_file() {
    let paths = parse_status_paths("A  added.rs\n");
    assert_eq!(paths, vec!["added.rs"]);
}

#[test]
fn parse_status_paths_deleted_file() {
    let paths = parse_status_paths(" D removed.rs\n");
    assert_eq!(paths, vec!["removed.rs"]);
}

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
    assert!(super::MAX_CARRIED_MEETING_DECISIONS > 0);
    assert!(super::MAX_CARRIED_MEETING_DECISIONS <= 10);
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
    assert!(super::CARGO_COMMAND_TIMEOUT_SECS >= super::GIT_COMMAND_TIMEOUT_SECS);
}
