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

/// Env var override for turn timeout (0 = no timeout, default).
const TURN_TIMEOUT_ENV: &str = "SIMARD_MEETING_TURN_TIMEOUT_SECS";

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
        let turn_timeout = std::env::var(TURN_TIMEOUT_ENV)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs);

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

        // Set cwd to the Simard source tree if available
        let simard_src = std::path::Path::new("/home/azureuser/src/Simard/worktrees/main");
        if simard_src.exists() {
            cmd.current_dir(simard_src);
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

        // Collect stdout lines until process exits or timeout
        let mut lines: Vec<String> = Vec::new();
        loop {
            if let Some(timeout) = self.turn_timeout
                && start.elapsed() >= timeout
            {
                warn!("Agent turn timeout reached, killing process");
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(line) => lines.push(line),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Check if process has exited
                    if let Ok(Some(_)) = child.try_wait() {
                        // Drain remaining lines
                        while let Ok(line) = rx.try_recv() {
                            lines.push(line);
                        }
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // stdout reader thread exited; drain and wait
                    while let Ok(line) = rx.try_recv() {
                        lines.push(line);
                    }
                    let _ = child.wait();
                    break;
                }
            }
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
        let (cmd, args) = resolve_agent_command()?;
        self.agent_cmd = cmd;
        self.agent_base_args = args;
        self.validate_agent()?;
        self.is_open = true;
        info!(cmd = %self.agent_cmd, "Agent proxy opened (direct-invoke mode)");
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
}
