use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};
use crate::sanitization::sanitize_terminal_text;

use super::{CARGO_COMMAND_TIMEOUT_SECS, CLEARED_GIT_ENV_VARS, GIT_COMMAND_TIMEOUT_SECS};

pub(crate) struct CommandOutput {
    pub(crate) stdout: String,
}

/// Spawn a thread that reads `pipe` to EOF and returns the captured bytes.
///
/// Draining stdout and stderr on their own threads is what keeps a child from
/// blocking on a full OS pipe buffer while we poll for its exit (issue #4360).
/// `ChildStdout` and `ChildStderr` both implement `Read`, so one generic helper
/// serves both streams.
fn drain_pipe<R: std::io::Read + Send + 'static>(
    pipe: Option<R>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
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

    // Drain stdout and stderr concurrently on dedicated reader threads so the
    // child can never block on a full OS pipe buffer (~64 KiB on Linux) while
    // our poll loop waits for it to exit. Without this, a child emitting more
    // than one pipe buffer of output deadlocks: it blocks on `write(2)`, never
    // exits, `try_wait()` never reports completion, and we spin until the
    // timeout kills it — surfacing a spurious `CommandTimeout` instead of the
    // real output (issue #4360). Each reader owns its pipe and runs
    // `read_to_end` to EOF, which arrives when the child closes the stream on
    // exit (or when we kill it on timeout), so the threads always join.
    let stdout_reader = drain_pipe(child.stdout.take());
    let stderr_reader = drain_pipe(child.stderr.take());

    let timeout = timeout_for_command(argv);
    let deadline = Instant::now() + timeout;
    // Poll with adaptive backoff: most git invocations finish within a few
    // milliseconds, so start with a 1 ms interval to return promptly, then
    // double up to a 50 ms cap so long-running commands (e.g. cargo builds)
    // keep the same cheap wakeup cadence as before.
    const MAX_POLL_INTERVAL: Duration = Duration::from_millis(50);
    let mut poll_interval = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Killing the child closes its pipes, so the reader threads
                    // hit EOF and finish; join them to avoid leaking threads.
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(SimardError::CommandTimeout {
                        action: argv.join(" "),
                        timeout_secs: timeout.as_secs(),
                    });
                }
                std::thread::sleep(poll_interval);
                poll_interval = (poll_interval * 2).min(MAX_POLL_INTERVAL);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: format!("failed to poll child process: {error}"),
                });
            }
        }
    };

    let stdout_bytes = stdout_reader
        .join()
        .map_err(|_| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: "stdout reader thread panicked".to_string(),
        })?;
    let stderr_bytes = stderr_reader
        .join()
        .map_err(|_| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: "stderr reader thread panicked".to_string(),
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
mod pipe_drain_tests {
    //! TDD regression tests for issue #4360: `run_command_inner`'s poll loop can
    //! stall on more than ~64 KiB of child output because it only calls
    //! [`std::process::Child::wait_with_output`] *after* the child has exited.
    //!
    //! While the loop spins on `try_wait()` + `sleep`, nothing drains the
    //! child's stdout/stderr pipes. Once the child fills the OS pipe buffer
    //! (~64 KiB on Linux) its next `write(2)` blocks forever, the child never
    //! exits, `try_wait()` never reports completion, and the two processes
    //! deadlock until the command timeout kills the child. The caller then sees
    //! a spurious [`SimardError::CommandTimeout`] instead of the real output.
    //!
    //! The contract these tests pin down: a child emitting ~1 MiB (far beyond
    //! any pipe buffer) to stdout and/or stderr must complete promptly with the
    //! full stdout captured. Both streams must be drained *concurrently* while
    //! the child runs.
    //!
    //! Against the current implementation these tests hang until the git
    //! command timeout (60s) and then fail with `CommandTimeout`. Once
    //! `run_command_inner` drains both pipes concurrently they pass quickly.

    use std::time::{Duration, Instant};

    use super::{run_command, run_command_allow_failure};

    /// 1 MiB — an order of magnitude past the ~64 KiB Linux pipe buffer, so it
    /// cannot be buffered and forces the child to block unless we drain.
    const PAYLOAD_BYTES: usize = 1_048_576;

    /// A generous bound: real draining finishes in milliseconds. Anything near
    /// the 60s git timeout is the deadlock symptom, not slowness.
    const FAST_ENOUGH: Duration = Duration::from_secs(30);

    /// >64 KiB of stdout must be captured in full without deadlocking.
    #[test]
    fn run_command_captures_large_stdout_without_deadlock() {
        let dir = tempfile::TempDir::new().unwrap();
        // `yes` streams "y\n" forever; `head -c` caps at 1 MiB then closes the
        // pipe (SIGPIPE ends `yes`), so the pipeline exits 0.
        let script = format!("yes | head -c {PAYLOAD_BYTES}");
        let argv = ["sh", "-c", script.as_str()];

        let start = Instant::now();
        let output = run_command(dir.path(), &argv)
            .expect("large stdout must be drained and captured, not deadlock into a timeout");
        let elapsed = start.elapsed();

        assert!(
            output.stdout.len() >= PAYLOAD_BYTES / 2,
            "expected the full ~1 MiB stdout to be captured; got {} bytes",
            output.stdout.len()
        );
        assert!(
            elapsed < FAST_ENOUGH,
            "draining ~1 MiB stdout should finish quickly; took {elapsed:?} (deadlock symptom)"
        );
    }

    /// A child flooding stderr must not wedge the loop even though the caller
    /// only consumes stdout. Exit is 0, so `run_command` succeeds and the small
    /// stdout marker is returned.
    #[test]
    fn run_command_does_not_deadlock_on_large_stderr() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = format!("yes | head -c {PAYLOAD_BYTES} 1>&2; printf DONE");
        let argv = ["sh", "-c", script.as_str()];

        let start = Instant::now();
        let output = run_command(dir.path(), &argv)
            .expect("large stderr must be drained concurrently, not deadlock into a timeout");
        let elapsed = start.elapsed();

        assert_eq!(
            output.stdout.trim(),
            "DONE",
            "stdout marker must survive a concurrent stderr flood"
        );
        assert!(
            elapsed < FAST_ENOUGH,
            "draining ~1 MiB stderr should finish quickly; took {elapsed:?} (deadlock symptom)"
        );
    }

    /// The classic deadlock shape: both pipes flooded simultaneously. The
    /// non-zero exit routes through `run_command_allow_failure`, whose captured
    /// stdout must still be the full payload.
    #[test]
    fn run_command_allow_failure_drains_both_streams_concurrently() {
        let dir = tempfile::TempDir::new().unwrap();
        let script =
            format!("yes | head -c {PAYLOAD_BYTES}; yes | head -c {PAYLOAD_BYTES} 1>&2; exit 3");
        let argv = ["sh", "-c", script.as_str()];

        let start = Instant::now();
        let output = run_command_allow_failure(dir.path(), &argv)
            .expect("allow_failure must return captured stdout even on non-zero exit");
        let elapsed = start.elapsed();

        assert!(
            output.stdout.len() >= PAYLOAD_BYTES / 2,
            "expected full stdout despite a concurrent stderr flood; got {} bytes",
            output.stdout.len()
        );
        assert!(
            elapsed < FAST_ENOUGH,
            "draining both streams should finish quickly; took {elapsed:?} (deadlock symptom)"
        );
    }
}
