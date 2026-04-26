use super::types::*;

#[test]
fn analyze_objective_create_file() {
    assert_eq!(
        analyze_objective("create a new file"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("add file to project"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_append() {
    assert_eq!(
        analyze_objective("append to the log"),
        AnalyzedAction::AppendToFile
    );
}

#[test]
fn analyze_objective_commit() {
    assert_eq!(
        analyze_objective("commit the changes"),
        AnalyzedAction::GitCommit
    );
    assert_eq!(analyze_objective("save changes"), AnalyzedAction::GitCommit);
}

#[test]
fn analyze_objective_issue() {
    assert_eq!(
        analyze_objective("open a new issue"),
        AnalyzedAction::OpenIssue
    );
    assert_eq!(
        analyze_objective("file a bug report"),
        AnalyzedAction::OpenIssue
    );
    assert_eq!(
        analyze_objective("create a feature request"),
        AnalyzedAction::OpenIssue
    );
}

#[test]
fn analyze_objective_cargo_test() {
    assert_eq!(analyze_objective("cargo test"), AnalyzedAction::CargoTest);
    assert_eq!(analyze_objective("run tests"), AnalyzedAction::CargoTest);
    assert_eq!(analyze_objective("test suite"), AnalyzedAction::CargoTest);
}

#[test]
fn analyze_objective_shell() {
    assert_eq!(
        analyze_objective("run ls -la"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("execute the script"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_objective_structured_edit() {
    assert_eq!(
        analyze_objective("fix the typo"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("update the version"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("replace old with new"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_objective_readonly_default() {
    assert_eq!(
        analyze_objective("inspect the workspace layout"),
        AnalyzedAction::ReadOnlyScan
    );
}

#[test]
fn extract_command_from_objective_run() {
    let argv = extract_command_from_objective("run cargo test --all").unwrap();
    assert_eq!(argv, vec!["cargo", "test", "--all"]);
}

#[test]
fn extract_command_from_objective_execute() {
    let argv = extract_command_from_objective("execute git status").unwrap();
    assert_eq!(argv, vec!["git", "status"]);
}

#[test]
fn extract_command_from_objective_no_match() {
    assert!(extract_command_from_objective("just some text").is_none());
}

#[test]
fn extract_command_rejects_prose_with_period() {
    // Issue #912: prose fragments like "git commit -m and open PR against #890."
    // should not be treated as shell commands.
    assert!(
        extract_command_from_objective("run git commit -m and open PR against #890.").is_none()
    );
}

#[test]
fn extract_command_rejects_prose_with_conjunctions() {
    assert!(extract_command_from_objective("run the migration and then deploy").is_none());
}

#[test]
fn extract_command_rejects_prose_with_issue_ref() {
    assert!(extract_command_from_objective("execute the fix for #123 in the planner").is_none());
}

#[test]
fn extract_command_accepts_real_commands() {
    let argv = extract_command_from_objective("run cargo test --all").unwrap();
    assert_eq!(argv, vec!["cargo", "test", "--all"]);
    let argv = extract_command_from_objective("run git status").unwrap();
    assert_eq!(argv, vec!["git", "status"]);
}

#[test]
fn is_prose_fragment_detects_sentence_ending() {
    assert!(is_prose_fragment("commit -m and open PR against #890."));
    assert!(is_prose_fragment("what should we do?"));
    assert!(is_prose_fragment("stop the process!"));
}

#[test]
fn is_prose_fragment_detects_conjunctions() {
    assert!(is_prose_fragment("the migration and then deploy"));
}

#[test]
fn is_prose_fragment_detects_issue_refs() {
    assert!(is_prose_fragment("the fix for #123 in the planner"));
}

#[test]
fn is_prose_fragment_allows_real_commands() {
    assert!(!is_prose_fragment("cargo test --all"));
    assert!(!is_prose_fragment("git status"));
    assert!(!is_prose_fragment("gh issue list"));
}

#[test]
fn is_prose_fragment_empty_is_prose() {
    assert!(is_prose_fragment(""));
    assert!(is_prose_fragment("   "));
}

#[test]
fn extract_file_path_from_objective_finds_path() {
    let path = extract_file_path_from_objective("create src/main.rs with content").unwrap();
    assert_eq!(path, "src/main.rs");
}

#[test]
fn extract_file_path_from_objective_finds_dotfile() {
    let path = extract_file_path_from_objective("update Cargo.toml").unwrap();
    assert_eq!(path, "Cargo.toml");
}

#[test]
fn extract_file_path_from_objective_none_when_no_path() {
    assert!(extract_file_path_from_objective("do something").is_none());
}

#[test]
fn validate_repo_relative_path_valid() {
    assert_eq!(
        validate_repo_relative_path("src/main.rs").unwrap(),
        "src/main.rs"
    );
}

#[test]
fn validate_repo_relative_path_strips_curdir() {
    assert_eq!(
        validate_repo_relative_path("./src/main.rs").unwrap(),
        "src/main.rs"
    );
}

#[test]
fn validate_repo_relative_path_rejects_absolute() {
    assert!(validate_repo_relative_path("/etc/passwd").is_err());
}

#[test]
fn validate_repo_relative_path_rejects_parent() {
    assert!(validate_repo_relative_path("../secret").is_err());
}

#[test]
fn validate_repo_relative_path_rejects_empty() {
    assert!(validate_repo_relative_path("").is_err());
}

#[test]
fn unescape_edit_value_newlines_and_tabs() {
    assert_eq!(unescape_edit_value("line1\\nline2"), "line1\nline2");
    assert_eq!(unescape_edit_value("col1\\tcol2"), "col1\tcol2");
}

#[test]
fn parse_structured_edit_request_complete() {
    let objective = "edit-file: src/lib.rs\nreplace: old_fn\nwith: new_fn\nverify-contains: new_fn";
    let request = parse_structured_edit_request(objective).unwrap().unwrap();
    assert_eq!(request.relative_path, "src/lib.rs");
    assert_eq!(request.search, "old_fn");
    assert_eq!(request.replacement, "new_fn");
    assert_eq!(request.verify_contains, "new_fn");
}

#[test]
fn parse_structured_edit_request_missing_field_errors() {
    let objective = "edit-file: src/lib.rs\nreplace: old_fn";
    let result = parse_structured_edit_request(objective);
    assert!(result.is_err());
}

#[test]
fn parse_structured_edit_request_no_directives_returns_none() {
    let result = parse_structured_edit_request("just regular text").unwrap();
    assert!(result.is_none());
}

#[test]
fn non_empty_objective_value_trims() {
    assert_eq!(
        non_empty_objective_value("field", "  hello  ").unwrap(),
        "hello"
    );
}

#[test]
fn non_empty_objective_value_empty_errors() {
    assert!(non_empty_objective_value("field", "   ").is_err());
}

#[test]
fn phase_outcome_variants() {
    let success = PhaseOutcome::Success;
    let failed = PhaseOutcome::Failed("reason".into());
    let skipped = PhaseOutcome::Skipped("why".into());
    assert_eq!(success, PhaseOutcome::Success);
    assert_ne!(success, failed);
    assert_ne!(failed, skipped);
}
