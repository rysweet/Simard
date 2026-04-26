use super::execution::*;
use super::types::{EngineerActionKind, SelectedEngineerAction};
use super::{CARGO_COMMAND_TIMEOUT_SECS, GIT_COMMAND_TIMEOUT_SECS};
use crate::error::SimardError;
use std::time::Duration;

// --- timeout_for_command ---

#[test]
fn timeout_cargo_gets_longer_timeout() {
    let timeout = timeout_for_command(&["cargo", "test"]);
    assert_eq!(timeout, Duration::from_secs(CARGO_COMMAND_TIMEOUT_SECS));
}

#[test]
fn timeout_git_gets_shorter_timeout() {
    let timeout = timeout_for_command(&["git", "status"]);
    assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
}

#[test]
fn timeout_other_command_gets_git_timeout() {
    let timeout = timeout_for_command(&["rustfmt", "--check"]);
    assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
}

#[test]
fn timeout_empty_argv_gets_git_timeout() {
    let timeout = timeout_for_command(&[]);
    assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
}

// --- parse_status_paths ---

#[test]
fn parse_status_paths_standard_git_output() {
    let paths = parse_status_paths(" M src/lib.rs\nA  tests/new.rs\n?? untracked.md\n");
    assert_eq!(paths, vec!["src/lib.rs", "tests/new.rs", "untracked.md"]);
}

#[test]
fn parse_status_paths_empty_input() {
    let paths = parse_status_paths("");
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_only_whitespace_lines_filtered() {
    // trim_end on whitespace lines leaves them non-empty, but the
    // is_empty filter catches fully empty lines.  Trailing-space-only
    // lines survive trim_end; verify behavior:
    let paths = parse_status_paths("   \n   \n");
    // "   " after trim_end is still "   " which passes !is_empty,
    // but len > 3 is false so the line is returned as-is.
    // Actually trim_end("   ") → "" so they're filtered out.
    assert!(paths.is_empty());
}

#[test]
fn parse_status_paths_short_line_kept_as_is() {
    let paths = parse_status_paths("ab\n");
    assert_eq!(paths, vec!["ab"]);
}

// --- trimmed_stdout ---

#[test]
fn trimmed_stdout_nonempty_returns_trimmed() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "  hello world  \n".to_string(),
        stderr: String::new(),
    };
    assert_eq!(trimmed_stdout(&output).unwrap(), "hello world");
}

#[test]
fn trimmed_stdout_empty_returns_error() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "   \n  ".to_string(),
        stderr: String::new(),
    };
    let result = trimmed_stdout(&output);
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("non-empty command result")
    );
}

// --- trimmed_stdout_allow_empty ---

#[test]
fn trimmed_stdout_allow_empty_returns_trimmed() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "  value  ".to_string(),
        stderr: String::new(),
    };
    assert_eq!(trimmed_stdout_allow_empty(&output), "value");
}

#[test]
fn trimmed_stdout_allow_empty_returns_empty_string_for_whitespace() {
    let output = CommandOutput {
        status: std::process::Command::new("true").status().unwrap(),
        stdout: "   \n  ".to_string(),
        stderr: String::new(),
    };
    assert_eq!(trimmed_stdout_allow_empty(&output), "");
}

// --- run_command ---

#[test]
fn run_command_empty_argv_fails() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_command(dir.path(), &[]);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn run_command_newline_in_segment_fails() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_command(dir.path(), &["echo", "line\nbreak"]);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.to_string().contains("single-line"));
}

#[test]
fn run_command_empty_segment_fails() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_command(dir.path(), &["echo", ""]);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.to_string().contains("single-line"));
}

#[test]
fn run_command_carriage_return_in_segment_fails() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_command(dir.path(), &["echo", "cr\rhere"]);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.to_string().contains("single-line"));
}

#[test]
fn run_command_echo_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let output = run_command(dir.path(), &["echo", "hello"]).unwrap();
    assert!(output.stdout.contains("hello"));
}

#[test]
fn run_command_failing_command_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = run_command(dir.path(), &["false"]);
    assert!(result.is_err());
}

#[test]
#[serial_test::serial]
fn run_command_git_rev_parse_non_repo_gives_not_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    // Defensive isolation: prevent a polluted HOME from making git treat
    // the tempdir as part of an outer worktree.
    let result = run_command(dir.path(), &["git", "rev-parse", "--show-toplevel"]);
    assert!(result.is_err());
    match result.err().unwrap() {
        SimardError::NotARepo { .. } => {}
        other => panic!("expected NotARepo, got: {other}"),
    }
}

// --- execute_engineer_action: StructuredTextReplace ---

#[test]
fn execute_structured_edit_replaces_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "hello old world").unwrap();
    let selected = SelectedEngineerAction {
        label: "edit".into(),
        rationale: "test".into(),
        argv: vec!["simard-structured-edit".into()],
        plan_summary: "test".into(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["file.txt".into()],
        kind: EngineerActionKind::StructuredTextReplace(super::types::StructuredEditRequest {
            relative_path: "file.txt".into(),
            search: "old".into(),
            replacement: "new".into(),
            verify_contains: "new".into(),
        }),
    };
    let result = execute_engineer_action(dir.path(), selected).unwrap();
    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(dir.path().join("file.txt")).unwrap();
    assert_eq!(content, "hello new world");
    assert_eq!(result.changed_files, vec!["file.txt"]);
}

#[test]
fn execute_structured_edit_not_found_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "no match here").unwrap();
    let selected = SelectedEngineerAction {
        label: "edit".into(),
        rationale: "test".into(),
        argv: vec!["simard-structured-edit".into()],
        plan_summary: "test".into(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["file.txt".into()],
        kind: EngineerActionKind::StructuredTextReplace(super::types::StructuredEditRequest {
            relative_path: "file.txt".into(),
            search: "nonexistent".into(),
            replacement: "new".into(),
            verify_contains: "new".into(),
        }),
    };
    let err = execute_engineer_action(dir.path(), selected).unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[test]
fn execute_structured_edit_missing_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let selected = SelectedEngineerAction {
        label: "edit".into(),
        rationale: "test".into(),
        argv: vec!["simard-structured-edit".into()],
        plan_summary: "test".into(),
        verification_steps: Vec::new(),
        expected_changed_files: vec!["missing.txt".into()],
        kind: EngineerActionKind::StructuredTextReplace(super::types::StructuredEditRequest {
            relative_path: "missing.txt".into(),
            search: "a".into(),
            replacement: "b".into(),
            verify_contains: "b".into(),
        }),
    };
    let err = execute_engineer_action(dir.path(), selected).unwrap_err();
    assert!(err.to_string().contains("could not read"));
}

// --- sanitize_issue_create_args ---

#[test]
fn sanitize_issue_create_args_happy_path_with_labels() {
    let argv = sanitize_issue_create_args(
        "fix: bug in parser",
        "Body content here.",
        &["bug".to_string(), "p1".to_string()],
        Some("goal-123"),
        Some("engineer"),
    );
    assert_eq!(
        argv,
        vec![
            "gh".to_string(),
            "issue".to_string(),
            "create".to_string(),
            "--title".to_string(),
            "fix: bug in parser".to_string(),
            "--body".to_string(),
            "Body content here.".to_string(),
            "--label".to_string(),
            "bug".to_string(),
            "--label".to_string(),
            "p1".to_string(),
        ]
    );
}

#[test]
fn sanitize_issue_create_args_empty_body_substitutes_placeholder() {
    let argv = sanitize_issue_create_args(
        "Some title",
        "   \n  ",
        &[],
        Some("goal-42"),
        Some("ooda-engineer"),
    );
    // --body must always be present (issue #1011).
    let body_index = argv
        .iter()
        .position(|s| s == "--body")
        .expect("--body flag");
    let body_value = &argv[body_index + 1];
    assert!(
        body_value.contains("goal-42"),
        "body refs goal id: {body_value}"
    );
    assert!(
        body_value.contains("ooda-engineer.log"),
        "body refs agent log path: {body_value}"
    );
    assert!(!argv.iter().any(|s| s == "--label"));
}

#[test]
fn sanitize_issue_create_args_collapses_newlines_and_defaults_title() {
    let argv = sanitize_issue_create_args(
        "  \n\r ",
        "line1\nline2\rline3",
        &["valid-label".to_string(), "  \n  ".to_string()],
        None,
        None,
    );
    let title_index = argv
        .iter()
        .position(|s| s == "--title")
        .expect("--title flag");
    assert_eq!(
        argv[title_index + 1],
        "(untitled issue spawned by OODA daemon)"
    );
    let body_index = argv
        .iter()
        .position(|s| s == "--body")
        .expect("--body flag");
    let body = &argv[body_index + 1];
    assert!(
        !body.contains('\n') && !body.contains('\r'),
        "newlines collapsed: {body:?}"
    );
    assert!(body.contains("line1 line2 line3"), "body collapsed: {body}");
    // Empty label was filtered; valid one kept.
    let label_count = argv.iter().filter(|s| s.as_str() == "--label").count();
    assert_eq!(label_count, 1);
    assert!(argv.iter().any(|s| s == "valid-label"));
}
