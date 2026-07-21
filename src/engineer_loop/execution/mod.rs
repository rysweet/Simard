use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::thread::JoinHandle;
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
    let mut child = command
        .spawn()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: error.to_string(),
        })?;

    // Drain stdout/stderr concurrently with the wait loop. A well-behaved but
    // chatty child that writes more than the OS pipe buffer (~64 KB on Linux)
    // before exiting would otherwise block on `write()` against a full pipe
    // while the parent only calls `try_wait()`. The child never reaches exit,
    // so the loop spins until the timeout fires — turning verbose output into a
    // spurious `CommandTimeout`. Reader threads keep the pipes drained so the
    // child can run to completion.
    let mut stdout_reader = child.stdout.take().map(spawn_pipe_reader);
    let mut stderr_reader = child.stderr.take().map(spawn_pipe_reader);

    let deadline = Instant::now() + timeout_for_command(argv);
    let status: ExitStatus = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    join_pipe_reader_discard(stdout_reader.take());
                    join_pipe_reader_discard(stderr_reader.take());
                    return Err(SimardError::CommandTimeout {
                        action: argv.join(" "),
                        timeout_secs: timeout_for_command(argv).as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                join_pipe_reader_discard(stdout_reader.take());
                join_pipe_reader_discard(stderr_reader.take());
                return Err(SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: format!("failed to poll child process: {error}"),
                });
            }
        }
    };

    let stdout_bytes = join_pipe_reader(stdout_reader.take(), argv, "stdout")?;
    let stderr_bytes = join_pipe_reader(stderr_reader.take(), argv, "stderr")?;

    if !status.success() && !allow_nonzero_exit {
        let stderr = sanitize_terminal_text(&String::from_utf8_lossy(&stderr_bytes));
        let stdout = sanitize_terminal_text(&String::from_utf8_lossy(&stdout_bytes));
        let reason = if stderr.trim().is_empty() {
            format!(
                "command exited with status {} and stdout='{}'",
                status,
                stdout.trim()
            )
        } else {
            format!(
                "command exited with status {} and stderr='{}'",
                status,
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
        stdout: sanitize_terminal_text(&String::from_utf8_lossy(&stdout_bytes)),
    })
}

/// Handle for a background thread draining one of a child's output pipes.
type PipeReader = JoinHandle<std::io::Result<Vec<u8>>>;

/// Spawn a thread that reads `pipe` to EOF into an owned buffer. Keeping the
/// pipe drained lets the child run to completion even when it writes more than
/// the OS pipe buffer before exiting.
fn spawn_pipe_reader<R: Read + Send + 'static>(mut pipe: R) -> PipeReader {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        pipe.read_to_end(&mut buf)?;
        Ok(buf)
    })
}

/// Join a pipe-reader thread and return the bytes it collected. Missing readers
/// (pipe never captured) yield an empty buffer.
fn join_pipe_reader(
    reader: Option<PipeReader>,
    argv: &[&str],
    stream: &str,
) -> SimardResult<Vec<u8>> {
    match reader {
        None => Ok(Vec::new()),
        Some(handle) => match handle.join() {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(error)) => Err(SimardError::ActionExecutionFailed {
                action: argv.join(" "),
                reason: format!("failed to read child {stream}: {error}"),
            }),
            Err(_) => Err(SimardError::ActionExecutionFailed {
                action: argv.join(" "),
                reason: format!("child {stream} reader thread panicked"),
            }),
        },
    }
}

/// Join a pipe-reader thread on the error/timeout path, discarding its output so
/// the thread is never leaked.
fn join_pipe_reader_discard(reader: Option<PipeReader>) {
    if let Some(handle) = reader {
        let _ = handle.join();
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Regression for the missing pipe-drain in `run_command_inner`: a child that
    /// emits far more than the OS pipe buffer (~64 KB on Linux) before exiting
    /// must run to completion instead of blocking on a full pipe and tripping a
    /// spurious `CommandTimeout`. See `rysweet/amplihack-rs#964` / Simard #4360.
    #[test]
    fn drains_large_child_output_without_timeout() {
        // ~200 KB of printable output, well beyond the ~64 KB pipe buffer.
        let argv = ["sh", "-c", "yes x | head -c 200000"];
        let started = Instant::now();
        let result = run_command(&std::env::temp_dir(), &argv);

        let output = result.expect("chatty command should succeed, not time out");
        assert!(
            output.stdout.len() > 64 * 1024,
            "expected the full drained output, got {} bytes",
            output.stdout.len()
        );
        // With the pipe drained the child exits immediately; guard against a
        // regression that would only complete once the 60s timeout fires.
        assert!(
            started.elapsed() < Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS),
            "command should finish promptly once pipes are drained"
        );
    }
}
