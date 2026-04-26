use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::error::{SimardError, SimardResult};
use crate::sanitization::sanitize_terminal_text;

use crate::engineer_loop::types::{
    EngineerActionKind, ExecutedEngineerAction, SelectedEngineerAction, validate_repo_relative_path,
};
use crate::engineer_loop::SHELL_COMMAND_ALLOWLIST;

use super::{
    run_command, sanitize_issue_create_args, timeout_for_command, trimmed_stdout,
    trimmed_stdout_allow_empty,
};

pub fn execute_engineer_action(
    repo_root: &Path,
    selected: SelectedEngineerAction,
) -> SimardResult<ExecutedEngineerAction> {
    match selected.kind.clone() {
        EngineerActionKind::ReadOnlyScan => {
            let argv = selected.argv.iter().map(String::as_str).collect::<Vec<_>>();
            let output = run_command(repo_root, &argv)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::StructuredTextReplace(edit_request) => {
            let target_path = repo_root.join(&edit_request.relative_path);
            let current = fs::read_to_string(&target_path).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "could not read '{}' before applying the bounded edit: {error}",
                        target_path.display()
                    ),
                }
            })?;
            let updated = current.replacen(&edit_request.search, &edit_request.replacement, 1);
            if updated == current {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "replacement target was not found in '{}'",
                        edit_request.relative_path
                    ),
                });
            }
            fs::write(&target_path, updated).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "could not write '{}' after applying the bounded edit: {error}",
                        target_path.display()
                    ),
                }
            })?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: 0,
                stdout: format!(
                    "updated '{}' with one structured replacement",
                    edit_request.relative_path
                ),
                stderr: String::new(),
                changed_files: vec![edit_request.relative_path.clone()],
            })
        }
        EngineerActionKind::CargoTest | EngineerActionKind::CargoCheck => {
            let argv = selected.argv.iter().map(String::as_str).collect::<Vec<_>>();
            let output = run_command(repo_root, &argv)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::CreateFile(ref req) => {
            let relative_path = validate_repo_relative_path(&req.relative_path)?;
            let target_path = repo_root.join(&relative_path);
            if target_path.exists() {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "file '{}' already exists; CreateFile refuses to overwrite",
                        relative_path
                    ),
                });
            }
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|error| SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "could not create parent directories for '{}': {error}",
                        relative_path
                    ),
                })?;
            }
            fs::write(&target_path, &req.content).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!("could not write '{}': {error}", relative_path),
                }
            })?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: 0,
                stdout: format!("created file '{}'", relative_path),
                stderr: String::new(),
                changed_files: vec![relative_path],
            })
        }
        EngineerActionKind::AppendToFile(ref req) => {
            let relative_path = validate_repo_relative_path(&req.relative_path)?;
            let target_path = repo_root.join(&relative_path);
            if !target_path.exists() {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "file '{}' does not exist; AppendToFile requires an existing file",
                        relative_path
                    ),
                });
            }
            let mut file = OpenOptions::new()
                .append(true)
                .open(&target_path)
                .map_err(|error| SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!("could not open '{}' for appending: {error}", relative_path),
                })?;
            file.write_all(req.content.as_bytes()).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!("could not append to '{}': {error}", relative_path),
                }
            })?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: 0,
                stdout: format!("appended content to '{}'", relative_path),
                stderr: String::new(),
                changed_files: vec![relative_path],
            })
        }
        EngineerActionKind::RunShellCommand(ref req) => {
            let first = req
                .argv
                .first()
                .ok_or_else(|| SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: "shell command argv is empty".to_string(),
                })?;
            if !SHELL_COMMAND_ALLOWLIST.contains(&first.as_str()) {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "command '{}' is not in the shell command allowlist {:?}",
                        first, SHELL_COMMAND_ALLOWLIST
                    ),
                });
            }
            let argv_refs: Vec<&str> = req.argv.iter().map(String::as_str).collect();
            let output = run_command(repo_root, &argv_refs)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::GitCommit(ref req) => {
            run_command(repo_root, &["git", "add", "-A"])?;
            let output = run_command(repo_root, &["git", "commit", "-m", &req.message])?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::OpenIssue(ref req) => {
            let argv_owned =
                sanitize_issue_create_args(&req.title, &req.body, &req.labels, None, None);
            let argv_refs: Vec<&str> = argv_owned.iter().map(String::as_str).collect();
            let output = run_command(repo_root, &argv_refs)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
    }
}
