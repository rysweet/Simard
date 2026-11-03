use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};
use crate::sanitization::sanitize_terminal_text;

use super::{CARGO_COMMAND_TIMEOUT_SECS, CLEARED_GIT_ENV_VARS, GIT_COMMAND_TIMEOUT_SECS};

#[cfg(unix)]
fn spawn_pipe_reader<R: Read + std::os::fd::AsRawFd + Send + 'static>(
    mut pipe: R,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        // SAFETY: the descriptor is owned by this thread and remains open for this call.
        let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut result = Vec::new();
        let mut buf = [0; 8192];
        while !stop.load(Ordering::Relaxed) {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => result.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    })
}

#[cfg(not(unix))]
fn spawn_pipe_reader<R: Read + Send + 'static>(
    mut pipe: R,
    _stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    })
}

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

    // Drain both pipes while polling: either pipe can fill before the child exits.
    let stdout = child.stdout.take().expect("stdout was configured as piped");
    let stderr = child.stderr.take().expect("stderr was configured as piped");
    let stop_readers = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_pipe_reader(stdout, Arc::clone(&stop_readers));
    let stderr_reader = spawn_pipe_reader(stderr, Arc::clone(&stop_readers));

    let deadline = Instant::now() + timeout_for_command(argv);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    stop_readers.store(true, Ordering::Relaxed);
                    #[cfg(unix)]
                    {
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                    }
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
                stop_readers.store(true, Ordering::Relaxed);
                #[cfg(unix)]
                {
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                }
                return Err(SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: format!("failed to poll child process: {error}"),
                });
            }
        }
    };

    let collect = |reader: std::thread::JoinHandle<std::io::Result<Vec<u8>>>| {
        reader
            .join()
            .map_err(|_| "reader thread panicked".to_string())?
            .map_err(|error| error.to_string())
    };
    let stdout_result = collect(stdout_reader);
    let stderr_result = collect(stderr_reader);
    let stdout = stdout_result.map_err(|error| SimardError::ActionExecutionFailed {
        action: argv.join(" "),
        reason: format!("failed to collect child stdout: {error}"),
    })?;
    let stderr = stderr_result.map_err(|error| SimardError::ActionExecutionFailed {
        action: argv.join(" "),
        reason: format!("failed to collect child stderr: {error}"),
    })?;

    if !status.success() && !allow_nonzero_exit {
        let stderr = sanitize_terminal_text(&String::from_utf8_lossy(&stderr));
        let stdout = sanitize_terminal_text(&String::from_utf8_lossy(&stdout));
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
        stdout: sanitize_terminal_text(&String::from_utf8_lossy(&stdout)),
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

    #[cfg(unix)]
    #[test]
    fn drains_large_stderr_then_stdout() {
        let output = super::run_command(
            std::path::Path::new("."),
            &[
                "sh",
                "-c",
                "yes e | head -c 1048576 >&2; yes o | head -c 1048576",
            ],
        )
        .expect("large child output should not block the child");
        assert_eq!(
            output.stdout,
            crate::sanitization::sanitize_terminal_text(&"o\n".repeat(524_288))
        );
    }

    #[cfg(unix)]
    #[test]
    fn large_output_preserves_nonzero_status() {
        let args = [
            "sh",
            "-c",
            "yes e | head -c 131072 >&2; printf result; exit 7",
        ];
        let error = super::run_command(std::path::Path::new("."), &args)
            .err()
            .expect("nonzero status must fail");
        assert!(error.to_string().contains("exit status: 7"));
        let output = super::run_command_allow_failure(std::path::Path::new("."), &args)
            .expect("allow failure must preserve stdout");
        assert_eq!(output.stdout, "result");
    }

    #[cfg(unix)]
    #[test]
    fn pipe_reader_stops_while_writer_remains_open() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exec sleep 10"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn writer");
        let stop = Arc::new(AtomicBool::new(false));
        let reader = super::spawn_pipe_reader(child.stdout.take().unwrap(), Arc::clone(&stop));
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap().expect("reader stopped");
        child.kill().expect("stop test child");
        child.wait().expect("reap test child");
    }

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
