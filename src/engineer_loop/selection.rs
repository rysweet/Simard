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
