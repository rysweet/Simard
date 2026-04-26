use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};
use crate::sanitization::sanitize_terminal_text;

use super::{CARGO_COMMAND_TIMEOUT_SECS, CLEARED_GIT_ENV_VARS, GIT_COMMAND_TIMEOUT_SECS};

pub(crate) struct CommandOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Collapse newline (`\n`) and carriage-return (`\r`) characters in `input`
/// to single spaces, then trim whitespace from both ends. Used to keep argv
/// segments single-line so they pass the `run_command` validator. See
/// issue #943.
fn collapse_to_single_line(input: &str) -> String {
    input
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Build a sanitized argv for `gh issue create`. The returned argv is
/// guaranteed to contain both `--title` and `--body` flags. Title and body
/// are collapsed to single-line values (run_command rejects multiline argv
/// segments per issue #943). When the body would otherwise be empty, a
/// placeholder string referencing the originating goal id and agent log
/// path is substituted so `--body` is always present (issue #1011).
pub(crate) fn sanitize_issue_create_args(
    title: &str,
    body: &str,
    labels: &[String],
    goal_id: Option<&str>,
    agent: Option<&str>,
) -> Vec<String> {
    let mut sanitized_title = collapse_to_single_line(title);
    if sanitized_title.is_empty() {
        sanitized_title = "(untitled issue spawned by OODA daemon)".to_string();
    }
    let sanitized_body_raw = collapse_to_single_line(body);
    let sanitized_body = if sanitized_body_raw.is_empty() {
        let goal = goal_id.unwrap_or("unknown");
        let agent_name = agent.unwrap_or("unknown");
        format!(
            "_(spawned by OODA daemon for goal: {goal}; see ~/.simard/agent_logs/{agent_name}.log)_"
        )
    } else {
        sanitized_body_raw
    };
    let mut argv_owned: Vec<String> = vec![
        "gh".to_string(),
        "issue".to_string(),
        "create".to_string(),
        "--title".to_string(),
        sanitized_title,
        "--body".to_string(),
        sanitized_body,
    ];
    for label in labels {
        let label_clean = collapse_to_single_line(label);
        if label_clean.is_empty() {
            continue;
        }
        argv_owned.push("--label".to_string());
        argv_owned.push(label_clean);
    }
    argv_owned
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

mod dispatch;
pub use dispatch::execute_engineer_action;
