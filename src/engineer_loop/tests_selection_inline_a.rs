use super::selection::*;
use super::*;
use std::path::PathBuf;

#[test]
fn carry_forward_note_empty_when_no_decisions() {
    let inspection = RepoInspection {
        workspace_root: PathBuf::from("."),
        repo_root: PathBuf::from("."),
        branch: "main".into(),
        head: "abc123".into(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    };
    assert!(carry_forward_note(&inspection).is_empty());
}

#[test]
fn carry_forward_note_singular_decision() {
    let inspection = RepoInspection {
        workspace_root: PathBuf::from("."),
        repo_root: PathBuf::from("."),
        branch: "main".into(),
        head: "abc123".into(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec!["decision-1".into()],
        architecture_gap_summary: String::new(),
    };
    let note = carry_forward_note(&inspection);
    assert!(note.contains("1 meeting decision record"));
    assert!(!note.contains("records"));
}

#[test]
fn carry_forward_note_plural_decisions() {
    let inspection = RepoInspection {
        workspace_root: PathBuf::from("."),
        repo_root: PathBuf::from("."),
        branch: "main".into(),
        head: "abc123".into(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec!["d1".into(), "d2".into()],
        architecture_gap_summary: String::new(),
    };
    let note = carry_forward_note(&inspection);
    assert!(note.contains("2 meeting decision records"));
}

#[test]
fn extract_content_body_extracts_after_content_line() {
    let objective = "Some preamble\ncontent: start\nline1\nline2";
    let body = extract_content_body(objective);
    assert_eq!(body, "line1\nline2");
}

#[test]
fn extract_content_body_empty_when_no_content_directive() {
    let body = extract_content_body("just some text");
    assert!(body.is_empty());
}

#[test]
fn select_git_commit_extracts_message_after_commit() {
    let action = select_git_commit("commit Fix the bug", "");
    assert_eq!(action.label, "git-commit");
    assert_eq!(action.argv[3], "Fix the bug");
}

#[test]
fn select_git_commit_uses_full_objective_when_no_commit_keyword() {
    let action = select_git_commit("Save my changes now", "");
    assert_eq!(action.label, "git-commit");
    assert_eq!(action.argv[3], "Save my changes now");
}

#[test]
fn select_open_issue_creates_action() {
    let action = select_open_issue("Report a bug", "").unwrap();
    assert_eq!(action.label, "open-issue");
    assert!(action.argv.contains(&"--title".to_string()));
}

#[test]
fn select_open_issue_truncates_long_title() {
    let long_title = "x ".repeat(200);
    let action = select_open_issue(&long_title, "").unwrap();
    let title_arg_idx = action.argv.iter().position(|a| a == "--title").unwrap() + 1;
    // Title should be truncated near MAX_ISSUE_TITLE_LEN (plus '…' suffix)
    assert!(action.argv[title_arg_idx].len() <= MAX_ISSUE_TITLE_LEN + "…".len());
}

#[test]
fn select_open_issue_rejects_empty_title() {
    assert!(select_open_issue("", "").is_err());
}

#[test]
fn sanitize_issue_title_collapses_whitespace_and_newlines() {
    let raw = "line one\nline two\r\nline three";
    let title = sanitize_issue_title(raw);
    assert_eq!(title, "line one line two line three");
    assert!(!title.contains('\n'));
    assert!(!title.contains('\r'));
}

#[test]
fn sanitize_issue_title_strips_shell_unsafe_chars() {
    let raw = "Create `feature` with $(whoami) and \"quotes\"";
    let title = sanitize_issue_title(raw);
    assert!(!title.contains('`'));
    assert!(!title.contains('$'));
    assert!(!title.contains('"'));
    assert!(!title.contains('('));
    assert!(!title.contains(')'));
    assert_eq!(title, "Create feature with whoami and quotes");
}

#[test]
fn sanitize_issue_title_handles_non_json_llm_output() {
    // Simulates raw non-JSON LLM output with markdown and code blocks
    let raw = "```json\n{\"error\": \"parse failed\"}\n```\nSome `code` here; rm -rf /";
    let title = sanitize_issue_title(raw);
    assert!(!title.contains('\n'));
    assert!(!title.contains('`'));
    assert!(!title.contains(';'));
    assert!(!title.contains('"'));
}

#[test]
fn sanitize_issue_title_handles_pipe_and_redirect() {
    let raw = "Fix bug | echo pwned > /etc/passwd";
    let title = sanitize_issue_title(raw);
    assert!(!title.contains('|'));
    assert!(!title.contains('>'));
    assert_eq!(title, "Fix bug echo pwned /etc/passwd");
}

#[test]
fn sanitize_commit_message_strips_newlines_and_shell_chars() {
    let raw = "fix: update `config`\nwith $(cmd) injection";
    let msg = sanitize_commit_message(raw);
    assert!(!msg.contains('\n'));
    assert!(!msg.contains('`'));
    assert!(!msg.contains('$'));
    assert!(!msg.contains('('));
    assert_eq!(msg, "fix: update config with cmd injection");
}

#[test]
fn sanitize_commit_message_truncates_long_input() {
    let long = "word ".repeat(200);
    let msg = sanitize_commit_message(&long);
    assert!(msg.len() <= MAX_COMMIT_MESSAGE_LEN + "…".len());
}

#[test]
fn select_git_commit_sanitizes_message() {
    let action = select_git_commit("commit Fix `bug`\nwith $(exploit)", "");
    let msg = &action.argv[3];
    assert!(!msg.contains('\n'));
    assert!(!msg.contains('`'));
    assert!(!msg.contains('$'));
}

#[test]
fn select_open_issue_sanitizes_shell_chars_in_title() {
    let action = select_open_issue("Report a `bug` with $(cmd)", "").unwrap();
    let title_idx = action.argv.iter().position(|a| a == "--title").unwrap() + 1;
    let title = &action.argv[title_idx];
    assert!(!title.contains('`'));
    assert!(!title.contains('$'));
    assert!(!title.contains('('));
}

#[test]
fn select_open_issue_multiline_task_yields_argv_with_no_newlines_or_empties() {
    // Regression for issue #943: keyword-fallback planner used to emit
    // `gh issue create --title <multi-line task> --body ''` which the
    // argv-only validator in execution::run_command rejects with
    // "argv-only command segments must be non-empty single-line values".
    let multi_line = "Investigate planner crash\nand fix it\r\nplease";
    let action = select_open_issue(multi_line, "").unwrap();
    for segment in &action.argv {
        assert!(
            !segment.is_empty(),
            "argv segment must not be empty: {:?}",
            action.argv
        );
        assert!(
            !segment.contains('\n'),
            "argv segment must not contain '\\n': {segment:?}"
        );
        assert!(
            !segment.contains('\r'),
            "argv segment must not contain '\\r': {segment:?}"
        );
    }
    // The empty-body pair must not appear in the planner's argv.
    assert!(
        !action.argv.iter().any(|s| s == "--body"),
        "planner argv must not include --body for an empty body: {:?}",
        action.argv
    );
    // Sanity: title is single-line and non-empty.
    let title_idx = action.argv.iter().position(|a| a == "--title").unwrap() + 1;
    let title = &action.argv[title_idx];
    assert!(!title.is_empty());
    assert!(!title.contains('\n'));
    assert!(!title.contains('\r'));
}

#[test]
fn select_cargo_action_detects_test() {
    let action = select_cargo_action("run tests", "");
    assert_eq!(action.label, "cargo-test");
}

#[test]
fn select_cargo_action_detects_check() {
    let action = select_cargo_action("cargo check", "");
    assert_eq!(action.label, "cargo-check");
}

#[test]
fn select_cargo_action_falls_back_to_metadata() {
    let action = select_cargo_action("inspect the workspace", "");
    assert_eq!(action.label, "cargo-metadata-scan");
}

#[test]
fn select_shell_command_respects_allowlist() {
    let action = select_shell_command("run cargo test --all", "");
    assert!(action.is_some());

    let action = select_shell_command("run python script.py", "");
    assert!(action.is_none());
}

// ------------------------------------------------------------------
// WS-B: extract_existing_issue_number + verify-path tests (TDD)
// ------------------------------------------------------------------

#[test]
fn extract_existing_issue_number_matches_hash_form() {
    assert_eq!(extract_existing_issue_number("fix #915 now"), Some(915));
    assert_eq!(extract_existing_issue_number("see #1"), Some(1));
    assert_eq!(extract_existing_issue_number("#42"), Some(42));
}

#[test]
fn extract_existing_issue_number_matches_issue_word_form() {
    assert_eq!(extract_existing_issue_number("fix issue 915"), Some(915));
    assert_eq!(extract_existing_issue_number("fix issue #915"), Some(915));
    assert_eq!(
        extract_existing_issue_number("address issue number 42 today"),
        Some(42)
    );
    assert_eq!(
        extract_existing_issue_number("close issue id 7 finally"),
        Some(7)
    );
}

#[test]
fn extract_existing_issue_number_is_case_insensitive() {
    assert_eq!(extract_existing_issue_number("Fix ISSUE 915"), Some(915));
    assert_eq!(extract_existing_issue_number("Issue Number 42"), Some(42));
    assert_eq!(extract_existing_issue_number("ISSUE ID 7"), Some(7));
}

#[test]
fn extract_existing_issue_number_rejects_html_entity() {
    // `&#915;` is an HTML numeric character reference, not an issue ref.
    assert_eq!(extract_existing_issue_number("text &#915; more"), None);
}

#[test]
fn extract_existing_issue_number_rejects_embedded_alphanumeric() {
    // `foo#915bar` and `915abc` should not match.
    assert_eq!(extract_existing_issue_number("foo#915bar"), None);
    assert_eq!(extract_existing_issue_number("see #915abc"), None);
}

#[test]
fn extract_existing_issue_number_rejects_zero() {
    assert_eq!(extract_existing_issue_number("see #0"), None);
    assert_eq!(extract_existing_issue_number("issue 0"), None);
}
