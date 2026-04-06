use super::types::*;

// ── analyze_objective ────────────────────────────────────────────

#[test]
fn analyze_objective_create_file() {
    assert_eq!(
        analyze_objective("create a new file"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("add file foo.rs"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_append() {
    assert_eq!(
        analyze_objective("append to README"),
        AnalyzedAction::AppendToFile
    );
    assert_eq!(
        analyze_objective("add to the config"),
        AnalyzedAction::AppendToFile
    );
}

#[test]
fn analyze_objective_git_commit() {
    assert_eq!(
        analyze_objective("commit the changes"),
        AnalyzedAction::GitCommit
    );
    assert_eq!(
        analyze_objective("save changes now"),
        AnalyzedAction::GitCommit
    );
}

#[test]
fn analyze_objective_open_issue() {
    assert_eq!(
        analyze_objective("open an issue for tracking"),
        AnalyzedAction::OpenIssue
    );
    assert_eq!(
        analyze_objective("file a bug report"),
        AnalyzedAction::OpenIssue
    );
    assert_eq!(
        analyze_objective("submit feature request"),
        AnalyzedAction::OpenIssue
    );
}

#[test]
fn analyze_objective_cargo_test() {
    assert_eq!(
        analyze_objective("cargo test --lib"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(analyze_objective("run tests"), AnalyzedAction::CargoTest);
    assert_eq!(
        analyze_objective("run the tests now"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("test suite validation"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("test the module"),
        AnalyzedAction::CargoTest
    );
}

#[test]
fn analyze_objective_shell_command() {
    assert_eq!(
        analyze_objective("run cargo clippy"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("execute the script"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("check the output"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_objective_structured_replace() {
    assert_eq!(
        analyze_objective("fix the bug"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("change the config"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("update the version"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("replace the string"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_objective_read_only_fallback() {
    assert_eq!(
        analyze_objective("inspect the codebase"),
        AnalyzedAction::ReadOnlyScan
    );
    assert_eq!(
        analyze_objective("look at the structure"),
        AnalyzedAction::ReadOnlyScan
    );
}

// ── extract_command_from_objective ────────────────────────────────

#[test]
fn extract_command_present() {
    let argv = extract_command_from_objective("run cargo clippy --all-targets").unwrap();
    assert_eq!(argv, vec!["cargo", "clippy", "--all-targets"]);
}

#[test]
fn extract_command_execute() {
    let argv = extract_command_from_objective("please execute npm test").unwrap();
    assert_eq!(argv, vec!["npm", "test"]);
}

#[test]
fn extract_command_none() {
    assert!(extract_command_from_objective("inspect the files").is_none());
}

// ── extract_file_path_from_objective ─────────────────────────────

#[test]
fn extract_file_path_slash() {
    let path = extract_file_path_from_objective("fix src/main.rs").unwrap();
    assert_eq!(path, "src/main.rs");
}

#[test]
fn extract_file_path_dotted() {
    let path = extract_file_path_from_objective("edit Cargo.toml").unwrap();
    assert_eq!(path, "Cargo.toml");
}

#[test]
fn extract_file_path_none() {
    assert!(extract_file_path_from_objective("just check things").is_none());
}

// ── validate_repo_relative_path ──────────────────────────────────

#[test]
fn validate_relative_path_ok() {
    assert_eq!(
        validate_repo_relative_path("src/lib.rs").unwrap(),
        "src/lib.rs"
    );
}

#[test]
fn validate_relative_path_normalizes_dot() {
    assert_eq!(
        validate_repo_relative_path("./src/lib.rs").unwrap(),
        "src/lib.rs"
    );
}

#[test]
fn validate_relative_path_rejects_absolute() {
    assert!(validate_repo_relative_path("/etc/passwd").is_err());
}

#[test]
fn validate_relative_path_rejects_parent() {
    assert!(validate_repo_relative_path("../escape").is_err());
}

#[test]
fn validate_relative_path_rejects_empty() {
    assert!(validate_repo_relative_path("").is_err());
}

// ── parse_structured_edit_request ─────────────────────────────────

#[test]
fn parse_edit_request_complete() {
    let obj = "edit-file: src/lib.rs\nreplace: old\nwith: new\nverify-contains: new";
    let req = parse_structured_edit_request(obj).unwrap().unwrap();
    assert_eq!(req.relative_path, "src/lib.rs");
    assert_eq!(req.search, "old");
    assert_eq!(req.replacement, "new");
}

#[test]
fn parse_edit_request_no_directives() {
    assert!(
        parse_structured_edit_request("just a plain objective")
            .unwrap()
            .is_none()
    );
}

#[test]
fn parse_edit_request_missing_field() {
    let obj = "edit-file: src/lib.rs\nreplace: old";
    assert!(parse_structured_edit_request(obj).is_err());
}

// ── unescape_edit_value ──────────────────────────────────────────

#[test]
fn unescape_newlines_and_tabs() {
    assert_eq!(unescape_edit_value("a\\nb\\tc"), "a\nb\tc");
}
