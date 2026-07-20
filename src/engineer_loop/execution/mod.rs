use std::io::Read;
use std::path::Path;
use std::process::{Child, Command};
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

    // Drain stdout/stderr on dedicated threads so a child that writes more than
    // the OS pipe buffer (~64 KiB on Linux) never blocks on a full pipe while we
    // poll for exit. Without concurrent draining the child would block on
    // `write`, never exit, and the poll loop below would spin until the command
    // timeout fired — a classic pipe deadlock (issue #4360). The try_wait /
    // deadline poll is retained purely for timeout enforcement.
    let stdout_reader = spawn_pipe_drainer(child.stdout.take());
    let stderr_reader = spawn_pipe_drainer(child.stderr.take());

    let timeout = timeout_for_command(argv);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    // The killed child's pipes close, so the drain threads finish.
                    reap_child(&mut child, stdout_reader, stderr_reader);
                    return Err(SimardError::CommandTimeout {
                        action: argv.join(" "),
                        timeout_secs: timeout.as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                reap_child(&mut child, stdout_reader, stderr_reader);
                return Err(SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: format!("failed to poll child process: {error}"),
                });
            }
        }
    };

    // Join the drain threads to collect the full output. They read to EOF
    // concurrently with the poll loop, so this does not block on a live pipe.
    let stdout_bytes = stdout_reader
        .join()
        .map_err(|_| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: "stdout drain thread panicked".to_string(),
        })?;
    let stderr_bytes = stderr_reader
        .join()
        .map_err(|_| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: "stderr drain thread panicked".to_string(),
        })?;

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

/// Per-stream cap on retained captured bytes (16 MiB). A memory-safety
/// backstop: normal `git`/`cargo` output is far below this. On reaching the cap
/// the reader thread keeps draining (and discarding) the remaining bytes so the
/// child never blocks on a full pipe — only the retained buffer is bounded, not
/// the drain (issue #4360).
const MAX_CAPTURED_BYTES: usize = 16 * 1024 * 1024;

/// Marker appended to a captured stream that was clipped at [`MAX_CAPTURED_BYTES`]
/// so callers and error messages can tell capture was truncated rather than
/// silently losing the tail.
const CAPTURE_TRUNCATED_MARKER: &str = "… [output truncated at 16 MiB]";

/// Kill a still-running child and join its two drain threads, discarding any
/// captured output. Used on the timeout and poll-error paths where the result
/// is being abandoned: killing the child closes its pipes, which lets the
/// drain threads reach EOF and finish so the joins do not block.
fn reap_child(
    child: &mut Child,
    stdout_reader: JoinHandle<Vec<u8>>,
    stderr_reader: JoinHandle<Vec<u8>>,
) {
    let _ = child.kill();
    let _ = child.wait();
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
}

/// Spawn a thread that drains a child pipe to EOF, returning the captured bytes.
///
/// Draining each pipe concurrently with the poll loop is what prevents a
/// full-pipe deadlock for children that emit more than the OS pipe buffer
/// (~64 KiB on Linux) on stdout and/or stderr (issue #4360). A `None` pipe
/// (already taken or never piped) yields an empty buffer.
///
/// Retention is capped at [`MAX_CAPTURED_BYTES`] per stream: once the cap is
/// reached the thread keeps reading and discarding so the child never blocks,
/// and a [`CAPTURE_TRUNCATED_MARKER`] is appended to signal the clip.
fn spawn_pipe_drainer<R>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut retained: Vec<u8> = Vec::new();
        let Some(mut pipe) = pipe else {
            return retained;
        };
        let mut chunk = [0u8; 8192];
        let mut truncated = false;
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if retained.len() < MAX_CAPTURED_BYTES {
                        let take = (MAX_CAPTURED_BYTES - retained.len()).min(n);
                        retained.extend_from_slice(&chunk[..take]);
                        if take < n {
                            truncated = true;
                        }
                    } else {
                        // At the cap: keep draining so the child never blocks,
                        // but discard the bytes.
                        truncated = true;
                    }
                }
                Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if truncated {
            retained.extend_from_slice(CAPTURE_TRUNCATED_MARKER.as_bytes());
        }
        retained
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
