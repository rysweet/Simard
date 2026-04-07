use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};
use crate::sanitization::sanitize_terminal_text;

use super::types::{
    EngineerActionKind, ExecutedEngineerAction, SelectedEngineerAction, validate_repo_relative_path,
};
use super::{
    CARGO_COMMAND_TIMEOUT_SECS, CLEARED_GIT_ENV_VARS, GIT_COMMAND_TIMEOUT_SECS,
    SHELL_COMMAND_ALLOWLIST,
};

pub(crate) struct CommandOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn timeout_for_command(argv: &[&str]) -> Duration {
    if argv.first().is_some_and(|cmd| *cmd == "cargo") {
        Duration::from_secs(CARGO_COMMAND_TIMEOUT_SECS)
    } else {
        Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS)
    }
}

pub(crate) fn run_command(cwd: &Path, argv: &[&str]) -> SimardResult<CommandOutput> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| SimardError::ActionExecutionFailed {
            action: "<empty>".to_string(),
            reason: "argv command list cannot be empty".to_string(),
        })?;
    if argv
        .iter()
        .any(|segment| segment.is_empty() || segment.contains('\n') || segment.contains('\r'))
    {
        return Err(SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: "argv-only command segments must be non-empty single-line values".to_string(),
        });
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in CLEARED_GIT_ENV_VARS {
        command.env_remove(key);
    }
    let mut child = command
        .spawn()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: error.to_string(),
        })?;

    let deadline = Instant::now() + timeout_for_command(argv);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SimardError::CommandTimeout {
                        action: argv.join(" "),
                        timeout_secs: timeout_for_command(argv).as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: format!("failed to poll child process: {error}"),
                });
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: format!("failed to collect child output: {error}"),
        })?;

    if !output.status.success() {
        let stderr = sanitize_terminal_text(&String::from_utf8_lossy(&output.stderr));
        let stdout = sanitize_terminal_text(&String::from_utf8_lossy(&output.stdout));
        let reason = if stderr.trim().is_empty() {
            format!(
                "command exited with status {} and stdout='{}'",
                output.status,
                stdout.trim()
            )
        } else {
            format!(
                "command exited with status {} and stderr='{}'",
                output.status,
                stderr.trim()
            )
        };
        let error = if argv.starts_with(&["git", "rev-parse", "--show-toplevel"]) {
            SimardError::NotARepo {
                path: cwd.to_path_buf(),
                reason,
            }
        } else {
            SimardError::ActionExecutionFailed {
                action: argv.join(" "),
                reason,
            }
        };
        return Err(error);
    }

    Ok(CommandOutput {
        status: output.status,
        stdout: sanitize_terminal_text(&String::from_utf8_lossy(&output.stdout)),
        stderr: sanitize_terminal_text(&String::from_utf8_lossy(&output.stderr)),
    })
}

pub(crate) fn trimmed_stdout(output: &CommandOutput) -> SimardResult<String> {
    let trimmed = output.stdout.trim();
    if trimmed.is_empty() {
        return Err(SimardError::VerificationFailed {
            reason: "expected a non-empty command result while inspecting repo state".to_string(),
        });
    }

    Ok(trimmed.to_string())
}

pub(crate) fn trimmed_stdout_allow_empty(output: &CommandOutput) -> String {
    output.stdout.trim().to_string()
}

pub(crate) fn parse_status_paths(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(|line| {
            if line.len() > 3 {
                line[3..].trim().to_string()
            } else {
                line.to_string()
            }
        })
        .collect()
}

pub(crate) fn execute_engineer_action(
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
            let mut argv_owned: Vec<String> = vec![
                "gh".to_string(),
                "issue".to_string(),
                "create".to_string(),
                "--title".to_string(),
                req.title.clone(),
                "--body".to_string(),
                req.body.clone(),
            ];
            for label in &req.labels {
                argv_owned.push("--label".to_string());
                argv_owned.push(label.clone());
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_for_cargo_command() {
        let timeout = timeout_for_command(&["cargo", "test"]);
        assert_eq!(timeout, Duration::from_secs(CARGO_COMMAND_TIMEOUT_SECS));
    }

    #[test]
    fn timeout_for_git_command() {
        let timeout = timeout_for_command(&["git", "status"]);
        assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
    }

    #[test]
    fn timeout_for_other_command() {
        let timeout = timeout_for_command(&["ls", "-la"]);
        assert_eq!(timeout, Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS));
    }

    #[test]
    fn parse_status_paths_typical_output() {
        let stdout = " M src/main.rs\n M src/lib.rs\n";
        let paths = parse_status_paths(stdout);
        assert_eq!(paths, vec!["src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn parse_status_paths_empty_input() {
        let paths = parse_status_paths("");
        assert!(paths.is_empty());
    }

    #[test]
    fn parse_status_paths_short_line() {
        let paths = parse_status_paths("AB\n");
        assert_eq!(paths, vec!["AB"]);
    }

    #[test]
    fn trimmed_stdout_non_empty() {
        let output = CommandOutput {
            status: std::process::Command::new("true").status().unwrap(),
            stdout: "  hello world  ".to_string(),
            stderr: String::new(),
        };
        assert_eq!(trimmed_stdout(&output).unwrap(), "hello world");
    }

    #[test]
    fn trimmed_stdout_empty_errors() {
        let output = CommandOutput {
            status: std::process::Command::new("true").status().unwrap(),
            stdout: "   ".to_string(),
            stderr: String::new(),
        };
        assert!(trimmed_stdout(&output).is_err());
    }

    #[test]
    fn trimmed_stdout_allow_empty_trims() {
        let output = CommandOutput {
            status: std::process::Command::new("true").status().unwrap(),
            stdout: "  text  ".to_string(),
            stderr: String::new(),
        };
        assert_eq!(trimmed_stdout_allow_empty(&output), "text");
    }

    #[test]
    fn run_command_empty_argv_errors() {
        let result = run_command(Path::new("."), &[]);
        assert!(result.is_err());
    }

    #[test]
    fn run_command_rejects_newlines_in_args() {
        let result = run_command(Path::new("."), &["echo", "hello\nworld"]);
        assert!(result.is_err());
    }

    #[test]
    fn run_command_rejects_empty_segments() {
        let result = run_command(Path::new("."), &["echo", ""]);
        assert!(result.is_err());
    }
}
