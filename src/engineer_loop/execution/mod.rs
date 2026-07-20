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
    let mut child = command
        .spawn()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: error.to_string(),
        })?;

    // Drain stdout/stderr on dedicated reader threads *while* we poll for exit.
    // A child that writes more than the OS pipe buffer (~64 KB on Linux) before
    // exiting would otherwise block on `write()` against a full pipe that nobody
    // is reading, never reach exit, and spin this loop until the timeout fires —
    // turning a chatty-but-well-behaved command into a spurious `CommandTimeout`
    // (issue #4360). Reading concurrently keeps the pipes drained so the child
    // can always make progress and exit.
    let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
    let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

    let deadline = Instant::now() + timeout_for_command(argv);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Killing the child closes the pipe write ends, so the
                    // reader threads observe EOF and terminate; join them so we
                    // never leak threads on the timeout path.
                    join_pipe_reader(stdout_reader);
                    join_pipe_reader(stderr_reader);
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
    };

    // The child has exited, so its pipe write ends are closed and the reader
    // threads have already hit EOF (or will imminently); joining collects the
    // fully drained buffers.
    let collect = |reader: Option<std::thread::JoinHandle<Vec<u8>>>| -> SimardResult<Vec<u8>> {
        match reader {
            Some(handle) => handle
                .join()
                .map_err(|_| SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: "failed to collect child output: reader thread panicked".to_string(),
                }),
            None => Ok(Vec::new()),
        }
    };
    let stdout_bytes = collect(stdout_reader)?;
    let stderr_bytes = collect(stderr_reader)?;

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

/// Spawns a thread that drains `pipe` to end into a byte buffer, returning the
/// join handle. Read errors yield whatever was buffered so far so a partial
/// read never blocks the caller.
fn spawn_pipe_reader<R>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

/// Joins a reader thread on the timeout path, discarding its buffer. Best
/// effort: a panicked reader thread is ignored since we are already erroring.
fn join_pipe_reader(reader: Option<std::thread::JoinHandle<Vec<u8>>>) {
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

    /// Regression for issue #4360: a child that writes far more than the OS
    /// pipe buffer (~64 KB on Linux) before exiting must not stall the wait
    /// loop into a spurious `CommandTimeout`. Before the concurrent pipe drain,
    /// the child would block on a full stdout pipe and only unblock at kill.
    #[test]
    fn large_output_does_not_spurious_timeout() {
        // `yes x` emits "x\n" forever; `head -n 200000` takes 400_000 bytes
        // (well past the pipe buffer) then closes the pipe so `yes` exits.
        let cwd = std::env::temp_dir();
        let output = run_command(&cwd, &["sh", "-c", "yes x | head -n 200000"])
            .expect("chatty command that exits cleanly must not time out");
        assert!(
            output.stdout.len() > 64 * 1024,
            "expected the full drained stdout (>64 KiB), got {} bytes",
            output.stdout.len()
        );
    }

    /// A large-output command that also exits non-zero still surfaces the exit
    /// as an error (not a timeout) once the pipes are drained.
    #[test]
    fn large_output_nonzero_exit_is_reported_not_timed_out() {
        let cwd = std::env::temp_dir();
        match run_command(&cwd, &["sh", "-c", "yes x | head -n 200000; exit 3"]) {
            Ok(_) => panic!("non-zero exit must be reported as an execution failure, not Ok"),
            Err(err) => assert!(
                matches!(err, SimardError::ActionExecutionFailed { .. }),
                "expected ActionExecutionFailed, got {err:?}"
            ),
        }
    }

    /// `run_command_allow_failure` tolerates the non-zero exit and still returns
    /// the fully drained large stdout.
    #[test]
    fn large_output_allow_failure_returns_full_stdout() {
        let cwd = std::env::temp_dir();
        let output =
            run_command_allow_failure(&cwd, &["sh", "-c", "yes x | head -n 200000; exit 3"])
                .expect("allow_failure must return Ok even on non-zero exit");
        assert!(
            output.stdout.len() > 64 * 1024,
            "expected full drained stdout (>64 KiB), got {} bytes",
            output.stdout.len()
        );
    }
}
