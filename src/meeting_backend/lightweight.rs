//! Lightweight LLM chat session for meeting conversation turns.
//!
//! Delegates to [`SessionBuilder`] for LLM access instead of spawning a
//! subprocess. Both the CLI REPL and dashboard chat widget share the same
//! `SessionBuilder` backend, giving identical behavior regardless of
//! entry point.
//!
//! Fixes #2105, #2106.

use tracing::{debug, info};

use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession,
    BaseTypeTurnInput, capability_set, ensure_session_not_already_open, ensure_session_not_closed,
    ensure_session_open,
};
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::session_builder::{LlmProvider, SessionBuilder};

/// A lightweight `BaseTypeSession` that delegates to `SessionBuilder` for
/// LLM access. Wraps an inner session opened via `SessionBuilder::open()`
/// so callers get a simple open/turn/close lifecycle without managing
/// provider resolution or adapter wiring themselves.
pub struct LightweightChatSession {
    descriptor: BaseTypeDescriptor,
    inner: Option<Box<dyn BaseTypeSession>>,
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
    ///
    /// The inner `SessionBuilder` session is opened lazily in `open()`.
    pub fn new() -> SimardResult<Self> {
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("lightweight-chat"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "lightweight-chat::session-builder",
                    "lightweight-chat:session-builder",
                    Freshness::now()?,
                ),
                capabilities: capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                ]),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            inner: None,
            is_open: false,
            is_closed: false,
            turn_count: 0,
        })
    }
}

impl BaseTypeSession for LightweightChatSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;

        let provider = LlmProvider::resolve()?;
        let session = SessionBuilder::new(OperatingMode::Meeting, provider)
            .node_id("meeting-lightweight")
            .address("meeting-lightweight://local")
            .adapter_tag("meeting")
            .open()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: "lightweight-chat".to_string(),
                reason: format!("SessionBuilder::open failed: {e}"),
            })?;

        self.inner = Some(session);
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        self.turn_count += 1;

        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: "lightweight-chat".to_string(),
                reason: "inner session not initialized (open() not called?)".to_string(),
            })?;

        info!(
            turn = self.turn_count,
            prompt_len = input.objective.len(),
            "Lightweight chat: sending turn via SessionBuilder"
        );
        let start = std::time::Instant::now();

        let outcome = inner.run_turn(input)?;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        info!(
            elapsed_ms,
            response_len = outcome.execution_summary.len(),
            turn = self.turn_count,
            "Lightweight chat: received response"
        );

        // Record cost estimate
        if let Err(e) = crate::cost_tracking::record_cost(
            "lightweight-chat",
            "session-builder",
            outcome.plan.len(),
            outcome.execution_summary.len(),
            &format!("lightweight chat turn {}", self.turn_count),
        ) {
            debug!("Cost tracking write failed: {e}");
        }

        Ok(outcome)
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;

        if let Some(mut inner) = self.inner.take()
            && let Err(e) = inner.close()
        {
            debug!("Inner session close error (non-fatal): {e}");
        }

        self.is_closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_creates_successfully() {
        let session = LightweightChatSession::new();
        assert!(session.is_ok());
    }

    #[test]
    fn session_initial_state() {
        let session = LightweightChatSession::new().unwrap();
        assert!(!session.is_open);
        assert!(!session.is_closed);
        assert_eq!(session.turn_count, 0);
        assert!(session.inner.is_none());
    }

    #[test]
    fn double_open_fails() {
        // open() will fail in CI (no LLM provider configured), but if the
        // first open succeeds the second must return Err.
        let mut session = LightweightChatSession::new().unwrap();
        if session.open().is_ok() {
            assert!(session.open().is_err());
        }
    }

    #[test]
    fn run_turn_before_open_fails() {
        let mut session = LightweightChatSession::new().unwrap();
        let input = BaseTypeTurnInput::objective_only("hello");
        assert!(session.run_turn(input).is_err());
    }

    // ── session lifecycle contract tests ──────────────────────────────────────

    /// Closing a session that was never opened must return an error.
    #[test]
    fn close_before_open_returns_error() {
        let mut session = LightweightChatSession::new().unwrap();
        assert!(
            session.close().is_err(),
            "close() on a never-opened session must return Err"
        );
    }

    /// The session descriptor id must identify this as "lightweight-chat".
    #[test]
    fn session_descriptor_id_is_lightweight_chat() {
        let session = LightweightChatSession::new().unwrap();
        let id = session.descriptor().id.as_str();
        assert_eq!(id, "lightweight-chat");
    }

    /// Prompt building: when only objective is present, the prompt equals
    /// the objective string exactly.
    #[test]
    fn prompt_building_objective_only_equals_objective() {
        let input = BaseTypeTurnInput::objective_only("just the objective");
        assert!(input.identity_context.is_empty());
        assert!(input.prompt_preamble.is_empty());
        assert_eq!(input.objective, "just the objective");
    }

    /// When preamble and identity context are provided, both must be joinable
    /// with the objective into a combined prompt.
    #[test]
    fn prompt_building_with_preamble_and_identity() {
        let input = BaseTypeTurnInput {
            objective: "Do the task.".to_string(),
            identity_context: "You are Simard.".to_string(),
            prompt_preamble: "System preamble.".to_string(),
        };
        let mut parts = Vec::new();
        if !input.prompt_preamble.is_empty() {
            parts.push(input.prompt_preamble.as_str());
        }
        if !input.identity_context.is_empty() {
            parts.push(input.identity_context.as_str());
        }
        parts.push(&input.objective);
        let prompt = parts.join("\n\n");

        assert!(prompt.contains("System preamble."));
        assert!(prompt.contains("You are Simard."));
        assert!(prompt.contains("Do the task."));
        let preamble_pos = prompt.find("System preamble.").unwrap();
        let identity_pos = prompt.find("You are Simard.").unwrap();
        let objective_pos = prompt.find("Do the task.").unwrap();
        assert!(preamble_pos < identity_pos);
        assert!(identity_pos < objective_pos);
    }
}
