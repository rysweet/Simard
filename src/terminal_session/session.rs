use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};

use super::types::{DEFAULT_PATH, PTY_LAUNCHER, TerminalSessionCapture, TerminalWaitStatus};
use super::workflow_guard::{WorkflowRestoreGuard, capture_workflow_restore_guards};

struct TranscriptGuard {
    path: PathBuf,
    preserve: bool,
}

impl TranscriptGuard {
    fn new(path: PathBuf) -> Self {
        // Honour SIMARD_KEEP_TRANSCRIPTS=1 from process start so operators can
        // collect every transcript even on healthy sessions.
        let preserve = std::env::var("SIMARD_KEEP_TRANSCRIPTS").as_deref() == Ok("1");
        Self { path, preserve }
    }

    /// Mark this transcript file as "do not delete on Drop". Called when we
    /// detect a hang so the post-mortem evidence survives.
    fn preserve(&mut self) {
        self.preserve = true;
    }
}

impl Drop for TranscriptGuard {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) struct PtyTerminalSession {
    base_type: String,
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    transcript_path: PathBuf,
    _transcript_guard: TranscriptGuard,
    _workflow_restore_guards: Vec<WorkflowRestoreGuard>,
    final_status: Option<ExitStatus>,
}

impl PtyTerminalSession {
    pub(crate) fn launch(
        base_type: &str,
        shell: &str,
        working_directory: &Path,
    ) -> SimardResult<Self> {
        let launch_command = format!("{shell} {}", interactive_shell_flags(shell));
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
        let mut command = Command::new(PTY_LAUNCHER);
        // GNU `script` (Linux util-linux) uses `-qefc <command> <file>`.
        // BSD `script` (macOS) uses positional `script [-qFe] <file> <command...>`
        // and has no `-c` flag — the command and its args are passed positionally.
        #[cfg(target_os = "macos")]
        {
            command
                .arg("-qFe")
                .arg(&transcript_path)
                .arg("/bin/sh")
                .arg("-c")
                .arg(launch_command);
        }
        #[cfg(not(target_os = "macos"))]
        {
            command
                .arg("-qefc")
                .arg(launch_command)
                .arg(&transcript_path);
        }
        command.current_dir(working_directory).env("TERM", "dumb");
        // Ensure the child PTY has a usable PATH so ordinary commands resolve to
        // real binaries instead of failing with exit code 127. Only override when
        // the inherited PATH is missing or empty so operator-provided PATHs (and
        // any PATH set by the launching service) are preserved untouched.
        if let Some(path) = child_path_override(std::env::var_os("PATH").as_deref()) {
            command.env("PATH", path);
        }
        let mut child = command
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
            _transcript_guard: transcript_guard,
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
                //  2. If the transcript stops growing for `IDLE_TIMEOUT_SECS`
                //     after stdin is closed, the copilot likely finished but
                //     the `script` wrapper shell is hung at a prompt. Send
                //     SIGTERM, dumping the transcript first so post-mortem is
                //     possible.
                //
                // Diagnostic surface:
                //  - Every `HEARTBEAT_INTERVAL_SECS` we print the elapsed wait
                //    time, the current transcript byte count, and the last
                //    `HEARTBEAT_TAIL_BYTES` of the transcript. This lets an
                //    operator (or a test harness watching stderr) see *what*
                //    the subprocess is actually doing when it appears stuck —
                //    no need to guess at PIDs or hunt for transcript files.
                //  - On idle-timeout we dump the full transcript before
                //    SIGTERM and disarm the auto-deletion guard so the file
                //    survives for post-mortem.
                //  - Both intervals are tunable via env vars so that tests
                //    and CI can shorten them without recompiling.
                let mut idle_start: Option<std::time::Instant> = None;
                let mut last_transcript_size: u64 = std::fs::metadata(&self.transcript_path)
                    .ok()
                    .map(|m| m.len())
                    .unwrap_or(0);
                let wait_start = std::time::Instant::now();
                let mut last_heartbeat = wait_start;

                let idle_timeout = std::time::Duration::from_secs(env_secs(
                    "SIMARD_TERMINAL_IDLE_TIMEOUT_SECS",
                    300,
                ));
                let heartbeat_interval =
                    std::time::Duration::from_secs(env_secs("SIMARD_TERMINAL_HEARTBEAT_SECS", 30));
                let heartbeat_tail_bytes: usize =
                    env_secs("SIMARD_TERMINAL_HEARTBEAT_TAIL_BYTES", 512) as usize;

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

                            // Periodic heartbeat: who are we waiting on, what
                            // is the transcript saying right now?
                            if last_heartbeat.elapsed() >= heartbeat_interval {
                                last_heartbeat = std::time::Instant::now();
                                let elapsed_total = wait_start.elapsed().as_secs();
                                let idle_for =
                                    idle_start.map(|i| i.elapsed().as_secs()).unwrap_or(0);
                                let tail =
                                    transcript_tail(&self.transcript_path, heartbeat_tail_bytes);
                                eprintln!(
                                    "[simard] terminal heartbeat base_type={} pid={} \
                                     waiting={}s idle={}s transcript_bytes={} \
                                     transcript_path={} tail={:?}",
                                    self.base_type,
                                    self.child.id(),
                                    elapsed_total,
                                    idle_for,
                                    current_size,
                                    self.transcript_path.display(),
                                    tail,
                                );
                            }

                            // If transcript has been idle for the timeout AND
                            // no LLM work process is still running, the copilot
                            // finished but the wrapper shell is hung. Suppress
                            // SIGTERM while work processes are alive so silent
                            // LLM computation is not interrupted.
                            if let Some(start) = idle_start
                                && start.elapsed() >= idle_timeout
                                && !has_active_work_processes(self.child.id())
                            {
                                let dump = std::fs::read_to_string(&self.transcript_path)
                                    .unwrap_or_default();
                                eprintln!(
                                    "[simard] terminal HUNG base_type={} pid={} \
                                     idle={}s — preserving transcript at {} and sending SIGTERM. \
                                     Full transcript follows:\n--- BEGIN HUNG TRANSCRIPT ---\n\
                                     {}\n--- END HUNG TRANSCRIPT ---",
                                    self.base_type,
                                    self.child.id(),
                                    start.elapsed().as_secs(),
                                    self.transcript_path.display(),
                                    dump,
                                );
                                // Disarm the guard so the file survives for
                                // post-mortem. The path is already echoed
                                // above so the operator can find it.
                                self._transcript_guard.preserve();
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
                                            // SIGTERM did not reap the wedged
                                            // shell within the grace window.
                                            // Escalate to SIGKILL and block until
                                            // it is reaped so we never leak a
                                            // zombie, then report a NON-SUCCESS
                                            // "timed out" status. A session we had
                                            // to force-kill for hanging must never
                                            // be reported as success — doing so
                                            // silently masks a failed or hung turn
                                            // as a passing one.
                                            unsafe {
                                                libc::kill(self.child.id() as i32, libc::SIGKILL);
                                            }
                                            let _ = self.child.wait();
                                            break hung_session_timeout_status();
                                        }
                                        #[cfg(not(unix))]
                                        {
                                            let _ = self.child.kill();
                                            let _ = self.child.wait();
                                            break hung_session_timeout_status();
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

/// Decide whether the child PTY needs an explicit `PATH` override.
///
/// Returns the fallback [`DEFAULT_PATH`] when the inherited value is missing or
/// empty so ordinary commands resolve instead of failing with exit code 127;
/// returns `None` when the inherited `PATH` is usable and must be preserved.
fn child_path_override(inherited: Option<&OsStr>) -> Option<&'static str> {
    match inherited {
        Some(value) if !value.is_empty() => None,
        _ => Some(DEFAULT_PATH),
    }
}

/// Interactive launch flags appropriate for the given shell.
///
/// bash honours `--noprofile --norc` to skip startup files for a clean,
/// deterministic PTY session. POSIX `sh`/dash reject those long options
/// (`sh: Illegal option --`), so any non-bash interpreter — including the
/// `/bin/sh`/`$SHELL` fallbacks resolved by `default_shell` — gets a bare
/// interactive invocation instead. Without this, falling back to a non-bash
/// shell would itself fail with the very exit code this fix is meant to
/// eliminate.
fn interactive_shell_flags(shell: &str) -> &'static str {
    let name = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if is_bash_shell(name) {
        "--noprofile --norc -i"
    } else {
        "-i"
    }
}

/// Whether `name` is the bash interpreter, tolerating versioned basenames such
/// as `bash5` or `bash-5.2` while excluding unrelated names that merely start
/// with "bash" (e.g. `bashful`). The `--noprofile --norc` long options are
/// bash-only; applying them to a non-bash shell makes the interactive launch
/// abort with the very exit code this fix is meant to eliminate.
fn is_bash_shell(name: &str) -> bool {
    match name.strip_prefix("bash") {
        Some("") => true,
        Some(rest) => rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '-'),
        None => false,
    }
}

/// The exit status reported when a terminal session had to be force-killed
/// after hanging past the idle timeout.
///
/// Code `124` mirrors `timeout(1)`'s "command timed out" convention and is
/// deliberately **non-success**: a session we had to SIGTERM/SIGKILL because it
/// wedged must never be reported as a passing turn. Returning a synthesized
/// `exit 0` here previously masked a failed or hung turn (e.g. a one-shot
/// terminal command that actually exited non-zero but whose interactive shell
/// stalled on EOF under host load) as success.
#[cfg(unix)]
fn hung_session_timeout_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Encode as a normal `exit(124)` wait status: low byte 0 (not signalled),
    // exit code in the high byte, so `.code()` == Some(124) and `.success()`
    // == false.
    ExitStatus::from_raw(124 << 8)
}

/// Non-unix counterpart: `ExitStatusExt::from_raw` takes the raw exit code
/// directly, so a non-zero value is likewise non-success.
#[cfg(not(unix))]
fn hung_session_timeout_status() -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(124)
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
    use std::collections::{HashMap, HashSet, VecDeque};

    // Single /proc scan: build a parent→children map.  O(n) vs the previous
    // O(n²) approach that re-scanned the flat pairs Vec on every BFS step.
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
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
                    children.entry(ppid).or_default().push(pid);
                }
                break;
            }
        }
    }

    // BFS over descendants; check comm at each node and short-circuit on
    // first match — no need to collect the full set before checking.
    let mut visited: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<u32> = VecDeque::new();
    if let Some(kids) = children.get(&root_pid) {
        for &kid in kids {
            if visited.insert(kid) {
                queue.push_back(kid);
            }
        }
    }
    while let Some(pid) = queue.pop_front() {
        let comm_path = format!("/proc/{pid}/comm");
        if let Ok(comm) = std::fs::read_to_string(&comm_path)
            && WORK_PROCESS_NAMES.contains(&comm.trim())
        {
            return true;
        }
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                if visited.insert(kid) {
                    queue.push_back(kid);
                }
            }
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

/// Read at most `max_bytes` from the tail of `path` for diagnostic logging.
///
/// Returns a `String` rather than bytes so callers can format it directly.
/// Non-UTF-8 bytes are replaced with `U+FFFD` via lossy conversion. Returns
/// an empty string if the file cannot be read.
fn transcript_tail(path: &Path, max_bytes: usize) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let start = bytes.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// Read a non-negative integer from an env var, falling back to `default`
/// when the var is unset or unparseable. Used for the heartbeat / idle
/// timeout knobs so tests and CI can shorten them without recompiling.
fn env_secs(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── child_path_override ───────────────────────────────────────────────────

    #[test]
    fn child_path_override_preserves_usable_path() {
        assert_eq!(
            child_path_override(Some(OsStr::new("/usr/bin:/bin"))),
            None,
            "a non-empty inherited PATH must be preserved untouched"
        );
    }

    #[test]
    fn child_path_override_falls_back_when_missing() {
        assert_eq!(child_path_override(None), Some(DEFAULT_PATH));
    }

    #[test]
    fn child_path_override_falls_back_when_empty() {
        assert_eq!(
            child_path_override(Some(OsStr::new(""))),
            Some(DEFAULT_PATH)
        );
    }

    // ── interactive_shell_flags ───────────────────────────────────────────────

    #[test]
    fn interactive_shell_flags_uses_bash_options_for_bash() {
        assert_eq!(
            interactive_shell_flags("/usr/bin/bash"),
            "--noprofile --norc -i"
        );
        assert_eq!(
            interactive_shell_flags("/bin/bash"),
            "--noprofile --norc -i"
        );
    }

    #[test]
    fn interactive_shell_flags_treats_versioned_bash_as_bash() {
        // Versioned bash basenames still honour the bash-only startup-file
        // options, so a `$SHELL`/distro fallback to e.g. `bash5` stays
        // deterministic rather than silently running profile/rc files.
        assert_eq!(
            interactive_shell_flags("/usr/local/bin/bash5"),
            "--noprofile --norc -i"
        );
        assert_eq!(
            interactive_shell_flags("/opt/homebrew/bin/bash-5.2"),
            "--noprofile --norc -i"
        );
        assert!(is_bash_shell("bash"));
        assert!(is_bash_shell("bash5"));
        assert!(is_bash_shell("bash-5.2"));
    }

    #[test]
    fn interactive_shell_flags_uses_posix_options_for_non_bash() {
        // `--noprofile`/`--norc` are bash-specific; POSIX sh and other shells
        // must get a bare interactive invocation or they abort on launch.
        assert_eq!(interactive_shell_flags("/bin/sh"), "-i");
        assert_eq!(interactive_shell_flags("/usr/bin/dash"), "-i");
        assert_eq!(interactive_shell_flags("/usr/bin/zsh"), "-i");
        // Names that merely start with "bash" but are not bash must not get the
        // bash-only options.
        assert_eq!(interactive_shell_flags("/usr/games/bashful"), "-i");
        assert!(!is_bash_shell("bashful"));
        assert!(!is_bash_shell("sh"));
    }

    // ── has_active_work_processes ─────────────────────────────────────────────

    /// A force-killed hung session must NEVER be reported as success. The status
    /// synthesized on the SIGTERM/SIGKILL idle-timeout path must be non-success
    /// and carry the `timeout(1)` code 124 so `describe_terminal_failure`
    /// surfaces an actionable "timed out" diagnostic instead of silently passing.
    #[test]
    fn hung_session_timeout_status_is_never_success() {
        let status = hung_session_timeout_status();
        assert!(
            !status.success(),
            "a session we had to force-kill for hanging must not be success"
        );
        #[cfg(unix)]
        assert_eq!(
            status.code(),
            Some(124),
            "hung-session status must carry the timeout(1) exit code 124"
        );
    }

    /// A PID that virtually cannot exist (u32::MAX - 1) must return false
    /// without panicking — the function must be race-safe for vanished PIDs.
    #[test]
    #[cfg(unix)]
    fn has_active_work_processes_nonexistent_pid_returns_false() {
        assert!(!has_active_work_processes(u32::MAX - 1));
    }

    /// PID 0 is the swapper/idle process and has no user-space descendants
    /// named copilot/node/amplihack. Must not panic.
    #[test]
    #[cfg(unix)]
    fn has_active_work_processes_pid_zero_does_not_panic() {
        let _ = has_active_work_processes(0);
    }

    /// The test process itself should not have copilot/node/amplihack
    /// descendants when run via `cargo test` — verifies no false positive.
    #[test]
    #[cfg(unix)]
    fn has_active_work_processes_cargo_test_process_no_false_positive() {
        // This test runs inside `cargo test`. In a normal CI environment there
        // are no copilot/node/amplihack children hanging off the test runner.
        // We can't assert `false` because a developer's machine might actually
        // have those binaries running, but we CAN assert the call doesn't
        // panic or hang.
        let _ = has_active_work_processes(std::process::id());
    }

    /// WORK_PROCESS_NAMES must contain exactly the three names from the spec.
    #[test]
    #[cfg(unix)]
    fn work_process_names_contains_exactly_copilot_node_amplihack() {
        assert!(
            WORK_PROCESS_NAMES.contains(&"copilot"),
            "WORK_PROCESS_NAMES must include 'copilot'"
        );
        assert!(
            WORK_PROCESS_NAMES.contains(&"node"),
            "WORK_PROCESS_NAMES must include 'node'"
        );
        assert!(
            WORK_PROCESS_NAMES.contains(&"amplihack"),
            "WORK_PROCESS_NAMES must include 'amplihack'"
        );
        assert_eq!(
            WORK_PROCESS_NAMES.len(),
            3,
            "WORK_PROCESS_NAMES must have exactly 3 entries"
        );
    }

    /// Partial names like "node_modules" must NOT match — the check uses exact
    /// equality on the trimmed comm string, not a substring test.
    #[test]
    #[cfg(unix)]
    fn work_process_names_does_not_include_partial_matches() {
        assert!(
            !WORK_PROCESS_NAMES.contains(&"node_modules"),
            "'node_modules' is not a work process name"
        );
        assert!(
            !WORK_PROCESS_NAMES.contains(&"amplihack-server"),
            "'amplihack-server' is not a work process name"
        );
        assert!(
            !WORK_PROCESS_NAMES.contains(&"copilot-daemon"),
            "'copilot-daemon' is not a work process name"
        );
    }

    /// On non-unix targets has_active_work_processes must always return false
    /// (preserves original kill behaviour on those platforms).
    #[test]
    #[cfg(not(unix))]
    fn has_active_work_processes_always_false_on_non_unix() {
        assert!(!has_active_work_processes(1));
        assert!(!has_active_work_processes(std::process::id()));
        assert!(!has_active_work_processes(0));
    }

    // ── unique_transcript_path ────────────────────────────────────────────────

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

    // ── transcript diagnostics: tail / preserve / env_secs ──────────────

    #[test]
    fn transcript_tail_returns_empty_for_missing_file() {
        let path = std::env::temp_dir().join(format!(
            "simard-transcript-tail-missing-{}.log",
            uuid::Uuid::now_v7()
        ));
        assert_eq!(transcript_tail(&path, 64), "");
    }

    #[test]
    fn transcript_tail_returns_at_most_max_bytes_from_end() {
        let path = std::env::temp_dir().join(format!(
            "simard-transcript-tail-content-{}.log",
            uuid::Uuid::now_v7()
        ));
        let body = b"abcdefghijklmnopqrstuvwxyz0123456789";
        std::fs::write(&path, body).expect("write");

        let tail = transcript_tail(&path, 10);
        assert_eq!(tail, "0123456789");

        let full = transcript_tail(&path, body.len() + 100);
        assert_eq!(full.as_bytes(), body);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_guard_preserve_skips_drop_deletion() {
        let path = std::env::temp_dir().join(format!(
            "simard-transcript-guard-preserve-{}.log",
            uuid::Uuid::now_v7()
        ));
        std::fs::write(&path, b"persistent evidence").expect("write");
        {
            let mut guard = TranscriptGuard::new(path.clone());
            guard.preserve();
        } // drop here — must NOT delete because preserve() was called
        assert!(path.exists(), "preserved transcript must survive Drop");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_guard_default_drop_deletes_file() {
        let path = std::env::temp_dir().join(format!(
            "simard-transcript-guard-delete-{}.log",
            uuid::Uuid::now_v7()
        ));
        // Bypass the env-var check so this test stays deterministic
        // regardless of caller environment.
        std::fs::write(&path, b"ephemeral evidence").expect("write");
        {
            let guard = TranscriptGuard {
                path: path.clone(),
                preserve: false,
            };
            drop(guard);
        }
        assert!(!path.exists(), "default Drop must delete the transcript");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn env_secs_returns_default_when_unset() {
        // Use a uniquely named env var so we know it's unset.
        let var = format!("SIMARD_ENV_SECS_TEST_{}", uuid::Uuid::now_v7().simple());
        // SAFETY: tests are not parallel here; we only touch a unique var name.
        unsafe { std::env::remove_var(&var) };
        assert_eq!(env_secs(&var, 42), 42);
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn env_secs_returns_parsed_value_when_set() {
        let var = format!("SIMARD_ENV_SECS_TEST_{}", uuid::Uuid::now_v7().simple());
        unsafe { std::env::set_var(&var, "7") };
        assert_eq!(env_secs(&var, 42), 7);
        unsafe { std::env::remove_var(&var) };
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn env_secs_returns_default_when_unparseable() {
        let var = format!("SIMARD_ENV_SECS_TEST_{}", uuid::Uuid::now_v7().simple());
        unsafe { std::env::set_var(&var, "not-a-number") };
        assert_eq!(env_secs(&var, 42), 42);
        unsafe { std::env::remove_var(&var) };
    }

    // ── PtyTerminalSession end-to-end (no controlling terminal) ───────────────

    /// `which`-style probe for the PTY launcher (`script`). Used to skip the
    /// end-to-end test gracefully when the binary is absent, matching the
    /// project convention for tests that depend on an external binary
    /// (see `engineer_worktree::precommit` and `meeting_backend::agent_proxy`).
    #[cfg(unix)]
    fn pty_launcher_available() -> bool {
        std::process::Command::new("which")
            .arg(PTY_LAUNCHER)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// End-to-end smoke test of the full session lifecycle —
    /// `launch` → `send_input` → `wait_for_output` → `finish` — proving the
    /// terminal-shell session works even when the test process itself has **no
    /// controlling terminal** (`tty` reports "not a tty"), exactly as in CI /
    /// headless runners. `script` allocates its *own* PTY for the inner shell,
    /// so an interactive (`-i`) bash still launches, executes the command it
    /// receives, and reports output back through the transcript without any
    /// parent TTY. This is the only test that exercises the real spawn /
    /// transcript-capture path the production recipe runner depends on.
    ///
    /// Because `script` runs bash on a PTY with terminal `ECHO` enabled, the
    /// command line we type is copied verbatim into the transcript regardless
    /// of whether bash ever runs it. To prove the shell genuinely *executed*
    /// the command (not merely received and echoed it), the marker embeds a
    /// shell arithmetic expansion: the typed line contains the literal
    /// `$((20 + 22))`, whereas only successful execution yields the evaluated
    /// `42`. Asserting on the evaluated form therefore cannot be satisfied by
    /// the input echo alone — it requires real command execution plus output
    /// capture.
    ///
    /// Skips (rather than fails) when `script` is not on PATH.
    #[test]
    #[cfg(unix)]
    fn pty_session_runs_end_to_end_without_controlling_terminal() {
        use crate::terminal_session::types::DEFAULT_SHELL;

        if !pty_launcher_available() {
            eprintln!("skipping: PTY launcher '{PTY_LAUNCHER}' not on PATH");
            return;
        }

        let workdir = tempfile::tempdir().expect("create temp working directory");
        // Unique per-session token keeps the expected string specific to this
        // run; the `$((20 + 22))` expansion guarantees the asserted form
        // (`<token>-42-done`) can only come from executed output, never the
        // verbatim PTY echo of the typed command (which carries `$((20 + 22))`).
        let token = format!("notty-marker-{}", uuid::Uuid::now_v7().simple());
        let typed_command = format!("echo {token}-$((20 + 22))-done");
        let executed_output = format!("{token}-42-done");

        let mut session = PtyTerminalSession::launch("test-bt", DEFAULT_SHELL, workdir.path())
            .expect("launch PTY shell via script without a controlling terminal");

        session
            .send_input(&typed_command)
            .expect("send echo command to PTY shell");

        let status = session
            .wait_for_output(&executed_output, Duration::from_secs(15))
            .expect("poll transcript for expected output");
        assert!(
            matches!(status, TerminalWaitStatus::Satisfied),
            "expected executed output '{executed_output}' to appear in the transcript, got {status:?}"
        );

        // Mirror the production recipe pattern: a trailing `exit 0` cleanly
        // terminates the inner shell so `finish` observes a success status.
        session
            .send_input("exit 0")
            .expect("send exit command to PTY shell");

        let capture = session.finish().expect("finish PTY shell session");
        assert!(
            capture.exit_status.success(),
            "inner shell should exit successfully, got {:?}",
            capture.exit_status
        );
        assert!(
            capture.transcript.contains(&executed_output),
            "final transcript must contain the executed output '{executed_output}'; transcript was:\n{}",
            capture.transcript
        );
    }
}
