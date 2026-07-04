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
        if trimmed.contains("Staged") && trimmed.contains("hook") {
            continue;
        }
        if trimmed.contains("XPIA") || trimmed.starts_with("Script started on") {
            continue;
        }
        if trimmed.starts_with("Warning:") {
            continue;
        }
        if trimmed.len() <= 2 && !trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('●') {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

/// Env var override for turn timeout in seconds. When set to a positive value
/// it overrides [`DEFAULT_TURN_TIMEOUT_SECS`]; when explicitly set to `0` the
/// per-turn timeout is disabled (unbounded — operator escape hatch); when unset
/// or malformed the bounded default applies.
const TURN_TIMEOUT_ENV: &str = "SIMARD_MEETING_TURN_TIMEOUT_SECS";

/// Default per-turn timeout. A hung `copilot -p` child must not block the
/// meeting REPL indefinitely — after this bound the child is killed and the
/// turn degrades honestly via the `[meeting:error]`/`[meeting] WARNING` banner
/// (Pillar 11: honest degradation beats hidden silence). Issue #2549.
const DEFAULT_TURN_TIMEOUT_SECS: u64 = 120;

/// Env var giving an explicit directory the meeting agent should operate in.
/// When set to an existing directory it wins over cwd-derived resolution. This
/// is the "explicit config" seam referenced by issue #2549; there is no
/// per-operator absolute path baked into the binary.
const WORKDIR_ENV: &str = "SIMARD_MEETING_AGENT_DIR";

/// Resolve the per-turn timeout from the environment, falling back to the
/// bounded default. Issue #2549.
///
/// - `SIMARD_MEETING_TURN_TIMEOUT_SECS=<n>` with `n > 0` → `Some(n secs)`
/// - `SIMARD_MEETING_TURN_TIMEOUT_SECS=0` → `None` (explicitly disabled)
/// - unset or malformed → `Some(DEFAULT_TURN_TIMEOUT_SECS)`
fn resolve_turn_timeout() -> Option<Duration> {
    parse_turn_timeout(std::env::var(TURN_TIMEOUT_ENV).ok().as_deref())
}

/// Pure (env-free) core of [`resolve_turn_timeout`] so the fallback/override
/// semantics are testable without mutating process-global environment state.
fn parse_turn_timeout(raw: Option<&str>) -> Option<Duration> {
    match raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => Some(Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)),
        },
        None => Some(Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)),
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
    turn_timeout: Option<Duration>,
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
        let turn_timeout = resolve_turn_timeout();

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
            turn_timeout,
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

    /// Invoke the agent with a prompt and return the response.
    fn invoke_agent(&self, prompt: &str) -> SimardResult<String> {
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

        // Run the agent as the leader of its own process group so a per-turn
        // timeout can kill the WHOLE subtree, not just the direct child. The
        // agent CLIs (`copilot`/`claude`) are Node processes that spawn
        // descendants which inherit the stdout/stderr pipe write-ends; killing
        // only the direct child would leave those descendants alive, holding
        // the pipes open so the reader threads never see EOF (leaked threads +
        // FDs + orphaned processes). Because the timeout is now default-on
        // (issue #2549) this kill path is regularly exercised. `process_group(0)`
        // makes the child's PGID equal its PID; the timeout handler then
        // signals the negated PGID.
        //
        // Tradeoff: the child no longer shares Simard's foreground process
        // group, so a terminal SIGINT (Ctrl-C) sent to Simard mid-turn no
        // longer reaches the agent. That is acceptable here — the agent is a
        // one-shot `-p` invocation that self-terminates, and the bounded
        // per-turn timeout still reaps a genuinely hung subtree.
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

        // Read stdout in a thread with timeout
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

        // Collect stdout lines. The channel disconnects EXACTLY when the reader
        // thread reaches stdout EOF (every writer has closed the pipe) — that is
        // the authoritative "all output received" signal. Draining via the
        // blocking `recv_timeout` until `Disconnected` (rather than `try_wait` +
        // a non-blocking `try_recv` drain) guarantees we never drop a burst the
        // child flushed just before exiting — critical because `copilot`/`claude`
        // write to a pipe (block-buffered) and tend to flush a large final chunk.
        //
        // The loop stays deadline-centric: it NEVER blocks on `child.wait()`
        // unbounded, so a child that closes stdout then hangs (`exec 1>&-; sleep
        // 999`), or one that never closes stdout, still degrades via the per-turn
        // timeout (issue #2549).
        let mut lines: Vec<String> = Vec::new();
        let mut timed_out = false;
        let mut stdout_eof = false;
        loop {
            if let Some(timeout) = self.turn_timeout
                && start.elapsed() >= timeout
            {
                warn!(
                    timeout_secs = timeout.as_secs(),
                    "Agent turn timeout reached, killing process"
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
                timed_out = true;
                break;
            }

            if stdout_eof {
                // All output has been received (reader reached EOF). Finish once
                // the child is reaped; if it closed stdout yet keeps running,
                // keep polling so the deadline above can still fire.
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ => std::thread::sleep(Duration::from_millis(50)),
                }
                continue;
            }

            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(line) => lines.push(line),
                // No line this tick — loop to re-check the deadline.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                // Reader hit stdout EOF and forwarded every line before dropping
                // its sender: `lines` is now complete. The child may still be
                // running (it closed stdout early), so switch to bounded
                // exit/deadline polling above.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => stdout_eof = true,
            }
        }

        // Honest degradation (Pillar 11, issue #2549): a hung child that hit
        // the per-turn timeout surfaces as a specific error rather than
        // blocking the REPL or masquerading as an empty response. The REPL's
        // `render_backend_error` turns this into the `[meeting:error] WARNING`
        // banner with a "retry or /close" hint.
        if timed_out {
            let secs = self
                .turn_timeout
                .map(|d| d.as_secs())
                .unwrap_or(DEFAULT_TURN_TIMEOUT_SECS);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: "persistent-agent-proxy".to_string(),
                reason: format!(
                    "agent turn exceeded {secs}s per-turn timeout \
                     ({TURN_TIMEOUT_ENV}); terminated hung child — honest \
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

        let response_text = self.invoke_agent(&prompt)?;

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

    // ── issue #2549: default per-turn timeout ──

    #[test]
    fn parse_turn_timeout_unset_uses_bounded_default() {
        assert_eq!(
            parse_turn_timeout(None),
            Some(Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)),
            "unset env must yield the bounded default, not None (no hang)"
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
            "0 is the explicit operator escape hatch (unbounded)"
        );
    }

    #[test]
    fn parse_turn_timeout_malformed_falls_back_to_default() {
        assert_eq!(
            parse_turn_timeout(Some("not-a-number")),
            Some(Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)),
            "malformed value must degrade to the bounded default, never None"
        );
    }

    #[test]
    fn new_defaults_to_bounded_turn_timeout_when_env_unset() {
        // Guard against the reproduction in #2549: with the env unset the
        // proxy must carry a bounded timeout so a hung child cannot block the
        // REPL forever. Only assert the default when the operator has NOT set
        // an override in this environment.
        if std::env::var(TURN_TIMEOUT_ENV).is_err() {
            let proxy = PersistentAgentProxy::new().unwrap();
            assert_eq!(
                proxy.turn_timeout,
                Some(Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)),
                "new() must default to a bounded per-turn timeout"
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

    // ── issue #2549: honest degradation on a hung turn ──

    #[test]
    fn invoke_agent_degrades_honestly_on_timeout() {
        // Drive `invoke_agent` against a child that never produces output and
        // never exits within the bound. `sh -c 'sleep 30'` ignores the trailing
        // `-p <prompt>` args (they become $0/$1), so it hangs deterministically.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "sleep 30".to_string()];
        proxy.turn_timeout = Some(Duration::from_millis(500));

        let started = Instant::now();
        let result = proxy.invoke_agent("hello");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "invoke_agent must not block indefinitely on a hung child (took {elapsed:?})"
        );
        let err = result.expect_err("a timed-out turn must surface an error, not Ok");
        let msg = err.to_string();
        assert!(
            msg.contains("timeout") && msg.contains("honest"),
            "error must be a clear honest-degradation timeout, got: {msg}"
        );
    }

    #[test]
    fn invoke_agent_degrades_when_child_closes_stdout_then_hangs() {
        // Regression for the disconnected-branch hang (issue #2549 review): a
        // child that closes its stdout but keeps running must STILL hit the
        // per-turn timeout, not block the REPL forever. `exec 1>&-` closes the
        // stdout write-end (the reader thread sees EOF immediately) and then the
        // shell sleeps — the deadline-centric loop must reap it via timeout.
        let mut proxy = PersistentAgentProxy::new().unwrap();
        proxy.agent_cmd = "sh".to_string();
        proxy.agent_base_args = vec!["-c".to_string(), "exec 1>&-; sleep 30".to_string()];
        proxy.turn_timeout = Some(Duration::from_secs(2));

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
        proxy.turn_timeout = Some(Duration::from_secs(3));

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
        proxy.turn_timeout = Some(Duration::from_secs(30));

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
        proxy.turn_timeout = Some(Duration::from_secs(30));

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
        proxy.turn_timeout = Some(Duration::from_secs(30));

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
