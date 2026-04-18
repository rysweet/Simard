//! Additional unit tests for engineer_loop::selection — edge cases for
//! `sanitize_issue_title`, `select_engineer_action`, and related helpers.

use std::path::PathBuf;

use super::selection::*;
use super::types::*;

fn make_clean_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/tmp/test-repo"),
        repo_root: PathBuf::from("/tmp/test-repo"),
        branch: "main".into(),
        head: "abc1234".into(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    }
}

// ── sanitize_issue_title ────────────────────────────────────────

#[test]
fn sanitize_issue_title_short_title_unchanged() {
    let title = sanitize_issue_title("Fix broken CI pipeline");
    assert_eq!(title, "Fix broken CI pipeline");
}

#[test]
fn sanitize_issue_title_empty_input() {
    let title = sanitize_issue_title("");
    assert_eq!(title, "");
}

#[test]
fn sanitize_issue_title_whitespace_only_collapses() {
    let title = sanitize_issue_title("   \t   \n   ");
    assert_eq!(title, "");
}

#[test]
fn sanitize_issue_title_multiple_newlines_become_spaces() {
    let title = sanitize_issue_title("first\n\nsecond\r\nthird");
    assert_eq!(title, "first second third");
}

#[test]
fn sanitize_issue_title_exactly_at_limit_not_truncated() {
    let title_256 = "a".repeat(256);
    let result = sanitize_issue_title(&title_256);
    assert_eq!(result.len(), 256);
    assert!(!result.contains('…'));
}

#[test]
fn sanitize_issue_title_one_over_limit_truncated() {
    // Build a string of 257 chars with word boundaries
    let words = "word ".repeat(60); // 300 chars
    let result = sanitize_issue_title(&words);
    assert!(result.len() <= 260); // 256 + ellipsis char
    assert!(result.ends_with('…'));
}

#[test]
fn sanitize_issue_title_long_single_word_truncated_at_limit() {
    // No word boundary to split at — truncates at exactly MAX_ISSUE_TITLE_LEN
    let long_word = "x".repeat(300);
    let result = sanitize_issue_title(&long_word);
    assert!(result.ends_with('…'));
    // Should be 256 x's + ellipsis
    assert!(result.len() <= 260);
}

#[test]
fn sanitize_issue_title_extra_spaces_collapsed() {
    let title = sanitize_issue_title("  too   many    spaces  ");
    assert_eq!(title, "too many spaces");
}

// ── select_cargo_action variants ────────────────────────────────

#[test]
fn select_cargo_action_cargo_build_detected() {
    let action = select_cargo_action("cargo build the project", "");
    assert_eq!(action.label, "cargo-check");
}

#[test]
fn select_cargo_action_compilation_check_detected() {
    let action = select_cargo_action("run a compilation check", "");
    assert_eq!(action.label, "cargo-check");
}

#[test]
fn select_cargo_action_note_appears_in_rationale() {
    let action = select_cargo_action("cargo test", " (note: decided in meeting)");
    assert!(action.rationale.contains("decided in meeting"));
}

// ── select_shell_command edge cases ─────────────────────────────

#[test]
fn select_shell_command_gh_allowed() {
    let action = select_shell_command("run gh issue list", "");
    assert!(action.is_some());
    let action = action.unwrap();
    assert!(action.argv.contains(&"gh".to_string()));
}

#[test]
fn select_shell_command_rustfmt_allowed() {
    let action = select_shell_command("run rustfmt src/main.rs", "");
    assert!(action.is_some());
}

#[test]
fn select_shell_command_npm_blocked() {
    let action = select_shell_command("run npm install", "");
    assert!(action.is_none());
}

// ── carry_forward_note ──────────────────────────────────────────

#[test]
fn carry_forward_note_three_decisions() {
    let inspection = RepoInspection {
        carried_meeting_decisions: vec!["d1".into(), "d2".into(), "d3".into()],
        ..make_clean_inspection()
    };
    let note = carry_forward_note(&inspection);
    assert!(note.contains("3 meeting decision records"));
}

// ── extract_content_body ────────────────────────────────────────

#[test]
fn extract_content_body_preserves_multiline_content() {
    let objective = "create file\ncontent: begin\nline1\nline2\nline3";
    let body = extract_content_body(objective);
    assert_eq!(body, "line1\nline2\nline3");
}

#[test]
fn extract_content_body_empty_after_content_directive() {
    let objective = "some preamble\ncontent:";
    let body = extract_content_body(objective);
    assert!(body.is_empty());
}

// ── select_git_commit ───────────────────────────────────────────

#[test]
fn select_git_commit_verification_steps_nonempty() {
    let action = select_git_commit("commit initial scaffold", "");
    assert!(!action.verification_steps.is_empty());
}

#[test]
fn select_git_commit_argv_starts_with_git() {
    let action = select_git_commit("commit refactor types", "");
    assert_eq!(action.argv[0], "git");
    assert_eq!(action.argv[1], "commit");
}
