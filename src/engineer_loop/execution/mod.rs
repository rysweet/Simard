use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};
use crate::sanitization::sanitize_terminal_text;

use super::{CARGO_COMMAND_TIMEOUT_SECS, CLEARED_GIT_ENV_VARS, GIT_COMMAND_TIMEOUT_SECS};

pub(crate) struct CommandOutput {
    pub(crate) stdout: String,
}

pub(crate) fn timeout_for_command(argv: &[&str]) -> Duration {
    if argv.first().is_some_and(|cmd| *cmd == "cargo") {
        Duration::from_secs(CARGO_COMMAND_TIMEOUT_SECS)
    } else {
        Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS)
    }
}

pub(crate) fn run_command(cwd: &Path, argv: &[&str]) -> SimardResult<CommandOutput> {
    run_command_inner(cwd, argv, /* allow_nonzero_exit = */ false)
}

/// Like [`run_command`] but tolerates non-zero exit codes. Returns `Ok` with
/// whatever stdout was captured even when the child exits non-zero. Still
/// returns `Err` for spawn failures, empty argv, or timeout.
pub(crate) fn run_command_allow_failure(cwd: &Path, argv: &[&str]) -> SimardResult<CommandOutput> {
    run_command_inner(cwd, argv, /* allow_nonzero_exit = */ true)
}

fn run_command_inner(
    cwd: &Path,
    argv: &[&str],
    allow_nonzero_exit: bool,
) -> SimardResult<CommandOutput> {
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
    // Cap cargo parallelism to prevent OOM (issues #2199, #4778). Every other
    // Simard-spawned cargo path applies this limit; run_command_inner was the
    // single outlier, so cargo invocations here could ignore SIMARD_CARGO_JOBS.
    if let Some((key, value)) = cargo_jobs_env(program) {
        command.env(key, value);
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

    if !output.status.success() && !allow_nonzero_exit {
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
        stdout: sanitize_terminal_text(&String::from_utf8_lossy(&output.stdout)),
    })
}

/// Pure decision seam for the cargo OOM guard (issues #2199, #4778).
///
/// Returns the `CARGO_BUILD_JOBS` env var (key + validated value from
/// [`crate::cargo_jobs::cargo_jobs`]) to apply when the spawned program is
/// `cargo`, and `None` for every other program so git invocations — the
/// other consumer of `run_command_inner` — stay untouched. Kept pure so the
/// invariant is unit-testable without spawning a process, matching how the
/// sibling tmux spawn path is covered.
fn cargo_jobs_env(program: &str) -> Option<(&'static str, String)> {
    (program == "cargo").then(|| ("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs()))
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

#[cfg(test)]
mod tests {
    use super::cargo_jobs_env;

    #[test]
    fn cargo_program_gets_build_jobs_cap() {
        let applied = cargo_jobs_env("cargo");
        assert_eq!(
            applied,
            Some(("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())),
            "cargo invocations must carry the CARGO_BUILD_JOBS OOM guard (issues #2199, #4778)"
        );
    }

    #[test]
    fn non_cargo_programs_are_untouched() {
        for program in ["git", "sh", "tmux", "rustc"] {
            assert_eq!(
                cargo_jobs_env(program),
                None,
                "{program} must not receive CARGO_BUILD_JOBS; only cargo is capped"
            );
        }
    }
}
