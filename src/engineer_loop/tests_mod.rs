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

// ---- extract_referenced_numbers tests (issue #1670) ----

#[test]
fn extract_referenced_numbers_finds_issue_numbers() {
    let nums = super::extract_referenced_numbers("Fix issue #1670 and close #42");
    assert_eq!(nums, vec![1670, 42]);
}

#[test]
fn extract_referenced_numbers_deduplicates() {
    let nums = super::extract_referenced_numbers("#5 is related to #5 and #5");
    assert_eq!(nums, vec![5]);
}

#[test]
fn extract_referenced_numbers_empty_when_none() {
    let nums = super::extract_referenced_numbers("no references here");
    assert!(nums.is_empty());
}

#[test]
fn extract_referenced_numbers_ignores_bare_hash() {
    let nums = super::extract_referenced_numbers("# heading with no number");
    assert!(nums.is_empty());
}

// ---- count_new_status_paths tests (issue #1670) ----

#[test]
fn count_new_status_paths_finds_new_files() {
    let before = vec!["a.rs".to_string()];
    let after = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
    let new = super::count_new_status_paths(&before, &after);
    assert_eq!(new, vec!["b.rs".to_string(), "c.rs".to_string()]);
}

#[test]
fn count_new_status_paths_empty_when_no_change() {
    let before = vec!["a.rs".to_string()];
    let after = vec!["a.rs".to_string()];
    let new = super::count_new_status_paths(&before, &after);
    assert!(new.is_empty());
}

#[test]
fn count_new_status_paths_empty_inputs() {
    let new = super::count_new_status_paths(&[], &[]);
    assert!(new.is_empty());
}

// ---- verify_agent_spawn_artifacts contract tests (issue #1670) ----

#[test]
fn verification_report_status_is_verified_or_unverified_never_agent_completed() {
    // Construct a verification report via the function under test, pointed at
    // the real repo root. Since we haven't made commits, this will be
    // "unverified" (or "verified" if the worktree is dirty). Either way,
    // "agent-completed" must never appear.
    let inspection = super::types::RepoInspection {
        workspace_root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        repo_root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        branch: "test".to_string(),
        head: "HEAD".to_string(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    };
    let report = super::verify_agent_spawn_artifacts(&inspection, "test objective");
    assert!(
        report.status == "verified" || report.status == "unverified",
        "status must be verified or unverified, got: {}",
        report.status
    );
    assert_ne!(report.status, "agent-completed");
    // When status is "verified", checks must be non-empty.
    if report.status == "verified" {
        assert!(
            !report.checks.is_empty(),
            "verified status requires at least one check"
        );
    }
}
