use crate::error::{SimardError, SimardResult};

use super::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind, GitCommitRequest,
    OpenIssueRequest, RepoInspection, SelectedEngineerAction, ShellCommandRequest,
    StructuredEditRequest, extract_command_from_objective, extract_file_path_from_objective,
    is_prose_fragment, parse_structured_edit_request, validate_repo_relative_path,
};

use super::SHELL_COMMAND_ALLOWLIST;

pub(crate) fn carry_forward_note(inspection: &RepoInspection) -> String {
    if inspection.carried_meeting_decisions.is_empty() {
        String::new()
    } else {
        format!(
            " Shared state root also carries {} meeting decision record{}, so the engineer loop keeps that handoff visible while choosing the next safe repo-native action.",
            inspection.carried_meeting_decisions.len(),
            if inspection.carried_meeting_decisions.len() == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

pub(crate) fn select_structured_edit(
    inspection: &RepoInspection,
    edit_request: StructuredEditRequest,
    note: &str,
) -> SimardResult<SelectedEngineerAction> {
    if inspection.worktree_dirty {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "structured text replacement objectives require a clean git worktree so Simard does not overwrite unrelated local changes".to_string(),
        });
    }
    let relative_path = validate_repo_relative_path(&edit_request.relative_path)?;
    let verify_contains = edit_request.verify_contains.clone();
    Ok(SelectedEngineerAction {
        label: "structured-text-replace".to_string(),
        rationale: format!(
            "Objective includes explicit edit-file/replace/with/verify-contains directives, so the next honest bounded engineer action is to update '{}' once, then verify the requested text is present and visible through git state.{note}",
            relative_path
        ),
        argv: vec![
            "simard-structured-edit".to_string(),
            relative_path.clone(),
            "replace-once".to_string(),
        ],
        plan_summary: format!(
            "Inspect the clean repo, replace the requested text once in '{}', then verify the file content and git state reflect exactly that bounded local change.",
            relative_path
        ),
        verification_steps: vec![
            format!("confirm '{}' contains '{}'", relative_path, verify_contains),
            format!(
                "confirm git status reports '{}' as the only changed file",
                relative_path
            ),
            "confirm carried meeting decisions and active goals stayed stable".to_string(),
        ],
        expected_changed_files: vec![relative_path.clone()],
        kind: EngineerActionKind::StructuredTextReplace(StructuredEditRequest {
            relative_path,
            ..edit_request
        }),
    })
}

pub(crate) fn extract_content_body(objective: &str) -> String {
    objective
        .lines()
        .skip_while(|l| !l.to_lowercase().starts_with("content:"))
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn select_create_file(
    objective: &str,
    note: &str,
) -> Option<SimardResult<SelectedEngineerAction>> {
    let path = extract_file_path_from_objective(objective)?;
    let relative_path = match validate_repo_relative_path(&path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let content = extract_content_body(objective);
    Some(Ok(SelectedEngineerAction {
        label: "create-file".to_string(),
        rationale: format!("Objective requests creating a new file at '{relative_path}'.{note}"),
        argv: vec!["simard-create-file".to_string(), relative_path.clone()],
        plan_summary: format!(
            "Create file '{}' with the specified content, then verify the file exists.",
            relative_path
        ),
        verification_steps: vec![
            format!("confirm '{}' exists", relative_path),
            "confirm file content matches request".to_string(),
        ],
        expected_changed_files: vec![relative_path.clone()],
        kind: EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path,
            content,
        }),
    }))
}

pub(crate) fn select_append_to_file(
    objective: &str,
    note: &str,
) -> Option<SimardResult<SelectedEngineerAction>> {
    let path = extract_file_path_from_objective(objective)?;
    let relative_path = match validate_repo_relative_path(&path) {
        Ok(p) => p,
        Err(e) => return Some(Err(e)),
    };
    let content = extract_content_body(objective);
    Some(Ok(SelectedEngineerAction {
        label: "append-to-file".to_string(),
        rationale: format!("Objective requests appending content to '{relative_path}'.{note}"),
        argv: vec!["simard-append-file".to_string(), relative_path.clone()],
        plan_summary: format!(
            "Append content to '{}', then verify the file contains the appended text.",
            relative_path
        ),
        verification_steps: vec![format!(
            "confirm '{}' contains appended content",
            relative_path
        )],
        expected_changed_files: vec![relative_path.clone()],
        kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path,
            content,
        }),
    }))
}

pub(crate) fn select_shell_command(objective: &str, note: &str) -> Option<SelectedEngineerAction> {
    let argv = extract_command_from_objective(objective)?;
    let first = argv.first().cloned().unwrap_or_default();
    if !SHELL_COMMAND_ALLOWLIST.contains(&first.as_str()) {
        return None;
    }
    Some(SelectedEngineerAction {
        label: "run-shell-command".to_string(),
        rationale: format!(
            "Objective requests running '{}', which is in the shell allowlist.{note}",
            argv.join(" ")
        ),
        argv: argv.clone(),
        plan_summary: format!("Execute '{}' and capture output.", argv.join(" ")),
        verification_steps: vec![format!("confirm '{}' exits with status 0", argv.join(" "))],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest { argv }),
    })
}

pub(crate) fn select_git_commit(objective: &str, note: &str) -> SelectedEngineerAction {
    let raw_message = {
        let lower = objective.to_lowercase();
        if let Some(idx) = lower.find("commit ") {
            objective[idx + 7..].trim().to_string()
        } else {
            objective.to_string()
        }
    };
    let message = sanitize_commit_message(&raw_message);
    SelectedEngineerAction {
        label: "git-commit".to_string(),
        rationale: format!(
            "Objective requests committing changes with message: '{}'.{note}",
            message
        ),
        argv: vec![
            "git".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            message.clone(),
        ],
        plan_summary: "Stage all changes and create a git commit.".to_string(),
        verification_steps: vec!["confirm HEAD changed (new commit created)".to_string()],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::GitCommit(GitCommitRequest { message }),
    }
}

/// Maximum length for a GitHub issue title created by the engineer loop.
const MAX_ISSUE_TITLE_LEN: usize = 256;

pub(crate) fn select_open_issue(
    objective: &str,
    note: &str,
) -> SimardResult<SelectedEngineerAction> {
    let title = sanitize_issue_title(objective);

    if title.is_empty() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "refusing to create a GitHub issue with an empty title".to_string(),
        });
    }

    Ok(SelectedEngineerAction {
        label: "open-issue".to_string(),
        rationale: format!("Objective requests opening a GitHub issue.{note}"),
        argv: vec![
            "gh".to_string(),
            "issue".to_string(),
            "create".to_string(),
            "--title".to_string(),
            title.clone(),
        ],
        plan_summary: "Create a GitHub issue via gh CLI.".to_string(),
        verification_steps: vec!["confirm issue URL is returned in stdout".to_string()],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::OpenIssue(OpenIssueRequest {
            title,
            body: String::new(),
            labels: Vec::new(),
        }),
    })
}

/// Strip characters that are dangerous in shell arguments or would produce
/// malformed CLI commands when embedded in `--title` / `-m` values.
fn strip_shell_unsafe(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '`' | '$'
                    | '\\'
                    | '"'
                    | '\''
                    | '|'
                    | ';'
                    | '&'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '!'
                    | '\0'
            )
        })
        .collect()
}

/// Sanitize an objective string for use as a GitHub issue title.
///
/// Strips newlines, removes shell-unsafe characters, collapses whitespace,
/// and truncates to a reasonable length.
pub(crate) fn sanitize_issue_title(raw: &str) -> String {
    let cleaned = strip_shell_unsafe(raw);
    let single_line: String = cleaned
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if single_line.len() <= MAX_ISSUE_TITLE_LEN {
        single_line
    } else {
        let truncated = &single_line[..MAX_ISSUE_TITLE_LEN];
        // Cut at the last word boundary to avoid mid-word truncation.
        match truncated.rfind(' ') {
            Some(pos) if pos > MAX_ISSUE_TITLE_LEN / 2 => format!("{}…", &truncated[..pos]),
            _ => format!("{truncated}…"),
        }
    }
}

/// Sanitize a commit message: strip newlines, remove shell-unsafe characters,
/// collapse whitespace, and truncate.
const MAX_COMMIT_MESSAGE_LEN: usize = 256;

pub(crate) fn sanitize_commit_message(raw: &str) -> String {
    let cleaned = strip_shell_unsafe(raw);
    let single_line: String = cleaned
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if single_line.len() <= MAX_COMMIT_MESSAGE_LEN {
        single_line
    } else {
        let truncated = &single_line[..MAX_COMMIT_MESSAGE_LEN];
        match truncated.rfind(' ') {
            Some(pos) if pos > MAX_COMMIT_MESSAGE_LEN / 2 => format!("{}…", &truncated[..pos]),
            _ => format!("{truncated}…"),
        }
    }
}

pub(crate) fn select_cargo_action(objective: &str, note: &str) -> SelectedEngineerAction {
    let obj_lower = objective.to_lowercase();
    if obj_lower.contains("cargo test")
        || obj_lower.contains("run tests")
        || obj_lower.contains("test suite")
        || obj_lower.contains("run the tests")
    {
        return SelectedEngineerAction {
            label: "cargo-test".to_string(),
            rationale: format!(
                "Objective explicitly requests running tests and a Cargo.toml is present, so the next bounded action is to run the test suite and report results.{note}"
            ),
            argv: vec![
                "cargo".to_string(),
                "test".to_string(),
                "--all-features".to_string(),
                "--locked".to_string(),
            ],
            plan_summary:
                "Run the full Rust test suite, capture results, and verify the build is healthy."
                    .to_string(),
            verification_steps: vec![
                "confirm cargo test exits with status 0".to_string(),
                "confirm test result line reports 0 failures".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::CargoTest,
        };
    }
    if obj_lower.contains("cargo check")
        || obj_lower.contains("compilation check")
        || obj_lower.contains("check compilation")
        || obj_lower.contains("cargo build")
    {
        return SelectedEngineerAction {
            label: "cargo-check".to_string(),
            rationale: format!(
                "Objective mentions build/check and a Cargo.toml is present, so the next bounded action is to run cargo check and report compilation status.{note}"
            ),
            argv: vec![
                "cargo".to_string(),
                "check".to_string(),
                "--all-targets".to_string(),
                "--all-features".to_string(),
            ],
            plan_summary: "Run cargo check to verify the codebase compiles cleanly.".to_string(),
            verification_steps: vec![
                "confirm cargo check exits with status 0".to_string(),
                "confirm no compilation errors in output".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::CargoCheck,
        };
    }
    SelectedEngineerAction {
        label: "cargo-metadata-scan".to_string(),
        rationale: format!(
            "Detected a Rust workspace via Cargo.toml, so the next honest v1 action is a local argv-only cargo metadata scan that inspects the workspace graph without pretending remote orchestration exists.{note}"
        ),
        argv: vec!["cargo".to_string(), "metadata".to_string(), "--format-version".to_string(), "1".to_string(), "--no-deps".to_string()],
        plan_summary: "Inspect the repo, query Cargo metadata without mutating files, and verify repo grounding stayed stable.".to_string(),
        verification_steps: vec![
            "confirm cargo metadata returns valid workspace JSON".to_string(),
            "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
            "confirm carried meeting decisions and active goals stayed stable".to_string(),
        ],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::ReadOnlyScan,
    }
}

/// Check whether a keyword-analyzed action is achievable given the current
/// objective and context. Returns `true` when the action is safe to proceed
/// with, `false` when it should be demoted to a safe default.
pub(crate) fn is_keyword_action_achievable(action: &AnalyzedAction, objective: &str) -> bool {
    match action {
        // These are always safe — they don't mutate the repo.
        AnalyzedAction::ReadOnlyScan | AnalyzedAction::CargoTest => true,
        // OpenIssue: only valid when the objective explicitly asks to *create* / *open* an issue,
        // not when "issue" appears as a reference (e.g. "fix issue #891").
        AnalyzedAction::OpenIssue => {
            let lower = objective.to_lowercase();
            (lower.contains("open") || lower.contains("file") || lower.contains("report"))
                && (lower.contains("issue") || lower.contains("bug"))
        }
        // GitCommit: only valid when the objective is specifically about committing,
        // and the text after "commit" is not a prose fragment.
        AnalyzedAction::GitCommit => {
            let lower = objective.to_lowercase();
            if !(lower.contains("commit") || lower.contains("save changes")) {
                return false;
            }
            // Reject if the message portion is prose (e.g. "commit -m and open PR against #890.")
            if let Some(idx) = lower.find("commit ") {
                let after_commit = &objective[idx + 7..];
                if is_prose_fragment(after_commit) {
                    return false;
                }
            }
            true
        }
        // CreateFile / AppendToFile: need a discernible file path in the objective.
        AnalyzedAction::CreateFile | AnalyzedAction::AppendToFile => {
            extract_file_path_from_objective(objective).is_some()
        }
        // StructuredTextReplace: needs edit-like directives.
        AnalyzedAction::StructuredTextReplace => {
            let lower = objective.to_lowercase();
            lower.contains("replace") || lower.contains("edit-file") || lower.contains("update")
        }
        // RunShellCommand: needs an extractable command that is not a prose fragment.
        AnalyzedAction::RunShellCommand => extract_command_from_objective(objective).is_some(),
    }
}

pub(crate) fn select_engineer_action(
    inspection: &RepoInspection,
    objective: &str,
) -> SimardResult<SelectedEngineerAction> {
    let note = carry_forward_note(inspection);

    if let Some(edit_request) = parse_structured_edit_request(objective)? {
        return select_structured_edit(inspection, edit_request, &note);
    }

    // LLM-based planning. If unavailable, use keyword analysis as the
    // base strategy — keyword analysis is the foundational
    // implementation, LLM planning is an enhancement.
    let mut used_keyword_analysis = false;
    let analyzed = match crate::engineer_plan::plan_objective(objective, inspection) {
        Ok(plan) if !plan.steps().is_empty() => {
            let action = &plan.steps()[0].action;
            // Validate that the LLM-chosen action is a known variant we can handle.
            if is_keyword_action_achievable(action, objective) {
                action.clone()
            } else {
                tracing::warn!(
                    "LLM plan selected action {:?} which is not achievable for this objective; \
                     using keyword analysis instead",
                    action
                );
                used_keyword_analysis = true;
                super::types::analyze_objective(objective)
            }
        }
        Ok(_) => {
            tracing::debug!("LLM plan returned empty steps, using keyword analysis");
            used_keyword_analysis = true;
            super::types::analyze_objective(objective)
        }
        Err(e) => {
            tracing::warn!("LLM planning failed: {e} — using keyword analysis");
            used_keyword_analysis = true;
            super::types::analyze_objective(objective)
        }
    };

    // Always validate keyword-analyzed actions. Keyword matching can trigger
    // on incidental words (e.g. "issue" in "fix issue #835" → OpenIssue,
    // or "run" in "run the migration and open PR" → RunShellCommand with
    // prose fragments as the argv).
    let analyzed = if used_keyword_analysis && !is_keyword_action_achievable(&analyzed, objective) {
        tracing::warn!(
            "suppressed {:?} action — keyword matched but action is not \
             achievable for this objective; defaulting to safe action",
            analyzed
        );
        AnalyzedAction::ReadOnlyScan
    } else {
        analyzed
    };

    match analyzed {
        AnalyzedAction::CreateFile => {
            if let Some(result) = select_create_file(objective, &note) {
                return result;
            }
        }
        AnalyzedAction::AppendToFile => {
            if let Some(result) = select_append_to_file(objective, &note) {
                return result;
            }
        }
        AnalyzedAction::RunShellCommand => {
            if let Some(action) = select_shell_command(objective, &note) {
                return Ok(action);
            }
        }
        AnalyzedAction::GitCommit => return Ok(select_git_commit(objective, &note)),
        AnalyzedAction::OpenIssue => return select_open_issue(objective, &note),
        _ => {}
    }

    if inspection.repo_root.join("Cargo.toml").is_file() {
        return Ok(select_cargo_action(objective, &note));
    }

    if inspection.repo_root.join(".git").exists() {
        return Ok(SelectedEngineerAction {
            label: "git-tracked-file-scan".to_string(),
            rationale: format!(
                "No repo-native language manifest was detected, so the loop falls back to a local argv-only scan of tracked files instead of inventing unsupported tooling.{note}"
            ),
            argv: vec!["git".to_string(), "ls-files".to_string(), "--cached".to_string()],
            plan_summary: "Inspect the repo, enumerate tracked files without mutating content, and verify repo grounding stayed stable.".to_string(),
            verification_steps: vec![
                "confirm at least one tracked file is reported".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
                "confirm carried meeting decisions and active goals stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::ReadOnlyScan,
        });
    }

    Err(SimardError::UnsupportedEngineerAction {
        reason: format!(
            "workspace '{}' is repo-grounded but exposes no supported local-first action policy",
            inspection.repo_root.display()
        ),
    })
}

#[cfg(test)]
mod tests {
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
}
