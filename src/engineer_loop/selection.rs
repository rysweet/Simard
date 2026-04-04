use crate::error::{SimardError, SimardResult};

use super::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind, GitCommitRequest,
    OpenIssueRequest, RepoInspection, SelectedEngineerAction, ShellCommandRequest,
    StructuredEditRequest, extract_command_from_objective, extract_file_path_from_objective,
    parse_structured_edit_request, validate_repo_relative_path,
};

use super::SHELL_COMMAND_ALLOWLIST;

fn carry_forward_note(inspection: &RepoInspection) -> String {
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

fn select_structured_edit(
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

fn extract_content_body(objective: &str) -> String {
    objective
        .lines()
        .skip_while(|l| !l.to_lowercase().starts_with("content:"))
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
}

fn select_create_file(objective: &str, note: &str) -> Option<SimardResult<SelectedEngineerAction>> {
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

fn select_append_to_file(
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

fn select_shell_command(objective: &str, note: &str) -> Option<SelectedEngineerAction> {
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

fn select_git_commit(objective: &str, note: &str) -> SelectedEngineerAction {
    let message = {
        let lower = objective.to_lowercase();
        if let Some(idx) = lower.find("commit ") {
            objective[idx + 7..].trim().to_string()
        } else {
            objective.to_string()
        }
    };
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

fn select_open_issue(objective: &str, note: &str) -> SelectedEngineerAction {
    let title = objective.to_string();
    SelectedEngineerAction {
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
    }
}

fn select_cargo_action(objective: &str, note: &str) -> SelectedEngineerAction {
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

pub(crate) fn select_engineer_action(
    inspection: &RepoInspection,
    objective: &str,
) -> SimardResult<SelectedEngineerAction> {
    let note = carry_forward_note(inspection);

    if let Some(edit_request) = parse_structured_edit_request(objective)? {
        return select_structured_edit(inspection, edit_request, &note);
    }

    // Try LLM-based planning first; fall back to keyword analysis.
    let analyzed = match crate::engineer_plan::plan_objective(objective, inspection) {
        Ok(plan) if !plan.steps().is_empty() => plan.steps()[0].action.clone(),
        _ => super::types::analyze_objective(objective),
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
        AnalyzedAction::OpenIssue => return Ok(select_open_issue(objective, &note)),
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

    fn make_inspection(dirty: bool, decisions: Vec<String>) -> RepoInspection {
        RepoInspection {
            workspace_root: PathBuf::from("/fake/workspace"),
            repo_root: PathBuf::from("/fake/repo"),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: dirty,
            changed_files: Vec::new(),
            active_goals: Vec::new(),
            carried_meeting_decisions: decisions,
            architecture_gap_summary: String::new(),
        }
    }

    // --- carry_forward_note ---

    #[test]
    fn carry_forward_note_empty_decisions() {
        let inspection = make_inspection(false, vec![]);
        assert_eq!(carry_forward_note(&inspection), "");
    }

    #[test]
    fn carry_forward_note_one_decision_singular() {
        let inspection = make_inspection(false, vec!["decision1".to_string()]);
        let note = carry_forward_note(&inspection);
        assert!(note.contains("1 meeting decision record"));
        // Should use singular (no trailing "s")
        assert!(!note.contains("records"));
    }

    #[test]
    fn carry_forward_note_multiple_decisions_plural() {
        let inspection = make_inspection(false, vec!["d1".into(), "d2".into()]);
        let note = carry_forward_note(&inspection);
        assert!(note.contains("2 meeting decision records"));
    }

    // --- extract_content_body ---

    #[test]
    fn extract_content_body_with_lines_after_content_directive() {
        let obj = "some header\ncontent:\nline one\nline two";
        assert_eq!(extract_content_body(obj), "line one\nline two");
    }

    #[test]
    fn extract_content_body_no_content_directive() {
        assert_eq!(extract_content_body("no directive here"), "");
    }

    #[test]
    fn extract_content_body_content_at_end_with_nothing_after() {
        assert_eq!(extract_content_body("content:"), "");
    }

    #[test]
    fn extract_content_body_case_insensitive_prefix() {
        let obj = "Content: ignored value\nactual body";
        assert_eq!(extract_content_body(obj), "actual body");
    }

    // --- select_shell_command ---

    #[test]
    fn select_shell_command_allowlisted_cargo() {
        let result = select_shell_command("run cargo fmt", "");
        assert!(result.is_some());
        let action = result.unwrap();
        assert_eq!(action.label, "run-shell-command");
        assert_eq!(action.argv[0], "cargo");
    }

    #[test]
    fn select_shell_command_allowlisted_git() {
        let result = select_shell_command("run git status", "");
        assert!(result.is_some());
        let action = result.unwrap();
        assert_eq!(action.argv[0], "git");
    }

    #[test]
    fn select_shell_command_not_allowlisted_returns_none() {
        let result = select_shell_command("run rm -rf /", "");
        assert!(result.is_none());
    }

    #[test]
    fn select_shell_command_no_run_keyword_returns_none() {
        assert!(select_shell_command("please do something", "").is_none());
    }

    #[test]
    fn select_shell_command_preserves_note_in_rationale() {
        let result = select_shell_command("run cargo fmt", " [note]").unwrap();
        assert!(result.rationale.contains("[note]"));
    }

    #[test]
    fn select_shell_command_execute_keyword() {
        let result = select_shell_command("execute git log", "");
        assert!(result.is_some());
        assert_eq!(result.unwrap().argv[0], "git");
    }

    // --- select_git_commit ---

    #[test]
    fn select_git_commit_extracts_message_after_commit() {
        let action = select_git_commit("commit fix typo in README", "");
        assert_eq!(action.label, "git-commit");
        match &action.kind {
            EngineerActionKind::GitCommit(req) => {
                assert_eq!(req.message, "fix typo in README");
            }
            _ => panic!("expected GitCommit kind"),
        }
    }

    #[test]
    fn select_git_commit_uses_full_objective_when_no_commit_keyword() {
        let action = select_git_commit("save the work", "");
        match &action.kind {
            EngineerActionKind::GitCommit(req) => {
                assert_eq!(req.message, "save the work");
            }
            _ => panic!("expected GitCommit kind"),
        }
    }

    #[test]
    fn select_git_commit_argv_contains_git_commit_m() {
        let action = select_git_commit("commit hello world", "");
        assert_eq!(action.argv[0], "git");
        assert_eq!(action.argv[1], "commit");
        assert_eq!(action.argv[2], "-m");
        assert_eq!(action.argv[3], "hello world");
    }

    // --- select_open_issue ---

    #[test]
    fn select_open_issue_builds_action_with_title() {
        let action = select_open_issue("fix the login page", "");
        assert_eq!(action.label, "open-issue");
        assert!(action.argv.contains(&"gh".to_string()));
        match &action.kind {
            EngineerActionKind::OpenIssue(req) => {
                assert_eq!(req.title, "fix the login page");
                assert!(req.body.is_empty());
                assert!(req.labels.is_empty());
            }
            _ => panic!("expected OpenIssue kind"),
        }
    }

    #[test]
    fn select_open_issue_note_appears_in_rationale() {
        let action = select_open_issue("test", " [extra]");
        assert!(action.rationale.contains("[extra]"));
    }

    // --- select_cargo_action ---

    #[test]
    fn select_cargo_action_cargo_test_keyword() {
        let action = select_cargo_action("cargo test the project", "");
        assert_eq!(action.label, "cargo-test");
        assert!(matches!(action.kind, EngineerActionKind::CargoTest));
    }

    #[test]
    fn select_cargo_action_run_tests() {
        assert_eq!(
            select_cargo_action("run tests please", "").label,
            "cargo-test"
        );
    }

    #[test]
    fn select_cargo_action_test_suite() {
        assert_eq!(
            select_cargo_action("test suite verification", "").label,
            "cargo-test"
        );
    }

    #[test]
    fn select_cargo_action_run_the_tests() {
        assert_eq!(
            select_cargo_action("run the tests now", "").label,
            "cargo-test"
        );
    }

    #[test]
    fn select_cargo_action_cargo_check() {
        let action = select_cargo_action("cargo check the build", "");
        assert_eq!(action.label, "cargo-check");
        assert!(matches!(action.kind, EngineerActionKind::CargoCheck));
    }

    #[test]
    fn select_cargo_action_cargo_build() {
        assert_eq!(
            select_cargo_action("cargo build the project", "").label,
            "cargo-check"
        );
    }

    #[test]
    fn select_cargo_action_compilation_check() {
        assert_eq!(
            select_cargo_action("compilation check", "").label,
            "cargo-check"
        );
    }

    #[test]
    fn select_cargo_action_check_compilation() {
        assert_eq!(
            select_cargo_action("check compilation errors", "").label,
            "cargo-check"
        );
    }

    #[test]
    fn select_cargo_action_default_falls_to_metadata_scan() {
        let action = select_cargo_action("inspect the workspace", "");
        assert_eq!(action.label, "cargo-metadata-scan");
        assert!(matches!(action.kind, EngineerActionKind::ReadOnlyScan));
    }

    #[test]
    fn select_cargo_action_metadata_has_no_deps_flag() {
        let action = select_cargo_action("what packages", "");
        assert!(action.argv.contains(&"--no-deps".to_string()));
    }

    // --- select_create_file ---

    #[test]
    fn select_create_file_with_valid_path_and_content() {
        let result = select_create_file("create src/lib.rs\ncontent:\nfn main() {}", "");
        assert!(result.is_some());
        let action = result.unwrap().unwrap();
        assert_eq!(action.label, "create-file");
        match &action.kind {
            EngineerActionKind::CreateFile(req) => {
                assert_eq!(req.relative_path, "src/lib.rs");
                assert_eq!(req.content, "fn main() {}");
            }
            _ => panic!("expected CreateFile kind"),
        }
    }

    #[test]
    fn select_create_file_no_path_returns_none() {
        assert!(select_create_file("create something", "").is_none());
    }

    #[test]
    fn select_create_file_absolute_path_returns_error() {
        let result = select_create_file("create /etc/passwd\ncontent:\nbad", "");
        assert!(result.is_some());
        let err = result.unwrap().unwrap_err();
        assert!(err.to_string().contains("must stay relative"));
    }

    // --- select_append_to_file ---

    #[test]
    fn select_append_to_file_with_valid_path() {
        let result = select_append_to_file("append to src/lib.rs\ncontent:\nnew line", "");
        assert!(result.is_some());
        let action = result.unwrap().unwrap();
        assert_eq!(action.label, "append-to-file");
        match &action.kind {
            EngineerActionKind::AppendToFile(req) => {
                assert_eq!(req.relative_path, "src/lib.rs");
            }
            _ => panic!("expected AppendToFile kind"),
        }
    }

    #[test]
    fn select_append_to_file_no_path_returns_none() {
        assert!(select_append_to_file("append something", "").is_none());
    }

    // --- select_structured_edit ---

    #[test]
    fn select_structured_edit_dirty_worktree_rejected() {
        let inspection = make_inspection(true, vec![]);
        let edit = StructuredEditRequest {
            relative_path: "src/lib.rs".to_string(),
            search: "old".to_string(),
            replacement: "new".to_string(),
            verify_contains: "new".to_string(),
        };
        let err = select_structured_edit(&inspection, edit, "").unwrap_err();
        assert!(err.to_string().contains("clean git worktree"));
    }

    #[test]
    fn select_structured_edit_clean_worktree_succeeds() {
        let inspection = make_inspection(false, vec![]);
        let edit = StructuredEditRequest {
            relative_path: "src/lib.rs".to_string(),
            search: "old".to_string(),
            replacement: "new".to_string(),
            verify_contains: "new".to_string(),
        };
        let action = select_structured_edit(&inspection, edit, "").unwrap();
        assert_eq!(action.label, "structured-text-replace");
        assert_eq!(action.expected_changed_files, vec!["src/lib.rs"]);
    }

    #[test]
    fn select_structured_edit_absolute_path_rejected() {
        let inspection = make_inspection(false, vec![]);
        let edit = StructuredEditRequest {
            relative_path: "/etc/passwd".to_string(),
            search: "old".to_string(),
            replacement: "new".to_string(),
            verify_contains: "new".to_string(),
        };
        let err = select_structured_edit(&inspection, edit, "").unwrap_err();
        assert!(err.to_string().contains("must stay relative"));
    }

    #[test]
    fn select_structured_edit_parent_traversal_rejected() {
        let inspection = make_inspection(false, vec![]);
        let edit = StructuredEditRequest {
            relative_path: "../escape.txt".to_string(),
            search: "old".to_string(),
            replacement: "new".to_string(),
            verify_contains: "new".to_string(),
        };
        let err = select_structured_edit(&inspection, edit, "").unwrap_err();
        assert!(err.to_string().contains("must not escape"));
    }

    #[test]
    fn select_structured_edit_verification_steps_mention_file_and_verify() {
        let inspection = make_inspection(false, vec![]);
        let edit = StructuredEditRequest {
            relative_path: "readme.md".to_string(),
            search: "old".to_string(),
            replacement: "new".to_string(),
            verify_contains: "new text".to_string(),
        };
        let action = select_structured_edit(&inspection, edit, "").unwrap();
        assert!(
            action
                .verification_steps
                .iter()
                .any(|s| s.contains("readme.md"))
        );
        assert!(
            action
                .verification_steps
                .iter()
                .any(|s| s.contains("new text"))
        );
    }
}
