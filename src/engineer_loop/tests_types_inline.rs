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

// ── AnalyzedAction::is_mutating ──────────────────────────────────

#[test]
fn is_mutating_true_for_create_file() {
    assert!(AnalyzedAction::CreateFile.is_mutating());
}

#[test]
fn is_mutating_true_for_append_to_file() {
    assert!(AnalyzedAction::AppendToFile.is_mutating());
}

#[test]
fn is_mutating_true_for_git_commit() {
    assert!(AnalyzedAction::GitCommit.is_mutating());
}

#[test]
fn is_mutating_true_for_open_issue() {
    assert!(AnalyzedAction::OpenIssue.is_mutating());
}

#[test]
fn is_mutating_true_for_structured_text_replace() {
    assert!(AnalyzedAction::StructuredTextReplace.is_mutating());
}

#[test]
fn is_mutating_false_for_read_only_scan() {
    assert!(!AnalyzedAction::ReadOnlyScan.is_mutating());
}

#[test]
fn is_mutating_false_for_cargo_test() {
    assert!(!AnalyzedAction::CargoTest.is_mutating());
}

#[test]
fn is_mutating_false_for_run_shell_command() {
    assert!(!AnalyzedAction::RunShellCommand.is_mutating());
}

// ── SessionErrorReflection ───────────────────────────────────────

#[test]
fn session_error_reflection_serialization_round_trip() {
    let reflection = SessionErrorReflection {
        objective: "fix the bug".to_string(),
        failed_phase: "agent-wait".to_string(),
        error_message: "LLM timeout".to_string(),
        phase_traces: vec![PhaseTrace {
            name: "inspect".to_string(),
            duration: std::time::Duration::from_millis(42),
            outcome: PhaseOutcome::Success,
        }],
    };
    let json = serde_json::to_string(&reflection).expect("serialize");
    let restored: SessionErrorReflection = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(reflection, restored);
}

#[test]
fn session_error_reflection_captures_all_fields() {
    let reflection = SessionErrorReflection {
        objective: "update config".to_string(),
        failed_phase: "review".to_string(),
        error_message: "review blocked".to_string(),
        phase_traces: vec![],
    };
    assert_eq!(reflection.objective, "update config");
    assert_eq!(reflection.failed_phase, "review");
    assert_eq!(reflection.error_message, "review blocked");
    assert!(reflection.phase_traces.is_empty());
}
