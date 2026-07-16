//! Direct-invoke agent proxy for meeting conversations.
//!
//! Spawns the coding agent per-turn with the prompt delivered on STDIN and
//! piped stdout — no PTY, no script(1), no bash wrapper. This is the "thin
//! proxy" replacing the 30-90s PTY overhead (issue #2179).
//!
//! The prompt is streamed on stdin (never inlined as a `-p <MESSAGE>` argv
//! token) so an arbitrarily large turn can never overflow `ARG_MAX` and make
//! `exec` fail with E2BIG ("Argument list too long") — the live Signal defect
//! fixed by issue #2640. Each turn is a separate subprocess; the conversation
//! context is maintained by the caller (MeetingBackend), not by the agent
//! process.

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

/// Resolve the agent command and its fixed-size base args. The prompt is
/// delivered on STDIN (issue #2640), never inlined on `argv`, so no arm carries
/// an inline prompt: `copilot` omits `-p` entirely (it reads its prompt from
/// stdin when `-p` is absent), while `claude` carries a BARE `-p` (print mode)
/// so it honours the piped-stdin prompt. Either way `argv` stays constant-size
/// and can never overflow `ARG_MAX`.
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
            vec![
                "-p".to_string(),
                "--allowedTools".to_string(),
                "all".to_string(),
            ],
        )),
    }
}

/// A direct-invoke agent proxy that spawns the coding agent per-turn with the
/// prompt streamed on STDIN and stdout captured over a pipe.
///
/// Unlike `CopilotSdkAdapter` (which uses PTY/script per turn), this proxy
/// invokes the agent directly and captures stdout — no PTY allocation, no
/// script(1) wrapper, no bash intermediary. The prompt rides on stdin, never on
/// `argv`, so a large turn cannot overflow `ARG_MAX` and E2BIG on `exec`
/// (issue #2640).
///
/// Response time: ~4-15s per turn vs 30-90s with the old PTY path.
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

    /// Build the per-turn agent command carrying ONLY the fixed-size base flags
    /// on `argv` — the prompt is delivered out-of-band on **stdin** (issue
    /// #2640), never as an argv token, so this `argv` is prompt-independent and
    /// can never overflow `ARG_MAX`. Configures the piped stdio, process group,
    /// and workdir shared by every invocation; the caller attaches the stdin
    /// prompt via [`crate::spawn_payload::attach_prompt_std`] before spawning.
    fn build_agent_command(&self) -> Command {
        let mut cmd = Command::new(&self.agent_cmd);
        cmd.args(&self.agent_base_args)
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
        // one-shot invocation that self-terminates, and the idle-liveness
        // reaper still reaps a genuinely hung subtree.
        cmd.process_group(0);

        // Operate in the active repository so the agent can inspect code and
        // run `simard` commands in the correct context. Resolved in `open()`
        // from the active repo / explicit config — never a hardcoded operator
        // path (issue #2549). When unresolved, inherit the process cwd.
        if let Some(dir) = &self.workdir {
            cmd.current_dir(dir);
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

        let mut cmd = self.build_agent_command();

        // Deliver the (possibly large) prompt on STDIN via the single spawn
        // facade, never as an argv token: the agent reads its prompt from stdin
        // (copilot when no `-p` is given; claude in `-p` print mode), so the
        // prompt never contributes to `ARG_MAX` regardless of size. Inlining it
        // as `-p <prompt>` made `exec` fail with E2BIG ("Argument list too
        // long") the instant a large Signal turn was routed here (elapsed_ms=0)
        // — the live defect fixed by issue #2640, mirroring the proven
        // base_type_copilot stdin transport. Sets the child's stdin to a pipe;
        // the bytes are written by the feeder thread below after spawn.
        let applied = crate::spawn_payload::attach_prompt_std(&mut cmd, prompt.as_bytes())
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!("failed to prepare agent prompt delivery: {e}"),
            })?;

        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| {
            // A pre-exec spawn failure (E2BIG / ENOMEM / ENOENT / …) has no
            // child and no `ExitStatus`, so the exit-code classifier never sees
            // it. Classify and record it into the Overseer failure sink so the
            // failure is diagnosed at its launch site, never silently swallowed
            // (issue #2640).
            crate::spawn_payload::record_spawn_failure(&e, "meeting-agent-proxy");
            SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!("failed to spawn '{}': {e}", self.agent_cmd),
            }
        })?;

        // Feed the prompt on a dedicated thread so a large prompt cannot
        // deadlock against the child filling its stdout pipe while we are still
        // writing stdin. The feeder closes stdin on completion so the agent
        // reads EOF. Joined after the stdout loop below.
        let stdin = child.stdin.take();
        let feeder = std::thread::spawn(move || applied.feed(stdin));

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

        // Join the stdin feeder now the child has exited / been reaped. A child
        // that exits (or is reaped on idle) before consuming its stdin closes
        // the read end, so the feeder's write gets `BrokenPipe` — expected here
        // (an agent that answers from a short prefix of the prompt, or the idle
        // reaper killing a hung child) and tolerated. Any OTHER feed error, or a
        // panic, is a real transport fault and is surfaced loudly — no silent
        // fallback (issue #2640).
        match feeder.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                debug!(
                    "agent stdin feeder: child closed stdin before consuming the \
                     prompt (BrokenPipe) — tolerated"
                );
            }
            Ok(Err(e)) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: "persistent-agent-proxy".to_string(),
                    reason: format!("failed to stream agent prompt on stdin: {e}"),
                });
            }
            Err(_) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: "persistent-agent-proxy".to_string(),
                    reason: "agent prompt feeder thread panicked".to_string(),
                });
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
    fn resolve_agent_command_maps_provider_to_a_known_agent() {
        // Config may be unavailable in a headless test env; but whenever it
        // resolves, the command must be one of the two supported agents with
        // its canonical argv — never an empty or arbitrary program.
        if let Ok((program, args)) = resolve_agent_command() {
            match program.as_str() {
                "copilot" => {
                    assert_eq!(args, vec!["--allow-all-tools", "--allow-all-paths"]);
                }
                "claude" => assert_eq!(args, vec!["-p", "--allowedTools", "all"]),
                other => panic!("unexpected agent program: {other:?}"),
            }
        }
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
        // masqueraded empty response). `sh -c 'sleep 30'` ignores its stdin
        // prompt entirely, so it hangs, silent, deterministically.
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

    // ── thin-proxy invariant (issue #2179): no PTY, piped stdio ──

    #[test]
    fn invoke_agent_uses_piped_stdio_not_a_pty() {
        // The thin proxy spawns the agent with a piped stdin (carrying the
        // prompt) and a piped stdout — never a PTY/script(1)/bash wrapper (issue
        // #2179, replacing the 30-90s PTY overhead). A child that inspects its
        // own descriptors must therefore observe NON-tty stdin and stdout; if a
        // PTY leaked back in, `[ -t 0 ]` / `[ -t 1 ]` would report a terminal
        // and this assertion would fail.
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
            .expect("no-PTY probe child must return Ok");
        assert_eq!(
            response, "stdin=notty stdout=notty",
            "proxy must invoke the agent over piped (non-PTY) stdio — no \
             script(1)/PTY/bash wrapper (issue #2179)"
        );
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

    // ── issue #2640: argv-free stdin prompt transport (meeting/Signal proxy) ──
    //
    // LIVE BUG: an inbound Signal message routed through a meeting session hits
    // `PersistentAgentProxy::invoke_agent_streaming`, which used to inline the
    // whole turn prompt as a `-p <prompt>` argv token. A large prompt (or one
    // crossing `MAX_ARG_STRLEN`) made `execve` return E2BIG ("Argument list too
    // long") PRE-EXEC, so the turn failed instantly (`elapsed_ms=0`) and the
    // user's message never reached the agent. The fix delivers the prompt on
    // STDIN (via `spawn_payload::attach_prompt_std` + a feeder thread) and puts
    // only fixed-size flags on argv. These tests pin that contract; they FAIL
    // against the pre-fix `-p <prompt>` code (T1 references the not-yet-created
    // `build_agent_command` seam; the runtime tests E2BIG or mis-shape argv).
    //
    // All hermetic: no network, no Signal, no real copilot/claude — argv-shape
    // is asserted on the builder seam without spawning, and the round-trip /
    // argv-free tests drive `cat` / `sh -c` stand-ins through the private
    // `agent_cmd` + `agent_base_args` seam (mirroring the liveness tests above).

    /// T1 — argv-constant guard (spawn-free). The command builder carries ONLY
    /// the fixed-size base flags; the prompt is not even a parameter, so the
    /// argv is prompt-independent BY CONSTRUCTION. Asserted on
    /// `build_agent_command().get_args()` with no spawn. Because
    /// `attach_prompt_std` adds no argv tokens, this vector is exactly what
    /// `execve` receives — proving the prompt left argv.
    #[test]
    fn build_agent_command_argv_is_fixed_and_prompt_free() {
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "copilot".to_string();
        proxy.agent_base_args = vec![
            "--allow-all-tools".to_string(),
            "--allow-all-paths".to_string(),
        ];

        // NOTE: `build_agent_command` is the fixed-argv seam introduced by the
        // fix. It takes NO prompt — that is the whole point (the prompt can
        // never reach argv even by accident). This call fails to compile against
        // the pre-fix code, which is the intended TDD "fails initially" signal.
        let cmd = proxy.build_agent_command();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args, proxy.agent_base_args,
            "build_agent_command must place ONLY the fixed base args on argv — \
             no -p, no prompt token"
        );
        assert!(
            !args.iter().any(|a| a == "-p"),
            "copilot argv must not contain -p: the prompt travels on stdin"
        );
        let argv_bytes: usize = args.iter().map(String::len).sum();
        assert!(
            argv_bytes < 4096,
            "argv must stay small and constant-size (got {argv_bytes} bytes) — \
             a prompt-sized argv is exactly the E2BIG defect (#2640)"
        );
    }

    /// T1 (runtime) — the crown-jewel regression guard: a >256 KiB prompt must
    /// spawn via STDIN, never argv. The child echoes its OWN argv (`$@`); if any
    /// prompt byte reached argv, (a) `execve` would E2BIG on the single ~300 KB
    /// argument and the child would never run, and (b) the sentinel would appear
    /// in the echoed argv. After the fix the prompt is on stdin, the child runs,
    /// and its argv is free of the sentinel. This also pins the BrokenPipe
    /// tolerance: `sh -c 'printf …'` exits WITHOUT reading its (huge) stdin, so
    /// the feeder gets EPIPE — which must be tolerated, not surfaced as an error.
    #[test]
    fn large_prompt_is_delivered_on_stdin_not_argv() {
        const SENTINEL: &str = "MEETING_SIGNAL_ARGV_SENTINEL";
        let prompt = format!("{SENTINEL}_{}", "X".repeat(300 * 1024));
        assert!(
            prompt.len() > 256 * 1024,
            "test payload must exceed 256 KiB to exercise the E2BIG boundary"
        );

        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "printf 'ARGV_START'; printf '<%s>' \"$@\"; printf 'ARGV_END'".to_string(),
            // Becomes $0. The fix must add NO further argv token; the pre-fix
            // code appended `-p <300 KB prompt>` here → instant E2BIG.
            "meeting-argv-probe".to_string(),
        ];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent(&prompt)
            .expect("a >256 KiB prompt must spawn (stdin transport) without E2BIG");

        assert!(
            response.contains("ARGV_START") && response.contains("ARGV_END"),
            "the child must have executed (argv small enough to exec): {response:?}"
        );
        assert!(
            !response.contains(SENTINEL),
            "prompt bytes must NEVER appear in the child's argv — they belong on \
             stdin (got: {response:?})"
        );
    }

    /// T2 — stdin round-trip: a `>= 256 KiB` prompt handed to `cat` (which reads
    /// stdin and echoes it) comes back in full and byte-exact. Proves the stdin
    /// transport, that a large payload does NOT E2BIG, and that nothing is
    /// truncated. Also pins the feeder-thread requirement: `cat` back-pressures
    /// stdout while the feeder writes 256 KiB to stdin, so an inline (non-thread)
    /// feeder would deadlock and be reaped by the idle window (Err) instead of
    /// returning the payload.
    #[test]
    fn large_prompt_round_trips_through_stdin_without_truncation() {
        // Build a >=256 KiB payload of clean, noise-free lines (each distinct so
        // the assertion also proves ordering and no de-duplication). Every line
        // is > 2 chars and matches no `line_is_noise` marker, so
        // `strip_copilot_noise` is a no-op and the round-trip is exact.
        let mut payload = String::new();
        let mut i = 0usize;
        while payload.len() < 256 * 1024 {
            if i > 0 {
                payload.push('\n');
            }
            payload.push_str(&format!("meeting-signal-payload-line-{i:08}-abcdefghij"));
            i += 1;
        }
        assert!(payload.len() >= 256 * 1024);

        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "cat".to_string();
        proxy.agent_base_args = vec![];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent(&payload)
            .expect("a >=256 KiB prompt must round-trip via stdin with no E2BIG");

        assert_eq!(
            response.len(),
            payload.len(),
            "stdin payload must round-trip without truncation"
        );
        assert_eq!(
            response, payload,
            "cat must echo the exact bytes fed on stdin (full-fidelity transport)"
        );
    }

    /// T3 — small-prompt happy path proving stdin delivery to a child that reads
    /// stdin. The prompt is fed on stdin (not argv); a `sh -c` stand-in reads it
    /// and echoes it back. Against the pre-fix `Stdio::null()` stdin this returns
    /// `got:` (empty) — so it fails until the prompt is piped.
    #[test]
    fn small_prompt_reaches_child_on_stdin() {
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "IFS= read -r line; printf '%s' \"got:$line\"".to_string(),
        ];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent("hello-from-stdin")
            .expect("prompt fed on stdin must reach a child that reads stdin");
        assert_eq!(
            response, "got:hello-from-stdin",
            "the prompt must arrive on the child's stdin, not on argv"
        );
    }

    /// T4 — injection inertness. A prompt full of shell metacharacters
    /// (`$(id)`, backtick `whoami`, `;`, `#`, a `rm -rf` token) fed to `cat`
    /// round-trips byte-identically with NO execution: there is no shell in the
    /// payload path, so the metacharacters are inert data. The `rm` target is a
    /// non-existent path purely as defence-in-depth — nothing here ever reaches
    /// a shell.
    #[test]
    fn shell_metacharacters_in_prompt_are_inert() {
        let payload = "$(id);`whoami`;rm -rf /tmp/nonexistent-meeting-signal-marker #e2big";

        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "cat".to_string();
        proxy.agent_base_args = vec![];
        proxy.idle_window = Some(Duration::from_secs(30));

        let response = proxy
            .invoke_agent(payload)
            .expect("cat must echo the raw prompt bytes");
        assert_eq!(
            response, payload,
            "prompt must round-trip byte-identically — no shell interpretation"
        );
        assert!(
            response.contains("$(id)") && response.contains("`whoami`"),
            "command-substitution text must be preserved verbatim, never expanded"
        );
    }

    // T5 — liveness regression: the existing sleep / `exec 1>&-` / `seq` /
    // `printf` / no-tty tests above (`invoke_agent_degrades_honestly_on_timeout`,
    // `invoke_agent_uses_piped_stdio_not_a_pty`, etc.) must stay green after the
    // transport change. `sh -c` scripts ignore stdin, so moving the prompt off
    // argv does not perturb them. No new test is added here — those tests ARE T5.

    /// T6 — per-provider stdin invocation shape. Neither provider carries the
    /// prompt on argv: `copilot` omits `-p` (it reads stdin when `-p` is absent),
    /// while `claude` gains a BARE `-p` (print mode) with the prompt piped on
    /// stdin. Serialized on the same key as the `runtime_config` env tests
    /// because it forces the provider via `SIMARD_LLM_PROVIDER`.
    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn resolve_agent_command_shapes_argv_for_stdin_prompt() {
        let saved = std::env::var(crate::runtime_config::ENV_LLM_PROVIDER).ok();

        // Copilot: no -p on argv (prompt on stdin).
        unsafe { std::env::set_var(crate::runtime_config::ENV_LLM_PROVIDER, "copilot") };
        let (cmd, args) = resolve_agent_command().expect("copilot provider must resolve");
        assert_eq!(cmd, "copilot");
        assert!(
            !args.iter().any(|a| a == "-p"),
            "copilot argv must NOT carry -p (prompt goes on stdin): {args:?}"
        );
        assert_eq!(
            args,
            vec![
                "--allow-all-tools".to_string(),
                "--allow-all-paths".to_string(),
            ],
        );

        // RustyClawd/claude: a BARE -p (print mode) with the prompt on stdin.
        unsafe { std::env::set_var(crate::runtime_config::ENV_LLM_PROVIDER, "rustyclawd") };
        let (cmd, args) = resolve_agent_command().expect("rustyclawd provider must resolve");
        assert_eq!(cmd, "claude");
        assert!(
            args.iter().any(|a| a == "-p"),
            "claude argv must carry a bare -p (print mode) so the piped stdin \
             prompt is honoured: {args:?}"
        );
        assert_eq!(
            args,
            vec![
                "-p".to_string(),
                "--allowedTools".to_string(),
                "all".to_string(),
            ],
        );

        // Restore the ambient env so other tests start clean.
        match saved {
            Some(v) => unsafe { std::env::set_var(crate::runtime_config::ENV_LLM_PROVIDER, v) },
            None => unsafe { std::env::remove_var(crate::runtime_config::ENV_LLM_PROVIDER) },
        }
    }
}
