//! Direct-invoke agent proxy for meeting conversations.
//!
//! Spawns the coding agent per-turn via `copilot -p "MESSAGE"`. Each turn runs
//! under a lightweight one-shot `script(1)` pseudo-terminal by default so the
//! Node-based CLI detects an interactive TTY and flushes its reply
//! incrementally — letting the streaming reader tee each line to the client as
//! it arrives (issue #2581) instead of the reply appearing only once the whole
//! turn completes. Over a plain pipe those CLIs detect a non-tty and
//! block-buffer the entire turn into a single final write, which is exactly the
//! "wait for the whole thread" latency this wrapper removes.
//!
//! This is distinct from the persistent, prompt-driven PTY session that added
//! 30-90s of handshake overhead (issue #2179): the wrapper here is a single
//! non-interactive invocation that self-terminates, so its startup cost is
//! negligible and per-turn latency stays in the ~4-15s range. Set
//! `SIMARD_AGENT_PTY_STREAM=0` to fall back to a direct piped invocation
//! (marginally faster to start, but the reply only appears once the turn ends).
//!
//! Copilot CLI does not support persistent interactive stdin when piped, so
//! each turn is a separate subprocess. The conversation context is maintained
//! by the caller (MeetingBackend), not by the agent process.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession,
    BaseTypeTurnInput, capability_set, ensure_session_not_already_open, ensure_session_not_closed,
    ensure_session_open,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

/// External PTY launcher used to give a one-shot agent turn an interactive
/// terminal so the CLI streams its output incrementally (issue #2581). Matches
/// the `terminal_session` launcher; `script(1)` is present on Linux
/// (util-linux) and macOS (BSD).
const PTY_LAUNCHER: &str = "script";

/// Whether agent turns run under a `script(1)` PTY (default) so the CLI streams
/// its reply incrementally, versus a direct piped invocation where the reply
/// only lands once the turn ends. Pure over the raw env value so the parsing is
/// unit-testable without mutating process env (mirrors `parse_turn_timeout`).
///
/// Any of `0`, `false`, `no`, `off` (case-insensitive) disables the PTY; every
/// other value — including an unset var — keeps it on.
fn pty_streaming_from_env(raw: Option<String>) -> bool {
    match raw {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        None => true,
    }
}

/// Live check of [`pty_streaming_from_env`] against the process environment.
fn pty_streaming_enabled() -> bool {
    pty_streaming_from_env(std::env::var("SIMARD_AGENT_PTY_STREAM").ok())
}

/// POSIX single-quote escaping so an arbitrary string — including the untrusted
/// user prompt — can be embedded in the `sh -c` command line that `script`
/// evaluates without any risk of shell injection. Wraps the value in single
/// quotes and rewrites each embedded quote as the canonical `'\''` sequence.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Reconstruct the full agent invocation (`<cmd> <base_args…> -p <prompt>`) as a
/// single shell-escaped command line suitable for `sh -c` / `script -c`. Every
/// token is quoted independently so neither the args nor the prompt can alter
/// the command structure.
fn agent_shell_line(agent_cmd: &str, base_args: &[String], prompt: &str) -> String {
    let mut tokens: Vec<String> = Vec::with_capacity(base_args.len() + 3);
    tokens.push(shell_single_quote(agent_cmd));
    for a in base_args {
        tokens.push(shell_single_quote(a));
    }
    tokens.push(shell_single_quote("-p"));
    tokens.push(shell_single_quote(prompt));
    tokens.join(" ")
}

/// Strip a trailing carriage return and any ANSI/VT escape sequences from one
/// line of agent output. PTY-relayed output carries CRLF line endings and may
/// embed colour/cursor control sequences the CLI emits once it detects a TTY;
/// neither belongs in the substantive chat text (issue #2581). A line with no
/// escapes is returned unchanged apart from a trailing `\r`, so this is safe to
/// apply to plain piped (non-PTY) output too. Char-based so multi-byte UTF-8 is
/// preserved intact.
///
/// Takes the line by value so the common escape-free case reuses the caller's
/// existing buffer — the trailing `\r` is popped in place and the same `String`
/// handed straight back with zero extra allocation. Only a line that actually
/// carries an ESC pays for a stripped copy. This matters because the streaming
/// reader calls this once per output line (issue #2581).
fn sanitize_terminal_line(mut line: String) -> String {
    // Drop a trailing CR from CRLF terminal endings in place — no reallocation.
    if line.ends_with('\r') {
        line.pop();
    }
    // Fast path: no escape sequences, so the buffer is already the clean line —
    // return it as-is without copying.
    if !line.contains('\u{1b}') {
        return line;
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC '[' … terminated by a final byte in 0x40..=0x7e.
            Some('[') => {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&nc) {
                        break;
                    }
                }
            }
            // String-payload control sequences: an ESC introducer followed by
            // an opaque data string terminated by ST (ESC '\') or, defensively,
            // BEL (0x07). All carry data — never visible text — so the whole
            // sequence including its payload is dropped:
            //   OSC  ESC ']'   operating system command (e.g. window title)
            //   DCS  ESC 'P'   device control string
            //   SOS  ESC 'X'   start of string
            //   PM   ESC '^'   privacy message
            //   APC  ESC '_'   application program command
            Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
                chars.next();
                while let Some(nc) = chars.next() {
                    if nc == '\u{07}' {
                        break;
                    }
                    if nc == '\u{1b}' {
                        if let Some('\\') = chars.peek() {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // Two-character escape (e.g. ESC M): drop the following byte.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// True when a single agent output line is copilot CLI noise (usage stats,
/// bootstrap banners, progress indicators) that should never be surfaced as
/// substantive response content. Shared by [`strip_copilot_noise`] (final
/// authoritative text) and the incremental streaming path (issue #2581) so the
/// live preview and the final message filter the same lines.
fn line_is_noise(trimmed: &str) -> bool {
    trimmed.starts_with("Total usage est:")
        || trimmed.starts_with("API time spent:")
        || trimmed.starts_with("Total session time:")
        || trimmed.starts_with("Changes ")
        || trimmed.starts_with("Requests ")
        || trimmed.starts_with("Tokens ")
        || (trimmed.contains("Staged") && trimmed.contains("hook"))
        || trimmed.contains("XPIA")
        || trimmed.starts_with("Script started on")
        || trimmed.starts_with("Warning:")
        || (trimmed.len() <= 2 && !trimmed.is_empty())
        || trimmed.starts_with('●')
}

/// Strip copilot CLI noise (usage stats, bootstrap lines, progress indicators)
/// from raw output, keeping only the substantive response.
fn strip_copilot_noise(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut skip_rest = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if result.is_empty() && trimmed.is_empty() {
            continue;
        }
        // The usage-stats footer (and everything after it) is discarded wholesale.
        if trimmed.starts_with("Total usage est:")
            || trimmed.starts_with("API time spent:")
            || trimmed.starts_with("Total session time:")
            || trimmed.starts_with("Changes ")
            || trimmed.starts_with("Requests ")
            || trimmed.starts_with("Tokens ")
        {
            skip_rest = true;
            continue;
        }
        if skip_rest {
            continue;
        }
        if line_is_noise(trimmed) {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

/// Primary env var setting the idle-liveness window in seconds — the maximum
/// time the agent child may produce **no output** before it is treated as
/// genuinely hung and terminated. Issue #2581: this replaces the old
/// wall-clock per-turn cap. A long-but-productive turn is never killed because
/// every streamed chunk resets the clock (see [`PersistentAgentProxy::invoke_agent_streaming`]).
const IDLE_LIVENESS_ENV: &str = "SIMARD_MEETING_IDLE_LIVENESS_SECS";

/// Deprecated alias for [`IDLE_LIVENESS_ENV`], kept working so existing
/// operator config does not break. It is NO LONGER a wall-clock per-turn cap
/// (issue #2581): when set it now configures the same idle-liveness window.
const TURN_TIMEOUT_ENV: &str = "SIMARD_MEETING_TURN_TIMEOUT_SECS";

/// Default idle-liveness window. A child that emits nothing for this long is
/// treated as hung and reaped, so a dead/stuck `copilot -p` child cannot block
/// the chat/meeting REPL forever (Pillar 11: honest degradation beats hidden
/// silence). Generous on purpose: real turns stream output within seconds, so
/// only a genuinely stalled child stays silent this long. The default is an
/// *hours*-scale window, not a minutes-scale one, so a legitimately
/// long-thinking-but-momentarily-silent agent is never killed prematurely;
/// only a truly wedged child hits this bound. Operators can still tighten or
/// disable it via `SIMARD_MEETING_IDLE_LIVENESS_SECS` (`0` = unbounded).
/// Issues #2549, #2581.
const DEFAULT_IDLE_LIVENESS_SECS: u64 = 3600;

/// Env var giving an explicit directory the meeting agent should operate in.
/// When set to an existing directory it wins over cwd-derived resolution. This
/// is the "explicit config" seam referenced by issue #2549; there is no
/// per-operator absolute path baked into the binary.
const WORKDIR_ENV: &str = "SIMARD_MEETING_AGENT_DIR";

/// Resolve the idle-liveness window from the environment, falling back to the
/// generous default. The primary [`IDLE_LIVENESS_ENV`] wins; the deprecated
/// [`TURN_TIMEOUT_ENV`] alias is consulted only when the primary is unset.
/// Issues #2549, #2581.
///
/// - `<n>` with `n > 0` → `Some(n secs)` idle window.
/// - `0` → `None` (idle detection explicitly disabled — fully unbounded escape hatch).
/// - unset or malformed → `Some(DEFAULT_IDLE_LIVENESS_SECS)`.
fn resolve_idle_window() -> Option<Duration> {
    if let Ok(raw) = std::env::var(IDLE_LIVENESS_ENV) {
        return parse_turn_timeout(Some(&raw));
    }
    parse_turn_timeout(std::env::var(TURN_TIMEOUT_ENV).ok().as_deref())
}

/// Pure (env-free) core of [`resolve_idle_window`] so the fallback/override
/// semantics are testable without mutating process-global environment state.
/// The returned `Duration` is the idle-liveness window (issue #2581).
fn parse_turn_timeout(raw: Option<&str>) -> Option<Duration> {
    match raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => Some(Duration::from_secs(DEFAULT_IDLE_LIVENESS_SECS)),
        },
        None => Some(Duration::from_secs(DEFAULT_IDLE_LIVENESS_SECS)),
    }
}

/// Resolve the directory the meeting agent should operate in — the active
/// repository, derived from the launch context, never a hardcoded operator
/// path. Issue #2549.
///
/// Resolution order:
///   1. `SIMARD_MEETING_AGENT_DIR` (explicit config) when it names a directory.
///   2. The repository root of the current working directory, via
///      `git rev-parse --show-toplevel`.
///
/// Returns `None` when no repository can be resolved (e.g. the meeting was
/// launched outside any git checkout). Callers then no-op the `--add-dir`
/// grant and let the agent inherit the process cwd rather than pointing it at
/// some other operator's worktree.
fn resolve_agent_workdir() -> Option<PathBuf> {
    // 1. Explicit operator/config override wins.
    if let Some(dir) = std::env::var_os(WORKDIR_ENV) {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            debug!(dir = %path.display(), "meeting agent workdir from {WORKDIR_ENV}");
            return Some(path);
        }
        warn!(
            dir = %path.display(),
            "{WORKDIR_ENV} is set but is not a directory — ignoring and deriving from cwd"
        );
    }

    // 2. Derive the active repository root from the current working directory.
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        debug!("git rev-parse --show-toplevel failed — meeting agent gets no repo grant");
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    let path = PathBuf::from(root);
    if path.is_dir() {
        debug!(dir = %path.display(), "meeting agent workdir derived from git repo root");
        Some(path)
    } else {
        None
    }
}

/// SIGKILL an entire process group given its leader PID (the group id equals the
/// leader's PID when the child was spawned with `process_group(0)`). Numeric-PID
/// signalling via `libc::kill` matches the repo's shell-free signal policy (see
/// `self_deploy::orphan`). Issue #2549: kills the agent subtree on a per-turn
/// timeout so no descendant keeps the stdout/stderr pipes open.
fn kill_process_group(leader_pid: u32) {
    let pid = leader_pid as i32;
    // Guard against pathological ids: `-0` targets the caller's own group and
    // `-1` broadcasts to every process. Real child PIDs are always > 1.
    if pid <= 1 {
        return;
    }
    // SAFETY: `libc::kill` is FFI but well-defined for any (pid, signal). The
    // negated group-leader PID targets exactly the child's own process group,
    // which we created via `process_group(0)`; it cannot reach this process.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// Resolve the agent command and args for one-shot `-p` invocations.
fn resolve_agent_command() -> SimardResult<(String, Vec<String>)> {
    let config = crate::runtime_config::RuntimeConfig::load()?;
    match config.llm_provider {
        crate::session_builder::LlmProvider::Copilot => Ok((
            "copilot".to_string(),
            vec![
                "--allow-all-tools".to_string(),
                "--allow-all-paths".to_string(),
            ],
        )),
        crate::session_builder::LlmProvider::RustyClawd => Ok((
            "claude".to_string(),
            vec!["--allowedTools".to_string(), "all".to_string()],
        )),
    }
}

/// A direct-invoke agent proxy that spawns the coding agent per-turn via
/// `copilot -p "MESSAGE"`.
///
/// Each turn is wrapped in a one-shot `script(1)` PTY by default so the CLI
/// streams its reply incrementally (issue #2581). This is a single
/// non-interactive invocation — not the persistent, prompt-driven PTY session
/// `CopilotSdkAdapter` uses — so it keeps the ~4-15s/turn latency rather than
/// the 30-90s of the old interactive PTY path (issue #2179).
/// `SIMARD_AGENT_PTY_STREAM=0` opts back into a direct piped invocation (no PTY,
/// but the reply then only appears once the turn ends).
pub struct PersistentAgentProxy {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
    /// Idle-liveness window: the child is reaped only after producing no output
    /// for this long (`None` = idle detection disabled). Every streamed chunk
    /// resets the clock, so a productive turn of any length is never killed
    /// (issue #2581). Replaces the former wall-clock per-turn cap.
    idle_window: Option<Duration>,
    /// Directory the agent operates in (cwd + `--add-dir` grant), resolved in
    /// `open()` from the active repo / explicit config. `None` when no repo can
    /// be resolved — the agent then inherits the process cwd with no grant.
    workdir: Option<PathBuf>,
    agent_cmd: String,
    agent_base_args: Vec<String>,
}

impl std::fmt::Debug for PersistentAgentProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentAgentProxy")
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .field("turn_count", &self.turn_count)
            .finish()
    }
}

impl PersistentAgentProxy {
    /// Create a new proxy (does NOT validate agent yet — call `open()` first).
    pub fn new() -> SimardResult<Self> {
        let idle_window = resolve_idle_window();

        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("persistent-agent-proxy"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "persistent-agent-proxy::direct",
                    "persistent-agent-proxy:one-shot",
                    Freshness::now()?,
                ),
                capabilities: capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                ]),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            is_open: false,
            is_closed: false,
            turn_count: 0,
            idle_window,
            workdir: None,
            agent_cmd: String::new(),
            agent_base_args: Vec::new(),
        })
    }

    /// Validate that the agent binary exists.
    fn validate_agent(&self) -> SimardResult<()> {
        let check = Command::new("which")
            .arg(&self.agent_cmd)
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!("failed to check for agent binary: {e}"),
            })?;

        if !check.status.success() {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!("agent binary '{}' not found in PATH", self.agent_cmd),
            });
        }
        Ok(())
    }

    /// Build the per-turn agent spawn command (program + argv only; stdio,
    /// process group, and working directory are applied by the caller so both
    /// modes share them).
    ///
    /// When `pty` is true (the default; see [`pty_streaming_enabled`]) the agent
    /// runs under a one-shot `script(1)` PTY so the Node-based CLI
    /// (`copilot`/`claude`) sees an interactive terminal and flushes its reply
    /// incrementally, letting [`Self::invoke_agent_streaming`] tee each line to
    /// the client as it is produced (issue #2581). Over a plain pipe those CLIs
    /// detect a non-tty and block-buffer the whole turn into a single final
    /// write, so nothing streams until the turn completes. The untrusted prompt
    /// is shell-escaped via [`agent_shell_line`] before it reaches `sh -c`.
    ///
    /// When `pty` is false the agent is spawned directly with its argv, giving
    /// the piped, non-tty stdio of the original thin proxy (issue #2179).
    fn build_agent_command(&self, prompt: &str, pty: bool) -> Command {
        if !pty {
            let mut cmd = Command::new(&self.agent_cmd);
            cmd.args(&self.agent_base_args).arg("-p").arg(prompt);
            return cmd;
        }

        let agent_line = agent_shell_line(&self.agent_cmd, &self.agent_base_args, prompt);
        let mut cmd = Command::new(PTY_LAUNCHER);
        if cfg!(target_os = "macos") {
            // BSD `script` (macOS): `-F` flushes each write; the typescript file
            // is positional and precedes the command, which we run via an
            // explicit shell so the escaped argv is honored identically.
            cmd.arg("-qFe")
                .arg("/dev/null")
                .arg("/bin/sh")
                .arg("-c")
                .arg(agent_line);
        } else {
            // util-linux `script` (Linux): `-f` flushes each write, `-e`
            // propagates the child exit code, and `-c` takes the command
            // string; the typescript is discarded to /dev/null.
            //
            // util-linux `script -c` runs the command via `$SHELL`, falling
            // back to `/bin/sh` only when it is unset. Pin `SHELL=/bin/sh` so
            // an operator's non-POSIX login shell (fish, csh, …) can never
            // reinterpret the POSIX single-quote-escaped command line — the
            // macOS branch above already names `/bin/sh` explicitly. This
            // hardens the one shell-boundary crossing (security review #2581).
            cmd.env("SHELL", "/bin/sh")
                .arg("-qefc")
                .arg(agent_line)
                .arg("/dev/null");
        }
        cmd
    }

    /// Invoke the agent with a prompt and return the full (noise-stripped)
    /// response. Thin wrapper over [`Self::invoke_agent_streaming`] with a
    /// no-op chunk sink, for callers that don't need incremental output.
    #[cfg(test)]
    fn invoke_agent(&self, prompt: &str) -> SimardResult<String> {
        self.invoke_agent_streaming(prompt, &mut |_| {})
    }

    /// Invoke the agent, streaming each substantive output line to `on_chunk`
    /// as it is produced, and returning the full noise-stripped response.
    ///
    /// Liveness model (issue #2581): there is **no** wall-clock cap on the
    /// turn. The child is terminated only when it produces no output for the
    /// idle-liveness window ([`Self::idle_window`]) — every line received
    /// resets that clock, so a long-but-productive turn streams indefinitely
    /// and is never killed. A genuinely hung/dead child (no output for the full
    /// window) is still reaped and surfaced as an honest idle-timeout error.
    fn invoke_agent_streaming(
        &self,
        prompt: &str,
        on_chunk: &mut dyn FnMut(&str),
    ) -> SimardResult<String> {
        info!(
            cmd = %self.agent_cmd,
            prompt_len = prompt.len(),
            idle_window_secs = self.idle_window.map(|d| d.as_secs()),
            "Invoking agent (streaming)"
        );

        // Build the turn command — under a one-shot `script(1)` PTY by default
        // so the agent CLI streams its reply incrementally instead of
        // block-buffering the whole turn behind a non-tty pipe (issue #2581).
        // `SIMARD_AGENT_PTY_STREAM=0` opts back into the direct piped path.
        let mut cmd = self.build_agent_command(prompt, pty_streaming_enabled());
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Run the agent as the leader of its own process group so the liveness
        // reaper can kill the WHOLE subtree, not just the direct child. The
        // agent CLIs (`copilot`/`claude`) are Node processes that spawn
        // descendants which inherit the stdout/stderr pipe write-ends; killing
        // only the direct child would leave those descendants alive, holding
        // the pipes open so the reader threads never see EOF (leaked threads +
        // FDs + orphaned processes). `process_group(0)` makes the child's PGID
        // equal its PID; the reaper then signals the negated PGID.
        //
        // Tradeoff: the child no longer shares Simard's foreground process
        // group, so a terminal SIGINT (Ctrl-C) sent to Simard mid-turn no
        // longer reaches the agent. That is acceptable here — the agent is a
        // one-shot `-p` invocation that self-terminates, and the idle-liveness
        // reaper still reaps a genuinely hung subtree.
        cmd.process_group(0);

        // Operate in the active repository so the agent can inspect code and
        // run `simard` commands in the correct context. Resolved in `open()`
        // from the active repo / explicit config — never a hardcoded operator
        // path (issue #2549). When unresolved, inherit the process cwd.
        if let Some(dir) = &self.workdir {
            cmd.current_dir(dir);
        }

        let start = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!("failed to spawn '{}': {e}", self.agent_cmd),
            })?;

        // Read stdout in a thread, forwarding each line over a channel.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: "failed to capture agent stdout".to_string(),
            })?;

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        // Drain stderr in a thread
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    debug!(stderr_line = %line, "agent stderr");
                }
            });
        }

        // Collect stdout lines, streaming each substantive one to `on_chunk`.
        // The channel disconnects EXACTLY when the reader thread reaches stdout
        // EOF (every writer has closed the pipe) — the authoritative "all output
        // received" signal. Draining via the blocking `recv_timeout` until
        // `Disconnected` (rather than `try_wait` + a non-blocking `try_recv`
        // drain) guarantees we never drop a burst the child flushed just before
        // exiting — critical because `copilot`/`claude` write to a pipe
        // (block-buffered) and tend to flush a large final chunk.
        //
        // Liveness (issue #2581): the loop tracks `last_activity`, reset on every
        // line. It NEVER blocks on `child.wait()` unbounded, so a child that
        // closes stdout then hangs (`exec 1>&-; sleep 999`), or one that never
        // closes stdout, still degrades once it has been idle for the full
        // window — while a child that keeps producing output runs unbounded.
        let mut lines: Vec<String> = Vec::new();
        let mut hung = false;
        let mut stdout_eof = false;
        let mut last_activity = Instant::now();
        loop {
            if let Some(idle) = self.idle_window
                && last_activity.elapsed() >= idle
            {
                warn!(
                    idle_window_secs = idle.as_secs(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "Agent produced no output for the idle-liveness window — reaping genuinely-hung child"
                );
                // Kill the entire process group (the agent + any descendants it
                // spawned) so no orphan keeps the stdout/stderr pipes open —
                // otherwise the reader threads block on `read()` forever. The
                // child leads its own group (`process_group(0)` above), so its
                // PID is the group id; signalling the negated PID reaches the
                // whole group. Fall back to a direct child kill if the group
                // signal fails for any reason.
                kill_process_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                hung = true;
                break;
            }

            if stdout_eof {
                // All output has been received (reader reached EOF). Finish once
                // the child is reaped; if it closed stdout yet keeps running,
                // keep polling so the idle deadline above can still fire.
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ => std::thread::sleep(Duration::from_millis(50)),
                }
                continue;
            }

            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    // Fresh output — reset the idle-liveness clock so a
                    // productive turn is never reaped regardless of total length.
                    last_activity = Instant::now();
                    // PTY-relayed output carries CRLF endings and may embed ANSI
                    // control sequences the CLI emits once it sees a TTY; strip
                    // both so the streamed preview and the final joined response
                    // are clean text (a no-op for plain piped output).
                    let line = sanitize_terminal_line(line);
                    if !line_is_noise(line.trim()) {
                        on_chunk(&line);
                    }
                    lines.push(line);
                }
                // No line this tick — loop to re-check the idle deadline.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                // Reader hit stdout EOF and forwarded every line before dropping
                // its sender: `lines` is now complete. The child may still be
                // running (it closed stdout early), so switch to bounded
                // exit/idle polling above.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => stdout_eof = true,
            }
        }

        // Honest degradation (Pillar 11, issues #2549/#2581): a child that went
        // idle for the full liveness window is genuinely hung — surface a
        // specific error rather than blocking the REPL or masquerading as an
        // empty response. The REPL's `render_backend_error` turns this into the
        // `[meeting:error] WARNING` banner with a "retry or /close" hint. This
        // fires ONLY on genuine inactivity, never on a still-streaming turn.
        if hung {
            let secs = self
                .idle_window
                .map(|d| d.as_secs())
                .unwrap_or(DEFAULT_IDLE_LIVENESS_SECS);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!(
                    "agent produced no output for {secs}s (idle-liveness timeout, \
                     {IDLE_LIVENESS_ENV}); terminated genuinely-hung child — honest \
                     degradation, retry your message or /close"
                ),
            });
        }

        let elapsed = start.elapsed();
        let raw_response = lines.join("\n");
        info!(
            elapsed_ms = elapsed.as_millis() as u64,
            raw_len = raw_response.len(),
            lines = lines.len(),
            "Agent invocation complete"
        );

        Ok(strip_copilot_noise(&raw_response))
    }

    /// Shared turn logic behind both [`BaseTypeSession::run_turn`] and
    /// [`BaseTypeSession::run_turn_streaming`]. Builds the prompt, invokes the
    /// agent (streaming each chunk to `on_chunk`), records cost, and returns the
    /// outcome. A no-op `on_chunk` yields the classic blocking behaviour.
    fn run_turn_streaming_impl(
        &mut self,
        input: BaseTypeTurnInput,
        on_chunk: &mut dyn FnMut(&str),
    ) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        self.turn_count += 1;

        // Build the full prompt including context on first turn
        let prompt = if self.turn_count == 1 {
            let mut parts = Vec::new();
            if !input.identity_context.is_empty() {
                parts.push(input.identity_context.as_str());
            }
            if !input.prompt_preamble.is_empty() {
                parts.push(input.prompt_preamble.as_str());
            }
            parts.push(&input.objective);
            parts.join("\n\n")
        } else {
            input.objective.clone()
        };

        info!(
            turn = self.turn_count,
            prompt_len = prompt.len(),
            "Agent proxy: sending turn"
        );
        let start = Instant::now();

        let response_text = self.invoke_agent_streaming(&prompt, on_chunk)?;

        info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            response_len = response_text.len(),
            turn = self.turn_count,
            "Agent proxy: received response"
        );

        if response_text.trim().is_empty() {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: "agent returned empty response".to_string(),
            });
        }

        // Record cost estimate
        if let Err(e) = crate::cost_tracking::record_cost(
            "persistent-agent-proxy",
            "direct-invoke",
            prompt.len(),
            response_text.len(),
            &format!("agent proxy turn {}", self.turn_count),
        ) {
            debug!("Cost tracking write failed: {e}");
        }

        Ok(BaseTypeOutcome {
            plan: format!("Agent proxy turn {} (direct-invoke).", self.turn_count),
            execution_summary: response_text,
            evidence: vec![
                format!("agent-proxy-turn={}", self.turn_count),
                format!("elapsed-ms={}", start.elapsed().as_millis()),
            ],
        })
    }
}

impl BaseTypeSession for PersistentAgentProxy {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        let (cmd, mut args) = resolve_agent_command()?;
        // Give the meeting agent access to the active repository so it can
        // inspect code, run `simard goal`, and execute other CLI commands in
        // the correct repository context. The directory is derived from the
        // launch context (explicit `SIMARD_MEETING_AGENT_DIR` or the current
        // repo root) — never a hardcoded operator path (issue #2549). When no
        // repository resolves, the grant is a no-op rather than a wrong path.
        let workdir = resolve_agent_workdir();
        if let Some(dir) = &workdir {
            args.push("--add-dir".to_string());
            args.push(dir.to_string_lossy().into_owned());
        } else {
            warn!(
                "meeting agent: no repository resolved from cwd/{WORKDIR_ENV} — \
                 agent runs without an explicit --add-dir grant"
            );
        }
        self.workdir = workdir;
        self.agent_cmd = cmd;
        self.agent_base_args = args;
        self.validate_agent()?;
        self.is_open = true;
        info!(
            cmd = %self.agent_cmd,
            workdir = ?self.workdir,
            "Agent proxy opened (direct-invoke mode)"
        );
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        self.run_turn_streaming_impl(input, &mut |_| {})
    }

    fn run_turn_streaming(
        &mut self,
        input: BaseTypeTurnInput,
        on_chunk: &mut dyn FnMut(&str),
    ) -> SimardResult<BaseTypeOutcome> {
        self.run_turn_streaming_impl(input, on_chunk)
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;
        info!("Closing agent proxy");
        self.is_closed = true;
        Ok(())
    }
}

impl Drop for PersistentAgentProxy {
    fn drop(&mut self) {
        // Nothing to clean up — no persistent subprocess
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_proxy() {
        let proxy = PersistentAgentProxy::new();
        assert!(proxy.is_ok());
        let proxy = proxy.unwrap();
        assert!(!proxy.is_open);
        assert!(!proxy.is_closed);
        assert_eq!(proxy.turn_count, 0);
    }

    #[test]
    fn resolve_agent_command_returns_valid_command() {
        let _result = resolve_agent_command();
    }

    #[test]
    fn strip_copilot_noise_removes_usage_stats() {
        let input = "Here is the answer.\nTotal usage est: 1234 tokens\nAPI time spent: 2.3s";
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Here is the answer.");
    }

    #[test]
    fn strip_copilot_noise_removes_bootstrap() {
        let input = "Staged 3 hook files\nXPIA defender loaded\nActual response here.";
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Actual response here.");
    }

    #[test]
    fn strip_copilot_noise_passes_clean_text() {
        let input = "Normal response.\nWith multiple lines.";
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Normal response.\nWith multiple lines.");
    }

    // ── issue #2581: PTY streaming helpers ──

    #[test]
    fn pty_streaming_defaults_on_when_unset() {
        assert!(
            pty_streaming_from_env(None),
            "unset env must keep incremental PTY streaming enabled by default"
        );
    }

    #[test]
    fn pty_streaming_disabled_by_falsey_values() {
        for v in ["0", "false", "FALSE", "no", "Off", " off "] {
            assert!(
                !pty_streaming_from_env(Some(v.to_string())),
                "{v:?} must disable PTY streaming"
            );
        }
    }

    #[test]
    fn pty_streaming_enabled_by_other_values() {
        for v in ["1", "true", "yes", "on", ""] {
            assert!(
                pty_streaming_from_env(Some(v.to_string())),
                "{v:?} must keep PTY streaming enabled"
            );
        }
    }

    #[test]
    fn shell_single_quote_escapes_embedded_quote() {
        // A prompt attempting to break out of the sh -c string stays inert.
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_single_quote("plain"), "'plain'");
    }

    #[test]
    fn agent_shell_line_quotes_every_token() {
        let base = vec!["--allow-all-tools".to_string()];
        let line = agent_shell_line("copilot", &base, "hi; rm -rf /");
        // Structure: '<cmd>' '<arg>' '-p' '<prompt>' — all single-quoted so the
        // injected ';' and spaces are literal prompt text, not shell syntax.
        assert_eq!(line, "'copilot' '--allow-all-tools' '-p' 'hi; rm -rf /'");
    }

    #[test]
    fn sanitize_terminal_line_strips_cr_and_ansi() {
        // Trailing CR from CRLF terminal endings.
        assert_eq!(sanitize_terminal_line("hello\r".to_string()), "hello");
        // CSI colour codes around real text.
        assert_eq!(
            sanitize_terminal_line("\u{1b}[32mgreen\u{1b}[0m text\r".to_string()),
            "green text"
        );
        // OSC sequence (window title) terminated by BEL.
        assert_eq!(
            sanitize_terminal_line("\u{1b}]0;title\u{07}visible".to_string()),
            "visible"
        );
        // DCS/SOS/PM/APC string sequences carry an opaque payload terminated by
        // ST (ESC '\') — the whole sequence, payload included, must be dropped.
        assert_eq!(
            sanitize_terminal_line("\u{1b}Pq#0;2;0;0;0\u{1b}\\shown".to_string()),
            "shown"
        );
        assert_eq!(
            sanitize_terminal_line("\u{1b}_private data\u{1b}\\kept".to_string()),
            "kept"
        );
        assert_eq!(
            sanitize_terminal_line("\u{1b}^pm payload\u{1b}\\end".to_string()),
            "end"
        );
    }

    #[test]
    fn sanitize_terminal_line_preserves_plain_and_utf8() {
        assert_eq!(sanitize_terminal_line("just text".to_string()), "just text");
        // Multi-byte UTF-8 must survive intact (char-based scan).
        assert_eq!(sanitize_terminal_line("café ☕\r".to_string()), "café ☕");
        assert_eq!(
            sanitize_terminal_line("\u{1b}[1mbold café ☕\u{1b}[0m".to_string()),
            "bold café ☕"
        );
    }

    // ── issues #2549/#2581: default idle-liveness window ──

    #[test]
    fn parse_turn_timeout_unset_uses_bounded_default() {
        assert_eq!(
            parse_turn_timeout(None),
            Some(Duration::from_secs(DEFAULT_IDLE_LIVENESS_SECS)),
            "unset env must yield the bounded idle-liveness default, not None (no hang)"
        );
    }

    #[test]
    fn parse_turn_timeout_positive_override_wins() {
        assert_eq!(
            parse_turn_timeout(Some("30")),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_turn_timeout(Some("  45  ")),
            Some(Duration::from_secs(45)),
            "surrounding whitespace must be tolerated"
        );
    }

    #[test]
    fn parse_turn_timeout_zero_disables_explicitly() {
        assert_eq!(
            parse_turn_timeout(Some("0")),
            None,
            "0 is the explicit operator escape hatch (idle detection fully disabled)"
        );
    }

    #[test]
    fn parse_turn_timeout_malformed_falls_back_to_default() {
        assert_eq!(
            parse_turn_timeout(Some("not-a-number")),
            Some(Duration::from_secs(DEFAULT_IDLE_LIVENESS_SECS)),
            "malformed value must degrade to the bounded default, never None"
        );
    }

    #[test]
    fn new_defaults_to_bounded_turn_timeout_when_env_unset() {
        // With neither env var set the proxy must carry a bounded idle-liveness
        // window so a genuinely-hung child cannot block the REPL forever (a
        // still-streaming child is never reaped — the clock resets per chunk).
        // Only assert the default when the operator has NOT set an override.
        if std::env::var(TURN_TIMEOUT_ENV).is_err() && std::env::var(IDLE_LIVENESS_ENV).is_err() {
            let proxy = PersistentAgentProxy::new().unwrap();
            assert_eq!(
                proxy.idle_window,
                Some(Duration::from_secs(DEFAULT_IDLE_LIVENESS_SECS)),
                "new() must default to a bounded idle-liveness window"
            );
        }
    }

    // ── issue #2549: repo-derived workdir (no hardcoded operator path) ──

    #[test]
    fn resolve_agent_workdir_derives_repo_root_from_cwd() {
        // `cargo test` runs inside this git checkout, so resolution must yield
        // a real repository root — and it must NOT be the old hardcoded path.
        let resolved = resolve_agent_workdir()
            .expect("workdir should resolve to the repo root inside a git checkout");
        assert!(resolved.is_dir(), "resolved workdir must exist");
        assert!(
            resolved.join(".git").exists(),
            "resolved workdir must be a git repository root: {}",
            resolved.display()
        );
        assert_ne!(
            resolved,
            PathBuf::from("/home/azureuser/src/Simard/worktrees/main"),
            "workdir must never be the hardcoded operator path (issue #2549)"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_agent_workdir_honors_explicit_override() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let prev = std::env::var_os(WORKDIR_ENV);
        // SAFETY: env mutation is serialised via the serial key above.
        unsafe { std::env::set_var(WORKDIR_ENV, tmp.path()) };

        let resolved = resolve_agent_workdir();

        // Restore before asserting so a panic cannot leak the override.
        unsafe {
            match &prev {
                Some(v) => std::env::set_var(WORKDIR_ENV, v),
                None => std::env::remove_var(WORKDIR_ENV),
            }
        }

        let resolved = resolved.expect("explicit override must resolve");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap(),
            "SIMARD_MEETING_AGENT_DIR must win over cwd-derived resolution"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_agent_workdir_ignores_nonexistent_override() {
        let prev = std::env::var_os(WORKDIR_ENV);
        // SAFETY: env mutation is serialised via the serial key above.
        unsafe { std::env::set_var(WORKDIR_ENV, "/nonexistent/simard/meeting/dir") };

        let resolved = resolve_agent_workdir();

        unsafe {
            match &prev {
                Some(v) => std::env::set_var(WORKDIR_ENV, v),
                None => std::env::remove_var(WORKDIR_ENV),
            }
        }

        // A bogus override must fall through to cwd-derivation (the repo root),
        // never a hardcoded path — so inside the checkout we still get a repo.
        let resolved = resolved.expect("should fall through to repo root");
        assert_ne!(
            resolved,
            PathBuf::from("/home/azureuser/src/Simard/worktrees/main"),
            "must not resolve to the hardcoded operator path"
        );
    }

    // ── issues #2549/#2581: honest degradation on a genuinely idle/hung turn ──

    #[test]
    fn invoke_agent_degrades_honestly_on_timeout() {
        // AC (b): a genuinely idle/hung child — one that produces NO output for
        // the whole idle-liveness window — must still be detected and reaped,
        // surfacing an honest idle-timeout error (never a silent hang, never a
        // masqueraded empty response). `sh -c 'sleep 30'` ignores the trailing
        // `-p <prompt>` args (they become $0/$1), so it hangs, silent,
        // deterministically.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "sleep 30".to_string()];
        proxy.idle_window = Some(Duration::from_millis(500));

        let started = Instant::now();
        let result = proxy.invoke_agent("hello");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "invoke_agent must not block indefinitely on a hung child (took {elapsed:?})"
        );
        let err = result.expect_err("a genuinely-idle turn must surface an error, not Ok");
        let msg = err.to_string();
        assert!(
            msg.contains("idle-liveness") && msg.contains("timeout") && msg.contains("honest"),
            "error must be a clear honest idle-liveness timeout, got: {msg}"
        );
    }

    #[test]
    fn invoke_agent_degrades_when_child_closes_stdout_then_hangs() {
        // Regression for the disconnected-branch hang (issue #2549 review): a
        // child that closes its stdout but keeps running (and produces no
        // output) must STILL be reaped by idle-liveness, not block the REPL
        // forever. `exec 1>&-` closes the stdout write-end (the reader thread
        // sees EOF immediately) and then the shell sleeps — the idle-centric
        // loop must reap it once the window elapses with no activity.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "exec 1>&-; sleep 30".to_string()];
        proxy.idle_window = Some(Duration::from_secs(2));

        let started = Instant::now();
        let result = proxy.invoke_agent("hello");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "must not block when the child closes stdout then hangs (took {elapsed:?})"
        );
        let err = result.expect_err("a hung turn (stdout closed) must surface a timeout error");
        assert!(
            err.to_string().contains("timeout"),
            "error must name the timeout, got: {err}"
        );
    }

    #[test]
    fn invoke_agent_timeout_reaps_descendant_processes() {
        // A hung turn must kill the WHOLE agent subtree, not just the direct
        // child — otherwise a descendant holding the stdout pipe leaks (and the
        // reader thread blocks forever). We spawn a shell that backgrounds a
        // grandchild `sleep` and records its PID, then force a timeout. After
        // the group-kill the grandchild must be gone. Without the process-group
        // kill (only `child.kill()`), the grandchild would survive its full 30s.
        //
        // A process is treated as terminated when `/proc/<pid>` is gone OR the
        // process is a zombie (state `Z`) — a zombie no longer holds the pipe
        // and is merely awaiting reaping, so it does not represent a leak.
        fn terminated(pid: i32) -> bool {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(_) => true, // no /proc entry → gone
                Ok(stat) => {
                    // "<pid> (<comm>) <state> ..." — comm may contain spaces or
                    // ')', so scan from the LAST ')' to find the state field.
                    stat.rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().next())
                        .map(|state| state == "Z")
                        .unwrap_or(true)
                }
            }
        }

        let pidfile = tempfile::NamedTempFile::new().expect("pidfile");
        let pidpath = pidfile.path().to_string_lossy().into_owned();

        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        // Single-quote the pidfile path so an unusual temp path can't break the
        // shell word-splitting. The grandchild PID is recorded immediately, well
        // before the timeout fires.
        proxy.agent_base_args = vec![
            "-c".to_string(),
            format!("sleep 30 & echo $! > '{pidpath}'; wait"),
        ];
        // 3s is comfortably longer than the shell needs to record the PID, and
        // far shorter than the grandchild's 30s sleep, so the grandchild is
        // guaranteed alive when the group-kill fires.
        proxy.idle_window = Some(Duration::from_secs(3));

        let result = proxy.invoke_agent("hello");
        assert!(result.is_err(), "hung turn must surface a timeout error");

        // Read the grandchild PID the shell recorded before it was killed.
        let mut grandchild_pid = None;
        for _ in 0..100 {
            if let Ok(raw) = std::fs::read_to_string(&pidpath)
                && let Ok(pid) = raw.trim().parse::<i32>()
                && pid > 1
            {
                grandchild_pid = Some(pid);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid = grandchild_pid.expect("shell should have recorded the grandchild PID");

        // The group-kill must reap the grandchild promptly.
        let mut reaped = false;
        for _ in 0..100 {
            if terminated(pid) {
                reaped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !reaped {
            // Best-effort direct cleanup so a failing assertion doesn't leak the
            // `sleep`. Positive PID targets exactly the grandchild.
            // SAFETY: `libc::kill` is well-defined for any (pid, signal).
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        assert!(
            reaped,
            "timeout must reap the agent's descendant (pid {pid}); it survived the group-kill"
        );
    }

    // ── streaming vs thin-proxy stdio (issues #2179 / #2581) ──

    #[test]
    fn invoke_agent_streams_under_a_pty_by_default() {
        // Streaming (issue #2581) requires the agent to see an interactive
        // terminal so it flushes incrementally instead of block-buffering the
        // whole turn behind a non-tty pipe. By default the turn therefore runs
        // under a one-shot `script(1)` PTY: a child that inspects its own
        // descriptors observes tty stdin AND stdout. (`SIMARD_AGENT_PTY_STREAM`
        // is unset in the test process, so the default PTY path is exercised.)
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "printf 'stdin='; if [ -t 0 ]; then printf 'tty'; else printf 'notty'; fi; \
             printf ' stdout='; if [ -t 1 ]; then printf 'tty'; else printf 'notty'; fi"
                .to_string(),
        ];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent("hello")
            .expect("PTY probe child must return Ok");
        assert_eq!(
            response, "stdin=tty stdout=tty",
            "streaming turns must run the agent under a PTY so it flushes \
             incrementally (issue #2581)"
        );
    }

    #[test]
    fn build_agent_command_direct_mode_is_a_plain_pipe() {
        // With the PTY disabled the agent is spawned directly with its own argv
        // — no `script(1)`/PTY/bash wrapper — preserving the thin-proxy path
        // (issue #2179) as an opt-out.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "copilot".to_string();
        proxy.agent_base_args = vec!["--allow-all-tools".to_string()];

        let cmd = proxy.build_agent_command("hi there", false);
        assert_eq!(cmd.get_program(), "copilot");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["--allow-all-tools", "-p", "hi there"]);
    }

    #[test]
    fn build_agent_command_pty_mode_wraps_script_with_escaped_prompt() {
        // With the PTY enabled the agent runs under `script(1)`; the full
        // invocation is passed as a single shell-escaped command string so an
        // adversarial prompt cannot break out of `sh -c`.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "copilot".to_string();
        proxy.agent_base_args = vec!["--allow-all-tools".to_string()];

        let cmd = proxy.build_agent_command("hi; rm -rf /", true);
        assert_eq!(cmd.get_program(), PTY_LAUNCHER);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // The escaped agent line must appear verbatim as one argument, with the
        // injected `;` quoted inside the prompt rather than as shell syntax.
        let expected_line = "'copilot' '--allow-all-tools' '-p' 'hi; rm -rf /'";
        assert!(
            args.iter().any(|a| a == expected_line),
            "escaped agent line must be passed as a single argument: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "/dev/null"),
            "typescript output must be discarded to /dev/null: {args:?}"
        );
        // Security hardening: the util-linux `script -c` path must pin
        // `SHELL=/bin/sh` so a non-POSIX operator login shell cannot reinterpret
        // the single-quote-escaped command line (macOS names /bin/sh directly).
        #[cfg(not(target_os = "macos"))]
        {
            let shell = cmd
                .get_envs()
                .find(|(k, _)| *k == std::ffi::OsStr::new("SHELL"))
                .and_then(|(_, v)| v)
                .map(|v| v.to_string_lossy().into_owned());
            assert_eq!(
                shell.as_deref(),
                Some("/bin/sh"),
                "script -c must run the command under /bin/sh, not $SHELL"
            );
        }
    }

    #[test]
    fn invoke_agent_returns_output_when_child_exits_before_timeout() {
        // Proves the non-timeout happy path still works: a child that prints
        // and exits within the bound returns its (noise-stripped) output.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "printf 'meeting-proxy-ok\\n'".to_string()];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent("hello")
            .expect("a prompt child that exits cleanly must return Ok");
        assert_eq!(response, "meeting-proxy-ok");
    }

    #[test]
    fn invoke_agent_captures_full_burst_before_exit() {
        // Regression for the try_wait/reader race (issue #2549 review): a child
        // that flushes a LARGE burst and exits immediately must have EVERY line
        // captured. `copilot`/`claude` write to a pipe (block-buffered) and
        // routinely flush a big final chunk right before exit; the old
        // `try_wait` + non-blocking-`try_recv` drain dropped that burst. We use
        // 4-digit numbers so `strip_copilot_noise` (which drops lines of length
        // <= 2) keeps all of them, and a burst (~30 KB) far larger than the
        // reader's buffer so the race would trigger on the old code.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "seq 1000 7000".to_string()];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent("hello")
            .expect("a burst child that exits cleanly must return Ok");
        let received: Vec<&str> = response.lines().collect();
        assert_eq!(
            received.len(),
            6001,
            "must capture the full burst without dropping any lines (got {})",
            received.len()
        );
        assert_eq!(received.first().copied(), Some("1000"));
        assert_eq!(received.last().copied(), Some("7000"));
    }

    // ── issue #2581: idle-liveness never kills a productive turn + streaming ──

    #[test]
    fn long_productive_turn_is_not_killed_and_streams_incrementally() {
        // AC (a): a turn whose TOTAL runtime far exceeds the idle-liveness
        // window must NOT be killed, as long as it keeps producing output — and
        // its output must arrive incrementally (streamed), not in one final
        // blob. The child prints a 4-digit line (survives noise-stripping) every
        // 100 ms for 20 lines (~2 s total) with a 1 s idle window: 2× the window
        // overall, but each gap (0.1 s) is well under it, so the clock keeps
        // resetting. Under the OLD wall-clock cap this turn would have been
        // killed at 1 s; under idle-liveness it runs to completion. This is the
        // bounded, CI-safe stand-in for the ">120 s productive turn" case — the
        // property proven (no upper bound while output flows) is identical.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "i=1000; while [ $i -lt 1020 ]; do echo $i; i=$((i+1)); sleep 0.1; done".to_string(),
        ];
        proxy.idle_window = Some(Duration::from_secs(1));

        let mut chunks: Vec<String> = Vec::new();
        let started = Instant::now();
        let response = proxy
            .invoke_agent_streaming("hello", &mut |c| chunks.push(c.to_string()))
            .expect("a long-but-productive turn must return Ok, never be killed");
        let elapsed = started.elapsed();

        // It ran the full duration — proof it was not reaped early at ~1 s.
        assert!(
            elapsed >= Duration::from_millis(1500),
            "productive turn was cut short (took only {elapsed:?}) — idle-liveness wrongly killed it"
        );
        // Output arrived incrementally: one streamed chunk per produced line.
        assert_eq!(
            chunks.len(),
            20,
            "expected 20 incrementally-streamed chunks, got {}",
            chunks.len()
        );
        assert_eq!(chunks.first().map(String::as_str), Some("1000"));
        assert_eq!(chunks.last().map(String::as_str), Some("1019"));
        // The final aggregate response carries every line.
        assert_eq!(response.lines().count(), 20);
    }

    #[test]
    fn streaming_filters_noise_lines_from_chunks() {
        // Streamed chunks are the substantive lines only — copilot usage/banner
        // noise is filtered from the live preview exactly as it is from the
        // final text, so the two never disagree.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "printf 'hello world\\nTotal usage est: 5 tokens\\n'".to_string(),
        ];
        proxy.idle_window = Some(Duration::from_secs(30));

        let mut chunks: Vec<String> = Vec::new();
        let response = proxy
            .invoke_agent_streaming("hi", &mut |c| chunks.push(c.to_string()))
            .expect("clean child must return Ok");

        assert_eq!(
            chunks,
            vec!["hello world".to_string()],
            "noise must be filtered from stream"
        );
        assert_eq!(
            response, "hello world",
            "final text must match the streamed substantive content"
        );
    }
}
