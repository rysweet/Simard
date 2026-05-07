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
fn extract_existing_issue_number_rejects_overflow() {
    // u64::MAX is 20 digits; 21 digits cannot fit.
    let huge = "see #999999999999999999999 here";
    assert_eq!(extract_existing_issue_number(huge), None);
}

#[test]
fn extract_existing_issue_number_requires_separator_after_issue() {
    // `issuenumber42` is not "issue" + separator + N.
    assert_eq!(extract_existing_issue_number("issuenumber42"), None);
    assert_eq!(extract_existing_issue_number("subissue 5"), None);
}

#[test]
fn extract_existing_issue_number_returns_none_when_absent() {
    assert_eq!(extract_existing_issue_number(""), None);
    assert_eq!(extract_existing_issue_number("just some prose"), None);
    assert_eq!(extract_existing_issue_number("version 1.2.3"), None);
}

#[test]
fn extract_existing_issue_number_picks_earliest_across_patterns() {
    // `issue 42` appears before `#7` → 42 wins.
    assert_eq!(extract_existing_issue_number("issue 42 fixes #7"), Some(42));
    // `#7` appears before `issue 42` → 7 wins.
    assert_eq!(
        extract_existing_issue_number("see #7 about issue 42"),
        Some(7)
    );
}

#[test]
fn select_open_issue_emits_verify_path_for_hash_reference() {
    let action = select_open_issue("add-more-gym-benchmark-scenarios for #915", "")
        .expect("verify path should not error");
    assert_eq!(action.label, "verify-existing-issue");
    assert_eq!(
        action.argv,
        vec![
            "gh".to_string(),
            "issue".to_string(),
            "view".to_string(),
            "915".to_string(),
        ]
    );
    assert!(
        !action.argv.iter().any(|a| a == "create"),
        "verify path must not contain `create`: {:?}",
        action.argv
    );
}

#[test]
fn select_open_issue_emits_verify_path_for_issue_word_reference() {
    let action = select_open_issue("address issue 915 with new scenarios", "")
        .expect("verify path should not error");
    assert_eq!(action.label, "verify-existing-issue");
    assert_eq!(action.argv[0], "gh");
    assert_eq!(action.argv[1], "issue");
    assert_eq!(action.argv[2], "view");
    assert_eq!(action.argv[3], "915");
}

#[test]
fn select_open_issue_verify_path_uses_open_issue_kind() {
    let action = select_open_issue("fix issue #915", "").unwrap();
    match action.kind {
        EngineerActionKind::OpenIssue(req) => {
            assert!(req.body.is_empty(), "verify-path body must be empty");
            assert!(
                req.title.contains("915"),
                "title should reference the issue number for trace clarity: {}",
                req.title
            );
        }
        other => panic!("expected OpenIssue kind, got {other:?}"),
    }
}

#[test]
fn select_open_issue_create_path_preserved_when_no_issue_number() {
    // No issue number reference → original create-path behavior must hold.
    let action = select_open_issue("file an issue for the new crash", "").unwrap();
    assert_eq!(action.label, "open-issue");
    assert!(action.argv.contains(&"create".to_string()));
    assert!(action.argv.contains(&"--title".to_string()));
    assert!(!action.argv.contains(&"view".to_string()));
}

// ------------------------------------------------------------------
// is_keyword_action_achievable: tightened OpenIssue gate
// ------------------------------------------------------------------

#[test]
fn keyword_gate_open_issue_accepts_track_prefix() {
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "track the flaky build failures"
    ));
}

#[test]
fn keyword_gate_open_issue_accepts_file_an_issue_for_prefix() {
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "file an issue for the planner regression"
    ));
}

#[test]
fn keyword_gate_open_issue_accepts_create_an_issue_prefix() {
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "create an issue about the broken loop"
    ));
}

#[test]
fn keyword_gate_open_issue_accepts_existing_issue_reference() {
    // Existing-issue references route through the verify path → achievable.
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "add-more-gym-benchmark-scenarios for issue #915"
    ));
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "fix issue 891 before release"
    ));
}

#[test]
fn keyword_gate_open_issue_is_case_insensitive_for_whitelist() {
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "Track the flaky build"
    ));
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "  File an issue for X"
    ));
    assert!(is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "CREATE AN ISSUE about Y"
    ));
}

#[test]
fn keyword_gate_open_issue_rejects_unrelated_prose() {
    // "Report a bug" no longer matches the tightened whitelist
    // and contains no existing-issue reference.
    assert!(!is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "Report a bug"
    ));
    assert!(!is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "open the documentation"
    ));
    assert!(!is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "investigate the planner"
    ));
}

#[test]
fn keyword_gate_open_issue_rejects_track_as_substring() {
    // "track" must be the leading word, not embedded.
    assert!(!is_keyword_action_achievable(
        &AnalyzedAction::OpenIssue,
        "backtrack the change"
    ));
}

// ──────────────────────────────────────────────────────────
// Regression coverage for fix/planner-shape-and-backoff:
// selector now consumes PlanStep.target instead of re-extracting
// argv from prose. These tests pin the new behavior.
// ──────────────────────────────────────────────────────────

#[test]
fn tokenise_target_argv_strips_backticks() {
    let argv = tokenise_target_argv("`gh issue view 915`");
    assert_eq!(argv, vec!["gh", "issue", "view", "915"]);
}

#[test]
fn tokenise_target_argv_strips_double_quotes() {
    let argv = tokenise_target_argv("\"cargo check --lib\"");
    assert_eq!(argv, vec!["cargo", "check", "--lib"]);
}

#[test]
fn tokenise_target_argv_strips_single_quotes() {
    let argv = tokenise_target_argv("'git status'");
    assert_eq!(argv, vec!["git", "status"]);
}

#[test]
fn tokenise_target_argv_handles_extra_whitespace() {
    let argv = tokenise_target_argv("  gh   issue  view   915  ");
    assert_eq!(argv, vec!["gh", "issue", "view", "915"]);
}

#[test]
fn tokenise_target_argv_empty_returns_empty() {
    assert!(tokenise_target_argv("").is_empty());
    assert!(tokenise_target_argv("   ").is_empty());
    assert!(tokenise_target_argv("``").is_empty());
}

#[test]
fn select_shell_command_from_argv_accepts_gh() {
    let action = select_shell_command_from_argv(
        vec!["gh".into(), "issue".into(), "view".into(), "915".into()],
        "note",
        "test",
    )
    .expect("gh is allowlisted");
    assert_eq!(action.argv[0], "gh");
    assert_eq!(action.argv.len(), 4);
}

#[test]
fn select_shell_command_from_argv_rejects_non_allowlisted() {
    let action = select_shell_command_from_argv(
        vec!["curl".into(), "https://example.com".into()],
        "note",
        "test",
    );
    assert!(action.is_none(), "curl must not be allowlisted");
}

#[test]
fn select_shell_command_from_argv_rejects_empty_argv() {
    assert!(select_shell_command_from_argv(vec![], "note", "test").is_none());
}

#[test]
fn is_action_achievable_runshell_with_llm_target_passes() {
    // Objective is multi-paragraph prose that the keyword extractor
    // would fail on, but the LLM provided a concrete allowlisted target.
    let prose = "Investigate issue 915 thoroughly. Read its body. Comment on findings.\n\
                 Several paragraphs of context. No literal command in this prose.";
    assert!(is_action_achievable(
        &AnalyzedAction::RunShellCommand,
        prose,
        Some("gh issue view 915"),
    ));
}

#[test]
fn is_action_achievable_runshell_rejects_non_allowlisted_target() {
    assert!(!is_action_achievable(
        &AnalyzedAction::RunShellCommand,
        "anything",
        Some("curl https://evil.example.com"),
    ));
}

#[test]
fn is_action_achievable_runshell_empty_target_falls_back_to_keyword_gate() {
    // Empty target ⇒ behaves like the legacy gate over objective text.
    let achievable_via_objective = is_action_achievable(
        &AnalyzedAction::RunShellCommand,
        "run cargo check",
        Some(""),
    );
    let legacy = is_keyword_action_achievable(&AnalyzedAction::RunShellCommand, "run cargo check");
    assert_eq!(achievable_via_objective, legacy);
}

#[test]
fn is_action_achievable_create_file_with_target() {
    assert!(is_action_achievable(
        &AnalyzedAction::CreateFile,
        "objective without filename",
        Some("src/foo.rs"),
    ));
}

#[test]
fn is_action_achievable_create_file_empty_target() {
    // Empty target ⇒ delegate to legacy gate on objective.
    let result = is_action_achievable(
        &AnalyzedAction::CreateFile,
        "objective without filename",
        Some(""),
    );
    assert_eq!(
        result,
        is_keyword_action_achievable(&AnalyzedAction::CreateFile, "objective without filename")
    );
}
