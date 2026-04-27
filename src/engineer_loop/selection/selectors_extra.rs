//! Cargo-action selector + action-achievability checks.

use super::{extract_existing_issue_number, tokenise_target_argv};

use crate::engineer_loop::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind, GitCommitRequest,
    OpenIssueRequest, RepoInspection, SelectedEngineerAction, ShellCommandRequest,
    StructuredEditRequest, extract_command_from_objective, extract_file_path_from_objective,
    is_prose_fragment, parse_structured_edit_request, validate_repo_relative_path,
};

use crate::engineer_loop::SHELL_COMMAND_ALLOWLIST;

pub fn select_cargo_action(objective: &str, note: &str) -> SelectedEngineerAction {
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
#[allow(dead_code)] // retained for tests + back-compat callers
pub fn is_keyword_action_achievable(action: &AnalyzedAction, objective: &str) -> bool {
    is_action_achievable(action, objective, None)
}

/// Like [`is_keyword_action_achievable`] but also considers the LLM's
/// explicit `PlanStep.target` field. RunShellCommand is achievable when
/// the target tokenises to an allowlisted command, even if the objective
/// itself is prose.
pub fn is_action_achievable(
    action: &AnalyzedAction,
    objective: &str,
    llm_target: Option<&str>,
) -> bool {
    match action {
        // These are always safe — they don't mutate the repo.
        AnalyzedAction::ReadOnlyScan | AnalyzedAction::CargoTest => true,
        // OpenIssue: only valid when the objective explicitly asks to *create* an issue
        // via a known prefix, OR references an existing issue number (which routes to
        // the verify path in `select_open_issue`). Bare prose like "Report a bug" or
        // "fix the issue" no longer qualifies.
        AnalyzedAction::OpenIssue => {
            if extract_existing_issue_number(objective).is_some() {
                return true;
            }
            let lower = objective.trim_start().to_lowercase();
            lower.starts_with("track ")
                || lower.starts_with("file an issue for")
                || lower.starts_with("create an issue")
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
        // CreateFile / AppendToFile: need a discernible file path in the objective
        // OR in the LLM's target field.
        AnalyzedAction::CreateFile | AnalyzedAction::AppendToFile => {
            if let Some(target) = llm_target
                && !target.trim().is_empty()
            {
                return true;
            }
            extract_file_path_from_objective(objective).is_some()
        }
        // StructuredTextReplace: needs edit-like directives.
        AnalyzedAction::StructuredTextReplace => {
            let lower = objective.to_lowercase();
            lower.contains("replace") || lower.contains("edit-file") || lower.contains("update")
        }
        // RunShellCommand: achievable when either the LLM provided a target
        // whose first token is allowlisted, or the objective contains an
        // extractable allowlisted command.
        AnalyzedAction::RunShellCommand => {
            if let Some(target) = llm_target {
                let argv = tokenise_target_argv(target);
                if let Some(first) = argv.first()
                    && SHELL_COMMAND_ALLOWLIST.contains(&first.as_str())
                {
                    return true;
                }
            }
            extract_command_from_objective(objective).is_some()
        }
    }
}
