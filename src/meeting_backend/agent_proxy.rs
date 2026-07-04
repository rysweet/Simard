//! Direct-invoke agent proxy for meeting conversations.
//!
//! Spawns the coding agent per-turn via `copilot -p "MESSAGE"` with piped
//! stdout — no PTY, no script(1), no bash wrapper. This is the "thin proxy"
//! replacing the 30-90s PTY overhead (issue #2179).
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

use super::streaming_sanitizer::StreamingSanitizer;

/// Env var (removed) that used to impose a fixed wall-clock per-turn timeout.
/// It is no longer honored: a long-but-productive agent turn must never be
/// killed by a wall-clock bound (operator directive; mirrors the amplihack
/// recipe-runner idle-timeout switch, issue #439). If an operator still has it
/// set, [`resolve_turn_idle_timeout`] emits a deprecation warning pointing at
/// [`TURN_IDLE_ENV`]. Issue #2586 follow-up.
const LEGACY_TURN_TIMEOUT_ENV: &str = "SIMARD_MEETING_TURN_TIMEOUT_SECS";

/// Env var overriding the per-turn *idle* timeout in seconds. A turn is only
/// terminated when the child produces NO output (stdout content or stderr
/// heartbeat) for this long; as long as it keeps producing output the turn runs
/// with no upper bound. Configurable but not disableable — it is the honest
/// hung-child safety net. Values below [`MIN_TURN_IDLE_SECS`] are clamped up.
const TURN_IDLE_ENV: &str = "SIMARD_MEETING_TURN_IDLE_SECS";

/// Default per-turn idle timeout: a generous 10 minutes. A healthy agent emits
/// tokens/log lines far more often than this; the window only fires for a
/// genuinely wedged child.
const DEFAULT_TURN_IDLE_SECS: u64 = 600;

/// Floor for the configured idle window. Guards against an operator setting a
/// tiny value that would false-positive on a slow-but-live turn (and against a
/// `0` that would otherwise disable the honest hang detector).
const MIN_TURN_IDLE_SECS: u64 = 5;

/// Metric emitted (value `1.0`) when a turn is terminated for idle-timeout, so
/// a real hang is surfaced explicitly, never silently swallowed. Snake_case to
/// match the `self_metrics` JSONL convention.
const IDLE_TIMEOUT_METRIC: &str = "meeting_turn_idle_timeout";

/// Env var giving an explicit directory the meeting agent should operate in.
/// When set to an existing directory it wins over cwd-derived resolution. This
/// is the "explicit config" seam referenced by issue #2549; there is no
/// per-operator absolute path baked into the binary.
const WORKDIR_ENV: &str = "SIMARD_MEETING_AGENT_DIR";

/// Resolve the per-turn idle timeout from the environment, clamped to
/// [`MIN_TURN_IDLE_SECS`] and defaulting to [`DEFAULT_TURN_IDLE_SECS`]. Emits a
/// deprecation warning if the removed wall-clock env var is still set.
fn resolve_turn_idle_timeout() -> Duration {
    if let Ok(stale) = std::env::var(LEGACY_TURN_TIMEOUT_ENV) {
        warn!(
            removed_var = LEGACY_TURN_TIMEOUT_ENV,
            stale_value = %stale,
            replacement = TURN_IDLE_ENV,
            "{LEGACY_TURN_TIMEOUT_ENV} is no longer honored — a productive turn is \
             never killed by a wall-clock bound; only idle turns time out. Set \
             {TURN_IDLE_ENV} to tune the idle window instead."
        );
    }
    parse_turn_idle_timeout(std::env::var(TURN_IDLE_ENV).ok().as_deref())
}

/// Pure (env-free) core of [`resolve_turn_idle_timeout`] so the clamp/default
/// semantics are testable without mutating process-global environment state.
///
/// - `Some("<n>")` with `n >= MIN` → `n` seconds.
/// - `Some("<n>")` with `n < MIN` (including `0`) → clamped to
///   [`MIN_TURN_IDLE_SECS`] — the idle detector is not disableable.
/// - `None` or malformed → [`DEFAULT_TURN_IDLE_SECS`].
fn parse_turn_idle_timeout(raw: Option<&str>) -> Duration {
    let secs = match raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(secs) => secs.max(MIN_TURN_IDLE_SECS),
            Err(_) => DEFAULT_TURN_IDLE_SECS,
        },
        None => DEFAULT_TURN_IDLE_SECS,
    };
    Duration::from_secs(secs)
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

// ── Idle-liveness drain seams (issue #2586 follow-up) ───────────────────────
//
// The per-turn drain loop is extracted behind two tiny traits so its
// idle-timeout behavior is deterministically testable with a fake child + fake
// clock — no real subprocess, no real sleeps. [`ChildOutput`] yields the
// child's output events with idle-aware waiting; [`TurnClock`] supplies
// monotonic elapsed time for logging/idle accounting.

/// One unit of progress observed from the agent child.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChildEvent {
    /// A line of stdout — substantive content AND a liveness signal.
    Stdout(String),
    /// A line of stderr — a liveness heartbeat only (logged at debug).
    Stderr(String),
    /// The child's stdout reached EOF: all content has been produced.
    StdoutEof,
}

/// Source of [`ChildEvent`]s with idle-aware waiting. The production impl wraps
/// the reader-thread channel; tests substitute a scripted fake.
trait ChildOutput {
    /// Wait up to `idle_budget` for the next event. `Some(event)` if one
    /// arrived within the budget; `None` if the budget elapsed with no activity
    /// (the child is idle).
    fn next_event(&mut self, idle_budget: Duration) -> Option<ChildEvent>;
}

/// Real [`ChildOutput`] over the stdout/stderr reader-thread channel. A
/// `recv_timeout` that times out is exactly "no output within the idle window";
/// a disconnect (all readers gone) is treated as stdout EOF.
struct PipeChildOutput {
    rx: std::sync::mpsc::Receiver<ChildEvent>,
}

impl ChildOutput for PipeChildOutput {
    fn next_event(&mut self, idle_budget: Duration) -> Option<ChildEvent> {
        match self.rx.recv_timeout(idle_budget) {
            Ok(event) => Some(event),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Some(ChildEvent::StdoutEof),
        }
    }
}

/// Monotonic clock seam so idle accounting is deterministic under test.
trait TurnClock {
    /// Milliseconds elapsed since the turn started (monotonic, non-decreasing).
    fn elapsed_ms(&self) -> u64;
}

/// Production [`TurnClock`] backed by [`Instant`].
struct SystemClock {
    start: Instant,
}

impl SystemClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl TurnClock for SystemClock {
    fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// Outcome of [`drain_with_liveness`].
#[derive(Debug, PartialEq, Eq)]
enum DrainOutcome {
    /// stdout reached EOF; the turn produced its full output. `elapsed_ms` is
    /// the total wall time (unbounded — proves there is no wall-clock cap).
    Completed { elapsed_ms: u64 },
    /// No output for a full idle window: the child is wedged. `elapsed_ms` is
    /// the total wall time at the point of detection.
    IdleTimeout { idle: Duration, elapsed_ms: u64 },
}

/// Drain a child's output, resetting the idle window on every event and only
/// giving up when NO output arrives for a full `idle_timeout`.
///
/// Productive turns run with no upper time bound: each stdout line is forwarded
/// to `on_stdout` (a combined content + liveness signal) as it arrives — true
/// incremental streaming — and stderr heartbeats keep the turn alive without
/// producing content. Only a genuinely silent child trips the idle timeout.
fn drain_with_liveness(
    src: &mut dyn ChildOutput,
    clock: &dyn TurnClock,
    idle_timeout: Duration,
    on_stdout: &mut dyn FnMut(&str),
) -> DrainOutcome {
    loop {
        match src.next_event(idle_timeout) {
            Some(ChildEvent::Stdout(line)) => on_stdout(&line),
            Some(ChildEvent::Stderr(line)) => {
                debug!(stderr_line = %line, "agent stderr (liveness heartbeat)");
            }
            Some(ChildEvent::StdoutEof) => {
                return DrainOutcome::Completed {
                    elapsed_ms: clock.elapsed_ms(),
                };
            }
            None => {
                return DrainOutcome::IdleTimeout {
                    idle: idle_timeout,
                    elapsed_ms: clock.elapsed_ms(),
                };
            }
        }
    }
}

/// Structured description of an idle-timeout hang, used to build the honest
/// error and emit the surfacing metric.
struct IdleTimeoutReport {
    idle: Duration,
    pid: Option<u32>,
    turn: u32,
}

/// Surface an idle-timeout hang: structured error-level tracing plus a metric
/// (identifiers only — never user/model content), returning the honest
/// degradation error. `metric` is injected so tests can assert emission without
/// writing to the metrics store. A real hang is never silently swallowed.
fn report_idle_timeout(
    report: &IdleTimeoutReport,
    metric: &mut dyn FnMut(&str, f64, &str),
) -> SimardError {
    let idle_secs = report.idle.as_secs();
    let context = format!(
        "idle_secs={idle_secs};pid={};turn={}",
        report
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        report.turn
    );
    tracing::error!(
        idle_secs,
        pid = report.pid,
        turn = report.turn,
        "meeting agent turn idle-timeout — no output for {idle_secs}s; terminating wedged child ({TURN_IDLE_ENV})"
    );
    metric(IDLE_TIMEOUT_METRIC, 1.0, &context);
    SimardError::AdapterInvocationFailed {
        base_type: "persistent-agent-proxy".to_string(),
        reason: format!(
            "agent turn idle-timeout: no output for {idle_secs}s \
             ({TURN_IDLE_ENV}); terminated wedged child — honest degradation, \
             retry your message or /close"
        ),
    }
}

/// Best-effort bounded reap of a child that has already closed stdout. The
/// turn's full output is already in hand; this only stops a descendant that
/// lingers after closing stdout from orphaning. Never blocks unbounded — after
/// a short grace the process group is killed.
fn reap_after_eof(child: &mut std::process::Child, pid: u32) {
    let grace = Duration::from_millis(500);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {
                if start.elapsed() >= grace {
                    kill_process_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

/// A direct-invoke agent proxy that spawns the coding agent per-turn via
/// `copilot -p "MESSAGE"` with piped stdout.
///
/// Unlike `CopilotSdkAdapter` (which uses PTY/script per turn), this proxy
/// invokes the agent directly with `-p` flag and captures stdout — no PTY
/// allocation, no script(1) wrapper, no bash intermediary.
///
/// Response time: ~4-15s per turn vs 30-90s with the old PTY path.
pub struct PersistentAgentProxy {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
    /// Per-turn *idle* timeout: the turn is killed only after the child
    /// produces no output for this long. A productive turn has no upper bound.
    turn_idle_timeout: Duration,
    /// Whether idle-timeout terminations emit a metric to the metrics store.
    /// Always `true` in production; tests set it `false` so the suite never
    /// writes to `~/.simard`.
    emit_metrics: bool,
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
        let turn_idle_timeout = resolve_turn_idle_timeout();

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
            turn_idle_timeout,
            emit_metrics: true,
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

    /// Invoke the agent with a prompt and return the (noise-stripped) response,
    /// discarding streamed deltas. Thin wrapper over
    /// [`PersistentAgentProxy::invoke_agent_streaming`], used by the tests that
    /// assert non-streaming behavior. Production turns go through
    /// `run_turn_streaming` → `invoke_agent_streaming` directly.
    #[cfg(test)]
    fn invoke_agent(&self, prompt: &str) -> SimardResult<String> {
        self.invoke_agent_streaming(prompt, &mut |_| {})
    }

    /// Invoke the agent, forwarding cleaned output fragments to `on_delta` as
    /// the child produces them (true incremental streaming) and returning the
    /// full noise-stripped response.
    ///
    /// The turn is terminated ONLY if the child goes idle — no stdout content
    /// and no stderr heartbeat — for the configured idle window
    /// ([`TURN_IDLE_ENV`]); a productive turn runs with no upper time bound. An
    /// idle kill reaps the whole process group, surfaces the hang via
    /// error-level tracing + a metric, and degrades honestly.
    fn invoke_agent_streaming(
        &self,
        prompt: &str,
        on_delta: &mut dyn FnMut(&str),
    ) -> SimardResult<String> {
        info!(
            cmd = %self.agent_cmd,
            prompt_len = prompt.len(),
            "Invoking agent"
        );

        let mut cmd = Command::new(&self.agent_cmd);
        cmd.args(&self.agent_base_args)
            .arg("-p")
            .arg(prompt)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Run the agent as the leader of its own process group so an idle-timeout
        // kill can reap the WHOLE subtree, not just the direct child. The agent
        // CLIs (`copilot`/`claude`) are Node processes that spawn descendants
        // inheriting the stdout/stderr pipe write-ends; killing only the direct
        // child would leave those descendants holding the pipes open so the
        // reader threads never see EOF (leaked threads + FDs + orphans).
        // `process_group(0)` makes the child's PGID equal its PID; the idle
        // handler signals the negated PGID. Tradeoff: a terminal SIGINT to
        // Simard no longer reaches the agent mid-turn — acceptable for a
        // one-shot `-p` invocation whose idle timeout still reaps a wedged tree.
        cmd.process_group(0);

        if let Some(dir) = &self.workdir {
            cmd.current_dir(dir);
        }

        let clock = SystemClock::new();
        let mut child = cmd
            .spawn()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!("failed to spawn '{}': {e}", self.agent_cmd),
            })?;
        let child_pid = child.id();

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: "failed to capture agent stdout".to_string(),
            })?;

        // Both reader threads feed one ordered channel of `ChildEvent`s. The
        // stdout thread emits an explicit `StdoutEof` at end-of-output — the
        // authoritative "all content produced" signal — so the drain finishes
        // without ever blocking on `child.wait()`.
        let (tx, rx) = std::sync::mpsc::channel::<ChildEvent>();
        let stdout_tx = tx.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if stdout_tx.send(ChildEvent::Stdout(line)).is_err() {
                    return;
                }
            }
            let _ = stdout_tx.send(ChildEvent::StdoutEof);
        });
        if let Some(stderr) = child.stderr.take() {
            let stderr_tx = tx.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if stderr_tx.send(ChildEvent::Stderr(line)).is_err() {
                        return;
                    }
                }
            });
        }
        // Drop our own sender so the channel disconnects once both reader
        // threads finish (defensive: the stdout thread already emits StdoutEof).
        drop(tx);

        let mut src = PipeChildOutput { rx };
        let mut sanitizer = StreamingSanitizer::new();
        let outcome = {
            // Each raw stdout line runs through the single incremental sanitizer;
            // kept fragments are forwarded to the live stream. Concatenating the
            // fragments and trimming equals `sanitizer.finish()` — so what
            // streams matches what is persisted, by construction.
            let mut on_stdout = |line: &str| {
                if let Some(delta) = sanitizer.push_line(line) {
                    on_delta(&delta);
                }
            };
            drain_with_liveness(&mut src, &clock, self.turn_idle_timeout, &mut on_stdout)
        };

        match outcome {
            DrainOutcome::IdleTimeout { idle, elapsed_ms } => {
                warn!(
                    idle_secs = idle.as_secs(),
                    elapsed_ms, "Agent turn idle-timeout reached — killing wedged child"
                );
                kill_process_group(child_pid);
                let _ = child.kill();
                let _ = child.wait();
                let report = IdleTimeoutReport {
                    idle,
                    pid: Some(child_pid),
                    turn: self.turn_count,
                };
                let emit = self.emit_metrics;
                let mut sink = |name: &str, value: f64, ctx: &str| {
                    if emit {
                        let _ = crate::self_metrics::record_metric(name, value, ctx);
                    }
                };
                Err(report_idle_timeout(&report, &mut sink))
            }
            DrainOutcome::Completed { elapsed_ms } => {
                // stdout EOF: the turn's full output is in hand. Reap the child
                // (bounded, best-effort) so a descendant lingering after closing
                // stdout doesn't orphan — but never block on it.
                reap_after_eof(&mut child, child_pid);
                let response = sanitizer.finish();
                info!(
                    elapsed_ms,
                    raw_len = response.len(),
                    "Agent invocation complete"
                );
                Ok(response)
            }
        }
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
        // Single code path: the non-streaming turn is streaming with a no-op
        // delta sink, so behavior cannot diverge between the two.
        self.run_turn_streaming(input, &mut |_| {})
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn run_turn_streaming(
        &mut self,
        input: BaseTypeTurnInput,
        on_delta: &mut dyn FnMut(&str),
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

        let response_text = self.invoke_agent_streaming(&prompt, on_delta)?;

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

    // ── issue #2586 follow-up: idle-liveness timeout config ──

    #[test]
    fn parse_turn_idle_unset_uses_generous_default() {
        assert_eq!(
            parse_turn_idle_timeout(None),
            Duration::from_secs(DEFAULT_TURN_IDLE_SECS),
            "unset env must yield the generous idle default"
        );
    }

    #[test]
    fn parse_turn_idle_positive_override_wins() {
        assert_eq!(parse_turn_idle_timeout(Some("30")), Duration::from_secs(30));
        assert_eq!(
            parse_turn_idle_timeout(Some("  45  ")),
            Duration::from_secs(45),
            "surrounding whitespace must be tolerated"
        );
    }

    #[test]
    fn parse_turn_idle_is_not_disableable_clamps_to_floor() {
        // The idle detector is the honest hung-child safety net: `0` (or any
        // sub-floor value) must clamp up, never disable it.
        assert_eq!(
            parse_turn_idle_timeout(Some("0")),
            Duration::from_secs(MIN_TURN_IDLE_SECS),
            "0 must clamp to the floor — idle detection is not disableable"
        );
        assert_eq!(
            parse_turn_idle_timeout(Some("1")),
            Duration::from_secs(MIN_TURN_IDLE_SECS),
            "a below-floor value must clamp up"
        );
    }

    #[test]
    fn parse_turn_idle_malformed_falls_back_to_default() {
        assert_eq!(
            parse_turn_idle_timeout(Some("not-a-number")),
            Duration::from_secs(DEFAULT_TURN_IDLE_SECS),
            "malformed value must degrade to the generous default"
        );
    }

    #[test]
    fn new_defaults_to_generous_idle_timeout_when_env_unset() {
        // With the idle env unset the proxy must carry the generous default.
        // Only assert when the operator has NOT set an override here.
        if std::env::var(TURN_IDLE_ENV).is_err() {
            let proxy = PersistentAgentProxy::new().unwrap();
            assert_eq!(
                proxy.turn_idle_timeout,
                Duration::from_secs(DEFAULT_TURN_IDLE_SECS),
                "new() must default to the generous idle timeout"
            );
        }
    }

    // ── issue #2586 follow-up: idle-liveness drain (fake child + fake clock) ──

    /// A deterministic clock whose virtual "now" the test advances explicitly.
    #[derive(Clone, Default)]
    struct FakeClock {
        ms: std::rc::Rc<std::cell::Cell<u64>>,
    }

    impl FakeClock {
        fn advance(&self, by_ms: u64) {
            self.ms.set(self.ms.get() + by_ms);
        }
    }

    impl TurnClock for FakeClock {
        fn elapsed_ms(&self) -> u64 {
            self.ms.get()
        }
    }

    /// A scripted [`ChildOutput`]: each step is either an event (optionally
    /// advancing the shared clock to model the passage of time between events)
    /// or an idle gap that returns `None` (no output within the budget).
    struct FakeChild {
        clock: FakeClock,
        /// (advance_ms_before, Some(event) | None-for-idle)
        steps: std::collections::VecDeque<(u64, Option<ChildEvent>)>,
    }

    impl ChildOutput for FakeChild {
        fn next_event(&mut self, _idle_budget: Duration) -> Option<ChildEvent> {
            match self.steps.pop_front() {
                Some((advance_ms, event)) => {
                    self.clock.advance(advance_ms);
                    event
                }
                // No script left: behave as stdout EOF so the drain terminates.
                None => Some(ChildEvent::StdoutEof),
            }
        }
    }

    /// A turn that keeps producing output for far longer than the former 120s
    /// wall-clock limit is NOT killed: it completes and every delta streams.
    #[test]
    fn drain_productive_turn_over_120s_is_not_killed_and_streams() {
        let clock = FakeClock::default();
        // 200 stdout lines, 30 virtual seconds apart → 6000s virtual elapsed,
        // ~50x the old 120s cap. No wall-clock bound exists any more.
        let mut steps: std::collections::VecDeque<(u64, Option<ChildEvent>)> = (0..200)
            .map(|i| {
                (
                    30_000u64,
                    Some(ChildEvent::Stdout(format!("token line {i}"))),
                )
            })
            .collect();
        steps.push_back((1_000, Some(ChildEvent::StdoutEof)));
        let mut child = FakeChild {
            clock: clock.clone(),
            steps,
        };

        let mut deltas = Vec::new();
        let outcome = drain_with_liveness(&mut child, &clock, Duration::from_secs(600), &mut |d| {
            deltas.push(d.to_string())
        });

        match outcome {
            DrainOutcome::Completed { elapsed_ms } => {
                assert!(
                    elapsed_ms > 120_000,
                    "virtual elapsed {elapsed_ms}ms must far exceed the old 120s cap"
                );
            }
            other => panic!("a productive turn must complete, got {other:?}"),
        }
        assert_eq!(
            deltas.len(),
            200,
            "every produced line must stream as a delta"
        );
        assert_eq!(deltas[0], "token line 0");
    }

    /// A child that goes fully idle past the window IS terminated, and the hang
    /// is surfaced explicitly (honest error + metric emitted).
    #[test]
    fn drain_idle_child_times_out_and_reports_hang_with_metric() {
        let clock = FakeClock::default();
        let mut steps = std::collections::VecDeque::new();
        steps.push_back((1_000, Some(ChildEvent::Stdout("thinking...".to_string()))));
        // Then an idle gap: the source returns None (no output within budget).
        steps.push_back((600_000, None));
        let mut child = FakeChild {
            clock: clock.clone(),
            steps,
        };

        let outcome =
            drain_with_liveness(&mut child, &clock, Duration::from_secs(600), &mut |_| {});
        let idle = match outcome {
            DrainOutcome::IdleTimeout { idle, .. } => idle,
            other => panic!("a fully-idle child must idle-timeout, got {other:?}"),
        };

        // The hang must surface as an honest error AND emit exactly one metric,
        // via the injected sink (so no write to ~/.simard).
        let mut emitted: Vec<(String, f64, String)> = Vec::new();
        let report = IdleTimeoutReport {
            idle,
            pid: Some(4321),
            turn: 2,
        };
        let err = report_idle_timeout(&report, &mut |name, value, ctx| {
            emitted.push((name.to_string(), value, ctx.to_string()));
        });
        let msg = err.to_string();
        assert!(
            msg.contains("idle-timeout") && msg.contains("honest"),
            "error must be an honest idle-timeout degradation, got: {msg}"
        );
        assert_eq!(emitted.len(), 1, "exactly one metric must be emitted");
        assert_eq!(emitted[0].0, IDLE_TIMEOUT_METRIC);
        assert_eq!(emitted[0].1, 1.0);
        assert!(
            emitted[0].2.contains("idle_secs=600") && emitted[0].2.contains("pid=4321"),
            "metric context must carry identifiers only, got: {}",
            emitted[0].2
        );
    }

    /// Continued output past the idle window keeps the turn alive: streaming
    /// deltas reset the idle clock, so a turn producing output for longer than
    /// the idle window is never killed.
    #[test]
    fn drain_streaming_deltas_reset_the_idle_clock() {
        let clock = FakeClock::default();
        // Each line arrives just under the idle budget; total elapsed (5 * 90s
        // = 450s) exceeds the 100s idle window many times over, yet — because
        // every event resets the window — the turn completes, never idles out.
        let mut steps: std::collections::VecDeque<(u64, Option<ChildEvent>)> = (0..5)
            .map(|i| (90_000u64, Some(ChildEvent::Stdout(format!("chunk {i}")))))
            .collect();
        steps.push_back((10_000, Some(ChildEvent::StdoutEof)));
        let mut child = FakeChild {
            clock: clock.clone(),
            steps,
        };

        let mut deltas = 0u32;
        let outcome =
            drain_with_liveness(&mut child, &clock, Duration::from_secs(100), &mut |_| {
                deltas += 1
            });
        assert!(
            matches!(outcome, DrainOutcome::Completed { .. }),
            "continued output must keep resetting the idle clock (no kill)"
        );
        assert_eq!(deltas, 5, "each output should have streamed a delta");
    }

    /// A stderr heartbeat (no stdout content) also proves liveness and resets
    /// the idle window — a child logging progress on stderr is not "idle".
    #[test]
    fn drain_stderr_heartbeat_keeps_turn_alive() {
        let clock = FakeClock::default();
        let mut steps = std::collections::VecDeque::new();
        for _ in 0..4 {
            steps.push_back((90_000, Some(ChildEvent::Stderr("progress...".to_string()))));
        }
        steps.push_back((1_000, Some(ChildEvent::StdoutEof)));
        let mut child = FakeChild {
            clock: clock.clone(),
            steps,
        };

        let mut deltas = 0u32;
        let outcome =
            drain_with_liveness(&mut child, &clock, Duration::from_secs(100), &mut |_| {
                deltas += 1
            });
        assert!(
            matches!(outcome, DrainOutcome::Completed { .. }),
            "stderr heartbeats must keep the turn alive"
        );
        assert_eq!(
            deltas, 0,
            "stderr heartbeats are liveness only, not content"
        );
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

    // ── idle-liveness: honest degradation on a wedged turn ──

    #[test]
    fn invoke_agent_degrades_honestly_on_idle_timeout() {
        // Drive `invoke_agent` against a child that never produces output and
        // never exits. `sh -c 'sleep 30'` ignores the trailing `-p <prompt>`
        // args (they become $0/$1), so it stays silent — a genuine hang the
        // idle window must reap.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "sleep 30".to_string()];
        proxy.turn_idle_timeout = Duration::from_millis(500);
        proxy.emit_metrics = false; // never write to ~/.simard from the test suite

        let started = Instant::now();
        let result = proxy.invoke_agent("hello");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "invoke_agent must not block indefinitely on a wedged child (took {elapsed:?})"
        );
        let err = result.expect_err("an idle-timed-out turn must surface an error, not Ok");
        let msg = err.to_string();
        assert!(
            msg.contains("idle-timeout") && msg.contains("honest"),
            "error must be a clear honest idle-timeout degradation, got: {msg}"
        );
    }

    #[test]
    fn invoke_agent_returns_output_when_child_closes_stdout_then_lingers() {
        // Idle semantics: stdout EOF means the turn's output is complete. A
        // child that prints, closes stdout (`exec 1>&-`), then lingers must
        // return its output PROMPTLY — never wait on the lingering process — and
        // the linger must be reaped (no orphan).
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "printf 'partial-answer\\n'; exec 1>&-; sleep 30".to_string(),
        ];
        proxy.turn_idle_timeout = Duration::from_secs(30);
        proxy.emit_metrics = false;

        let started = Instant::now();
        let result = proxy.invoke_agent("hello");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "EOF means output complete — must not block on the lingering child (took {elapsed:?})"
        );
        assert_eq!(
            result.expect("output produced before stdout close must return Ok"),
            "partial-answer",
            "the produced output must be returned as soon as stdout closes"
        );
    }

    #[test]
    fn invoke_agent_idle_timeout_reaps_descendant_processes() {
        // A wedged turn must kill the WHOLE agent subtree, not just the direct
        // child — otherwise a descendant holding the stdout pipe leaks (and the
        // reader thread blocks forever). We spawn a shell that backgrounds a
        // grandchild `sleep` and records its PID, then wait (no output → idle).
        // After the group-kill the grandchild must be gone. Without the
        // process-group kill (only `child.kill()`), it would survive its 30s.
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
        // before the idle window fires.
        proxy.agent_base_args = vec![
            "-c".to_string(),
            format!("sleep 30 & echo $! > '{pidpath}'; wait"),
        ];
        // 3s idle window: comfortably longer than the shell needs to record the
        // PID, far shorter than the grandchild's 30s sleep, so it is guaranteed
        // alive when the group-kill fires.
        proxy.turn_idle_timeout = Duration::from_secs(3);
        proxy.emit_metrics = false;

        let result = proxy.invoke_agent("hello");
        assert!(
            result.is_err(),
            "wedged turn must surface an idle-timeout error"
        );

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
            "idle timeout must reap the agent's descendant (pid {pid}); it survived the group-kill"
        );
    }

    // ── thin-proxy invariant (issue #2179): no PTY, piped stdio ──

    #[test]
    fn invoke_agent_uses_piped_stdio_not_a_pty() {
        // The thin proxy spawns the agent with a null stdin and a piped stdout —
        // never a PTY/script(1)/bash wrapper (issue #2179, replacing the 30-90s
        // PTY overhead). A child that inspects its own descriptors must therefore
        // observe NON-tty stdin and stdout; if a PTY leaked back in, `[ -t 0 ]` /
        // `[ -t 1 ]` would report a terminal and this assertion would fail.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec![
            "-c".to_string(),
            "printf 'stdin='; if [ -t 0 ]; then printf 'tty'; else printf 'notty'; fi; \
             printf ' stdout='; if [ -t 1 ]; then printf 'tty'; else printf 'notty'; fi"
                .to_string(),
        ];
        proxy.turn_idle_timeout = Duration::from_secs(30);

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
        proxy.turn_idle_timeout = Duration::from_secs(30);

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
        proxy.turn_idle_timeout = Duration::from_secs(30);

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
}
