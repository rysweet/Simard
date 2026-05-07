use super::types::*;

// ---------------------------------------------------------------------------
// analyze_objective
// ---------------------------------------------------------------------------

#[test]
fn analyze_create_file() {
    assert_eq!(
        analyze_objective("create a new file"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("add file src/main.rs"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("New file for logging"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_append_to_file() {
    assert_eq!(
        analyze_objective("append to config"),
        AnalyzedAction::AppendToFile
    );
    assert_eq!(
        analyze_objective("add to the file"),
        AnalyzedAction::AppendToFile
    );
}

#[test]
fn analyze_git_commit() {
    assert_eq!(
        analyze_objective("commit these changes"),
        AnalyzedAction::GitCommit
    );
    assert_eq!(
        analyze_objective("save changes to repo"),
        AnalyzedAction::GitCommit
    );
}

#[test]
fn analyze_open_issue() {
    assert_eq!(
        analyze_objective("open an issue for this bug"),
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
fn analyze_cargo_test() {
    assert_eq!(
        analyze_objective("cargo test the module"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("run tests for this crate"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("run the tests"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("execute test suite"),
        AnalyzedAction::CargoTest
    );
}

#[test]
fn analyze_run_shell_command() {
    assert_eq!(
        analyze_objective("run cargo clippy"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("execute the linter"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("check the build"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_structured_text_replace() {
    assert_eq!(
        analyze_objective("fix the import"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("change the timeout value"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("update the config"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("replace old API call"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_read_only_scan_default() {
    assert_eq!(
        analyze_objective("analyze the architecture"),
        AnalyzedAction::ReadOnlyScan
    );
    assert_eq!(
        analyze_objective("review the design"),
        AnalyzedAction::ReadOnlyScan
    );
}

#[test]
fn analyze_case_insensitive() {
    assert_eq!(
        analyze_objective("CREATE a new file"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("COMMIT changes"),
        AnalyzedAction::GitCommit
    );
    assert_eq!(analyze_objective("Run Tests"), AnalyzedAction::CargoTest);
}

// ---------------------------------------------------------------------------
// extract_command_from_objective
// ---------------------------------------------------------------------------

#[test]
fn extract_command_run() {
    let cmd = extract_command_from_objective("run cargo clippy --all-features").unwrap();
    assert_eq!(cmd, vec!["cargo", "clippy", "--all-features"]);
}

#[test]
fn extract_command_execute() {
    let cmd = extract_command_from_objective("execute ls -la").unwrap();
    assert_eq!(cmd, vec!["ls", "-la"]);
}

#[test]
fn extract_command_none_when_no_keyword() {
    assert!(extract_command_from_objective("fix the bug in main.rs").is_none());
}

#[test]
fn extract_command_none_when_empty_after_keyword() {
    assert!(extract_command_from_objective("run ").is_none());
}

// ---------------------------------------------------------------------------
// extract_file_path_from_objective
// ---------------------------------------------------------------------------

#[test]
fn extract_file_path_with_slash() {
    let path = extract_file_path_from_objective("update src/main.rs with fix").unwrap();
    assert_eq!(path, "src/main.rs");
}

#[test]
fn extract_file_path_with_extension() {
    let path = extract_file_path_from_objective("fix config.toml values").unwrap();
    assert_eq!(path, "config.toml");
}

#[test]
fn extract_file_path_none_when_no_path() {
    assert!(extract_file_path_from_objective("fix the bug").is_none());
}

// ---------------------------------------------------------------------------
// parse_structured_edit_request
// ---------------------------------------------------------------------------

#[test]
fn parse_structured_edit_valid() {
    let obj = "\
edit-file: src/lib.rs
replace: old_fn()
with: new_fn()
verify-contains: new_fn()";
    let req = parse_structured_edit_request(obj).unwrap().unwrap();
    assert_eq!(req.relative_path, "src/lib.rs");
    assert_eq!(req.search, "old_fn()");
    assert_eq!(req.replacement, "new_fn()");
    assert_eq!(req.verify_contains, "new_fn()");
}

#[test]
fn parse_structured_edit_unescape() {
    let obj = "\
edit-file: src/lib.rs
replace: line1\\nline2
with: line1\\nline2\\tnew
verify-contains: line2\\tnew";
    let req = parse_structured_edit_request(obj).unwrap().unwrap();
    assert_eq!(req.search, "line1\nline2");
    assert_eq!(req.replacement, "line1\nline2\tnew");
    assert_eq!(req.verify_contains, "line2\tnew");
}

#[test]
fn parse_structured_edit_returns_none_without_directives() {
    let obj = "just a plain objective with no edit directives";
    assert!(parse_structured_edit_request(obj).unwrap().is_none());
}

#[test]
fn parse_structured_edit_fails_on_partial_directives() {
    let obj = "\
edit-file: src/lib.rs
replace: old_fn()";
    let result = parse_structured_edit_request(obj);
    assert!(result.is_err());
}

#[test]
fn parse_structured_edit_fails_on_empty_field() {
    let obj = "\
edit-file:
replace: old
with: new
verify-contains: new";
    assert!(parse_structured_edit_request(obj).is_err());
}

// ---------------------------------------------------------------------------
// validate_repo_relative_path
// ---------------------------------------------------------------------------

#[test]
fn validate_relative_path_normal() {
    let result = validate_repo_relative_path("src/main.rs").unwrap();
    assert_eq!(result, "src/main.rs");
}

#[test]
fn validate_relative_path_strips_curdir() {
    let result = validate_repo_relative_path("./src/main.rs").unwrap();
    assert_eq!(result, "src/main.rs");
}

#[test]
fn validate_relative_path_rejects_absolute() {
    assert!(validate_repo_relative_path("/etc/passwd").is_err());
}

#[test]
fn validate_relative_path_rejects_parent_traversal() {
    assert!(validate_repo_relative_path("../secret.txt").is_err());
}

#[test]
fn validate_relative_path_rejects_empty() {
    assert!(validate_repo_relative_path("").is_err());
}

#[test]
fn validate_relative_path_rejects_dot_only() {
    assert!(validate_repo_relative_path(".").is_err());
}

// ---------------------------------------------------------------------------
// unescape_edit_value
// ---------------------------------------------------------------------------

#[test]
fn unescape_newlines_and_tabs() {
    assert_eq!(unescape_edit_value("a\\nb"), "a\nb");
    assert_eq!(unescape_edit_value("a\\tb"), "a\tb");
    assert_eq!(unescape_edit_value("no escapes"), "no escapes");
}

// ---------------------------------------------------------------------------
// non_empty_objective_value
// ---------------------------------------------------------------------------

#[test]
fn non_empty_ok() {
    assert_eq!(
        non_empty_objective_value("test", " hello ").unwrap(),
        "hello"
    );
}

#[test]
fn non_empty_rejects_whitespace() {
    assert!(non_empty_objective_value("test", "   ").is_err());
}
