use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};

use super::types::{PTY_LAUNCHER, TerminalSessionCapture, TerminalWaitStatus};
use super::workflow_guard::{WorkflowRestoreGuard, capture_workflow_restore_guards};

struct TranscriptGuard {
    path: PathBuf,
}

impl TranscriptGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TranscriptGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) struct PtyTerminalSession {
    base_type: String,
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    transcript_path: PathBuf,
    transcript_guard: TranscriptGuard,
    _workflow_restore_guards: Vec<WorkflowRestoreGuard>,
    final_status: Option<ExitStatus>,
}

impl PtyTerminalSession {
    pub(crate) fn launch(
        base_type: &str,
        shell: &str,
        working_directory: &Path,
    ) -> SimardResult<Self> {
        let launch_command = format!("{shell} --noprofile --norc -i");
        Self::launch_command(base_type, &launch_command, working_directory)
    }

    pub(crate) fn launch_command(
        base_type: &str,
        launch_command: &str,
        working_directory: &Path,
    ) -> SimardResult<Self> {
        let transcript_path = unique_transcript_path("transcript");
        let transcript_guard = TranscriptGuard::new(transcript_path.clone());
        let _transcript_file = open_exclusive_temp_file(&transcript_path, base_type)?;
        let workflow_restore_guards =
            capture_workflow_restore_guards(base_type, launch_command, working_directory)?;
        let mut child = Command::new(PTY_LAUNCHER)
            .arg("-qefc")
            .arg(launch_command)
            .arg(&transcript_path)
            .current_dir(working_directory)
            .env("TERM", "dumb")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: format!("failed to launch local PTY shell via '{PTY_LAUNCHER}': {error}"),
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: base_type.to_string(),
                reason: "terminal-shell session did not expose stdin".to_string(),
            })?;

        Ok(Self {
            base_type: base_type.to_string(),
            child,
            stdin: Some(BufWriter::new(stdin)),
            transcript_path,
            transcript_guard,
            _workflow_restore_guards: workflow_restore_guards,
            final_status: None,
        })
    }

    pub(crate) fn send_input(&mut self, command: &str) -> SimardResult<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: self.base_type.clone(),
                reason: "terminal-shell session stdin is already closed".to_string(),
            })?;
        writeln!(stdin, "{command}").map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: self.base_type.clone(),
            reason: format!("failed to write terminal command input: {error}"),
        })?;
        stdin
            .flush()
            .map_err(|error| SimardError::AdapterInvocationFailed {
                base_type: self.base_type.clone(),
                reason: format!("failed to flush terminal command input: {error}"),
            })
    }

    pub(crate) fn wait_for_output(
        &mut self,
        expected: &str,
        timeout: Duration,
    ) -> SimardResult<TerminalWaitStatus> {
        let start = Instant::now();
        loop {
            if self
                .read_transcript()
                .map(|transcript| transcript.contains(expected))
                .unwrap_or(false)
            {
                return Ok(TerminalWaitStatus::Satisfied);
            }

            if let Some(status) = self.poll_status()? {
                return Ok(TerminalWaitStatus::ExitedEarly(status));
            }

            if start.elapsed() >= timeout {
                return Ok(TerminalWaitStatus::TimedOut);
            }

            thread::sleep(Duration::from_millis(50));
        }
    }

    pub(crate) fn read_transcript(&self) -> SimardResult<String> {
        let bytes = fs::read(&self.transcript_path).map_err(|error| {
            SimardError::AdapterInvocationFailed {
                base_type: self.base_type.clone(),
                reason: format!(
                    "failed to read terminal transcript '{}': {error}",
                    self.transcript_path.display()
                ),
            }
        })?;
        // Terminal transcripts may contain raw ANSI escapes or other non-UTF-8
        // bytes from the copilot process.  Use lossy conversion so we never
        // fail just because the output includes terminal control sequences.
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub(crate) fn terminate(&mut self) -> SimardResult<()> {
        self.close_stdin()?;

        if self.final_status.is_none() && self.poll_status()?.is_none() {
            self.child
                .kill()
                .map_err(|error| SimardError::AdapterInvocationFailed {
                    base_type: self.base_type.clone(),
                    reason: format!("failed to stop local PTY shell session: {error}"),
                })?;
            self.final_status =
                Some(
                    self.child
                        .wait()
                        .map_err(|error| SimardError::AdapterInvocationFailed {
                            base_type: self.base_type.clone(),
                            reason: format!("failed to reap local PTY shell session: {error}"),
                        })?,
                );
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> SimardResult<TerminalSessionCapture> {
        if self.final_status.is_none() {
            self.close_stdin()?;
        }
        let exit_status = match self.final_status.take() {
            Some(status) => status,
            None => {
                // Two-phase wait:
                //  1. Wait indefinitely for the process to exit naturally.
                //     Agentic sessions (engineers, Copilot adapter) may run
                //     for hours — arbitrary timeouts cause premature termination.
                //  2. If the transcript stops growing for 5 minutes after stdin
                //     is closed, the copilot likely finished but the `script`
                //     wrapper shell is hung at a prompt. Send SIGTERM.
                let mut idle_start: Option<std::time::Instant> = None;
                let mut last_transcript_size: u64 = std::fs::metadata(&self.transcript_path)
                    .ok()
                    .map(|m| m.len())
                    .unwrap_or(0);
                const IDLE_TIMEOUT_SECS: u64 = 300; // 5 min of no transcript growth

                loop {
                    match self.child.try_wait() {
                        Ok(Some(status)) => break status,
                        Ok(None) => {
                            std::thread::sleep(std::time::Duration::from_secs(1));

                            // Check if transcript is still growing.
                            let current_size = std::fs::metadata(&self.transcript_path)
                                .ok()
                                .map(|m| m.len())
                                .unwrap_or(0);
                            if current_size > last_transcript_size {
                                last_transcript_size = current_size;
                                idle_start = None; // reset idle timer
                            } else if idle_start.is_none() {
                                idle_start = Some(std::time::Instant::now());
                            }

                            // If transcript has been idle for IDLE_TIMEOUT_SECS
                            // AND no LLM work process is still running, the
                            // copilot finished but the wrapper shell is hung.
                            // We suppress SIGTERM while work processes are alive
                            // so silent LLM computation is not interrupted.
                            if let Some(start) = idle_start
                                && start.elapsed()
                                    >= std::time::Duration::from_secs(IDLE_TIMEOUT_SECS)
                                && !has_active_work_processes(self.child.id())
                            {
                                eprintln!(
                                    "[simard] terminal session pid={} idle for {}s after \
                                         copilot exit — sending SIGTERM",
                                    self.child.id(),
                                    IDLE_TIMEOUT_SECS,
                                );
                                #[cfg(unix)]
                                {
                                    unsafe {
                                        libc::kill(self.child.id() as i32, libc::SIGTERM);
                                    }
                                }
                                // Give it a moment to clean up.
                                std::thread::sleep(std::time::Duration::from_secs(2));
                                match self.child.try_wait() {
                                    Ok(Some(status)) => break status,
                                    _ => {
                                        #[cfg(unix)]
                                        {
                                            use std::os::unix::process::ExitStatusExt;
                                            break std::process::ExitStatus::from_raw(0);
                                        }
                                        #[cfg(not(unix))]
                                        {
                                            break self.child.wait().unwrap_or_default();
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            return Err(SimardError::AdapterInvocationFailed {
                                base_type: self.base_type.clone(),
                                reason: format!("terminal-shell session failed while waiting: {e}"),
                            });
                        }
                    }
                }
            }
        };
        let transcript = self.read_transcript()?;
        let _ = &self.transcript_guard;
        Ok(TerminalSessionCapture {
            transcript,
            exit_status,
        })
    }

    pub(crate) fn status(&mut self) -> SimardResult<Option<ExitStatus>> {
        self.poll_status()
    }

    fn close_stdin(&mut self) -> SimardResult<()> {
        if let Some(mut stdin) = self.stdin.take() {
            stdin
                .flush()
                .map_err(|error| SimardError::AdapterInvocationFailed {
                    base_type: self.base_type.clone(),
                    reason: format!(
                        "failed to flush terminal command input before completion: {error}"
                    ),
                })?;
            drop(stdin);
        }
        Ok(())
    }

    fn poll_status(&mut self) -> SimardResult<Option<ExitStatus>> {
        if let Some(status) = self.final_status {
            return Ok(Some(status));
        }
        let status =
            self.child
                .try_wait()
                .map_err(|error| SimardError::AdapterInvocationFailed {
                    base_type: self.base_type.clone(),
                    reason: format!("failed to poll terminal-shell session state: {error}"),
                })?;
        if let Some(status) = status {
            self.final_status = Some(status);
            return Ok(Some(status));
        }
        Ok(None)
    }
}

/// Process names that indicate active LLM work is in progress.
#[cfg(unix)]
const WORK_PROCESS_NAMES: &[&str] = &["copilot", "node", "amplihack"];

/// Return `true` if any descendant of `root_pid` (via `/proc`) is named one
/// of the `WORK_PROCESS_NAMES`.  Used to suppress premature SIGTERM when
/// transcript growth has stopped but the LLM is still computing silently.
///
/// On I/O error (process vanished mid-read) the entry is skipped — never
/// panics.  Returns `false` on non-unix targets (preserving original
/// behaviour).
#[cfg(unix)]
fn has_active_work_processes(root_pid: u32) -> bool {
    use std::collections::HashSet;

    // Collect all (pid, ppid) pairs from /proc.
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let Ok(proc_dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        let Ok(pid) = s.parse::<u32>() else { continue };
        let status_path = format!("/proc/{pid}/status");
        let Ok(content) = std::fs::read_to_string(&status_path) else {
            continue;
        };
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("PPid:") {
                if let Ok(ppid) = rest.trim().parse::<u32>() {
                    pairs.push((pid, ppid));
                }
                break;
            }
        }
    }

    // BFS to find all descendants of root_pid.
    let mut descendants: HashSet<u32> = HashSet::new();
    let mut queue: Vec<u32> = vec![root_pid];
    while let Some(parent) = queue.pop() {
        for &(pid, ppid) in &pairs {
            if ppid == parent && descendants.insert(pid) {
                queue.push(pid);
            }
        }
    }

    // Check whether any descendant comm matches a work-process name.
    for pid in descendants {
        let comm_path = format!("/proc/{pid}/comm");
        let Ok(comm) = std::fs::read_to_string(&comm_path) else {
            continue;
        };
        let comm = comm.trim();
        if WORK_PROCESS_NAMES.contains(&comm) {
            return true;
        }
    }

    false
}

#[cfg(not(unix))]
fn has_active_work_processes(_root_pid: u32) -> bool {
    false
}

fn unique_transcript_path(label: &str) -> PathBuf {
    let id = uuid::Uuid::now_v7();
    std::env::temp_dir().join(format!("simard-terminal-shell-{label}-{id}.log",))
}

fn open_exclusive_temp_file(path: &Path, base_type: &str) -> SimardResult<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: base_type.to_string(),
            reason: format!(
                "failed to create terminal transcript '{}': {error}",
                path.display()
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_path_is_unique_per_call() {
        let paths: Vec<PathBuf> = (0..50)
            .map(|_| unique_transcript_path("transcript"))
            .collect();
        let unique: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(
            paths.len(),
            unique.len(),
            "every transcript path must be unique; got {} duplicates out of {}",
            paths.len() - unique.len(),
            paths.len(),
        );
    }

    #[test]
    fn transcript_path_contains_label() {
        let path = unique_transcript_path("my-label");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            name.contains("my-label"),
            "path should contain the label: {name}"
        );
    }

    #[test]
    fn transcript_path_ends_with_log_extension() {
        let path = unique_transcript_path("transcript");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("log"));
    }
}
