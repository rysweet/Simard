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
fn analyze_objective_read_only_default() {
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
fn extract_command_none() {
    // "inspect the files" has no run/execute keyword
    assert!(
        analyze_objective("inspect the files") == AnalyzedAction::ReadOnlyScan,
        "inspect should default to ReadOnlyScan"
    );
}
