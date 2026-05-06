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
fn phase_outcome_variants() {
    let success = PhaseOutcome::Success;
    let failed = PhaseOutcome::Failed("reason".into());
    let skipped = PhaseOutcome::Skipped("why".into());
    assert_eq!(success, PhaseOutcome::Success);
    assert_ne!(success, failed);
    assert_ne!(failed, skipped);
}
