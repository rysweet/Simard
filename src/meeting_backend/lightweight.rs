//! Lightweight LLM chat session for meeting conversation turns.
//!
//! Instead of spawning a full Copilot SDK subprocess through PTY infrastructure
//! (which auto-launches nested amplihack sessions adding ~50s overhead per turn),
//! this session runs the copilot command directly via `std::process::Command`
//! with piped stdin/stdout and captured stderr.
//!
//! This eliminates the PTY overhead and prevents stderr from leaking into
//! response content (fixes #568).

use std::io::Write;
use std::process::{Command, Stdio};

use tracing::{debug, info, warn};

use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession,
    BaseTypeTurnInput, capability_set, ensure_session_not_already_open,
    ensure_session_not_closed, ensure_session_open,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

/// Default command for lightweight chat (the copilot binary).
const DEFAULT_CHAT_COMMAND: &str = "amplihack";

/// A lightweight `BaseTypeSession` that calls the LLM directly via subprocess
/// pipes instead of the full PTY terminal infrastructure.
///
/// This avoids ~50s of overhead per turn from nested amplihack session startup
/// and cleanly separates stderr from the response content.
pub struct LightweightChatSession {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
}

impl std::fmt::Debug for LightweightChatSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightweightChatSession")
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .field("turn_count", &self.turn_count)
            .finish()
    }
}

impl LightweightChatSession {
    /// Create a new lightweight chat session.
    pub fn new() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("lightweight-chat"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "lightweight-chat::pipe",
                    "lightweight-chat:direct-subprocess",
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
        })
    }

    /// Execute a chat turn by piping the prompt to the copilot subprocess.
    ///
    /// stderr is captured separately and logged (not mixed into the response).
    fn execute_piped_turn(&self, prompt: &str) -> SimardResult<String> {
        let mut child = Command::new(DEFAULT_CHAT_COMMAND)
            .args(["copilot", "--subprocess-safe"])
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_MAX_DEPTH", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: "lightweight-chat".to_string(),
                reason: format!("failed to spawn copilot subprocess: {e}"),
            })?;

        // Write prompt to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes());
            // stdin dropped here, closing the pipe
        }

        let output = child.wait_with_output().map_err(|e| {
            SimardError::AdapterInvocationFailed {
                base_type: "lightweight-chat".to_string(),
                reason: format!("copilot subprocess failed: {e}"),
            }
        })?;

        // Log stderr separately — never include in response (#568)
        let stderr_text = String::from_utf8_lossy(&output.stderr);
        if !stderr_text.trim().is_empty() {
            debug!(stderr_lines = stderr_text.lines().count(), "Copilot subprocess stderr captured (not included in response)");
        }

        if !output.status.success() {
            warn!(
                exit_code = output.status.code(),
                stderr = %stderr_text.chars().take(500).collect::<String>(),
                "Copilot subprocess exited with non-zero status"
            );
        }

        let stdout_text = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(strip_copilot_noise(&stdout_text))
    }
}

impl BaseTypeSession for LightweightChatSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        self.turn_count += 1;

        // Build the full prompt from context + objective
        let prompt = if input.identity_context.is_empty() && input.prompt_preamble.is_empty() {
            input.objective.clone()
        } else {
            let mut parts = Vec::new();
            if !input.prompt_preamble.is_empty() {
                parts.push(input.prompt_preamble.as_str());
            }
            if !input.identity_context.is_empty() {
                parts.push(input.identity_context.as_str());
            }
            parts.push(&input.objective);
            parts.join("\n\n")
        };

        info!(
            turn = self.turn_count,
            prompt_len = prompt.len(),
            "Lightweight chat: sending turn via piped subprocess"
        );
        let start = std::time::Instant::now();

        let response_text = self.execute_piped_turn(&prompt)?;

        info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            response_len = response_text.len(),
            turn = self.turn_count,
            "Lightweight chat: received response"
        );

        // Record cost estimate
        if let Err(e) = crate::cost_tracking::record_cost(
            "lightweight-chat",
            "copilot-lightweight",
            prompt.len(),
            response_text.len(),
            &format!("lightweight chat turn {}", self.turn_count),
        ) {
            debug!("Cost tracking write failed: {e}");
        }

        Ok(BaseTypeOutcome {
            plan: format!(
                "Lightweight chat turn {} via piped subprocess.",
                self.turn_count
            ),
            execution_summary: response_text,
            evidence: vec![
                format!("lightweight-chat-turn={}", self.turn_count),
                format!(
                    "elapsed-ms={}",
                    start.elapsed().as_millis()
                ),
            ],
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

/// Strip copilot bootstrap noise and usage stats from stdout output.
fn strip_copilot_noise(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut skip_rest = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        // Skip empty leading lines
        if result.is_empty() && trimmed.is_empty() {
            continue;
        }

        // Stop at usage stats footer
        if trimmed.starts_with("Total usage est:")
            || trimmed.starts_with("API time spent:")
            || trimmed.starts_with("Total session time:")
        {
            skip_rest = true;
            continue;
        }

        if skip_rest {
            continue;
        }

        // Skip copilot bootstrap noise
        if trimmed.contains("Staged") && trimmed.contains("hook") {
            continue;
        }
        if trimmed.contains("XPIA") || trimmed.starts_with("Script started on") {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn strip_copilot_noise_handles_empty() {
        assert_eq!(strip_copilot_noise(""), "");
        assert_eq!(strip_copilot_noise("   \n  \n"), "");
    }

    #[test]
    fn new_session_creates_successfully() {
        let session = LightweightChatSession::new();
        assert!(session.is_ok());
    }

    #[test]
    fn session_lifecycle() {
        let mut session = LightweightChatSession::new().unwrap();
        assert!(!session.is_open);
        session.open().unwrap();
        assert!(session.is_open);
        session.close().unwrap();
        assert!(session.is_closed);
    }

    #[test]
    fn double_open_fails() {
        let mut session = LightweightChatSession::new().unwrap();
        session.open().unwrap();
        assert!(session.open().is_err());
    }

    #[test]
    fn run_turn_before_open_fails() {
        let mut session = LightweightChatSession::new().unwrap();
        let input = BaseTypeTurnInput::objective_only("hello");
        assert!(session.run_turn(input).is_err());
    }
}
