use super::execution::parse_status_paths;
use super::types::{AnalyzedAction, analyze_objective};

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

// ---- is_meeting_decision_record tests ----
