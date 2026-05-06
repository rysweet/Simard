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
