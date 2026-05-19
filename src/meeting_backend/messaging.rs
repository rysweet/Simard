//! Send-message turn handling: prompt construction + LLM dispatch.

use tracing::{debug, error, info, warn};

use crate::base_types::BaseTypeTurnInput;
use crate::error::{SimardError, SimardResult};

use super::MeetingBackend;
use super::sanitize::extract_response;
use super::types::{MeetingResponse, Role};

impl MeetingBackend {
    /// Send a user message and get Simard's response.
    ///
    /// Appends both the user message and the assistant response to history.
    /// The full conversation context is sent to the LLM on each turn.
    #[tracing::instrument(skip(self), fields(input_len = user_input.len()))]
    pub fn send_message(&mut self, user_input: &str) -> SimardResult<MeetingResponse> {
        if !self.is_open {
            return Err(SimardError::ActionExecutionFailed {
                action: "send-message".to_string(),
                reason: "meeting session is closed".to_string(),
            });
        }

        let trimmed = user_input.trim();
        if trimmed.is_empty() {
            return Ok(MeetingResponse {
                content: String::new(),
                message_count: self.history.len(),
            });
        }

        // Append user message
        self.push_message(Role::User, trimmed.to_string());

        // Build the prompt preamble from conversation history
        let preamble = self.build_conversation_preamble();

        let turn_input = BaseTypeTurnInput {
            objective: trimmed.to_string(),
            identity_context: self.system_prompt.clone(),
            prompt_preamble: preamble,
        };

        info!(
            topic = self.topic,
            messages = self.history.len(),
            input_len = trimmed.len(),
            "Sending message to LLM agent…"
        );
        let start = std::time::Instant::now();

        // Agent is normally `Some`; it is only `None` if a previous close
        // pipeline abandoned it to a detached worker on timeout (issue
        // #1908). `send_message` must therefore fail loud rather than
        // silently no-op so the operator notices the meeting is
        // unusable. The REPL exits on `/close` immediately, so reaching
        // this branch in practice would mean a buggy caller invoked
        // `send_message` after a partial close.
        let agent = self
            .agent
            .as_mut()
            .ok_or_else(|| SimardError::ActionExecutionFailed {
                action: "send-message".to_string(),
                reason: "meeting agent is no longer available (close pipeline took it)".to_string(),
            })?;

        let outcome = match agent.run_turn(turn_input) {
            Ok(o) => {
                info!(
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    response_len = o.execution_summary.len(),
                    "LLM agent returned response"
                );
                o
            }
            Err(e) => {
                warn!(elapsed_ms = start.elapsed().as_millis() as u64, error = %e, "LLM agent returned error");
                return Err(e);
            }
        };
        let extracted = extract_response(&outcome);
        if extracted.trim().is_empty() {
            // Fail loud on empty adapter output rather than substituting a
            // sentinel string that downstream code (transcript writers,
            // /act-on-decisions, dashboard chat) treats as legitimate
            // assistant content. See #1671.
            error!(
                raw_len = outcome.execution_summary.len(),
                topic = self.topic,
                "MeetingBackend: adapter returned empty response — failing the turn"
            );
            return Err(SimardError::ActionExecutionFailed {
                action: "send-message".to_string(),
                reason: format!(
                    "empty_adapter_response: extract_response produced empty result \
                     (raw_summary_len={})",
                    outcome.execution_summary.len()
                ),
            });
        }
        let response_text = extracted;

        // Append assistant response
        self.push_message(Role::Assistant, response_text.clone());

        // Auto-save transcript after every turn so killed meetings don't lose data
        self.auto_save_transcript();

        debug!(messages = self.history.len(), "Meeting turn completed");

        Ok(MeetingResponse {
            content: response_text,
            message_count: self.history.len(),
        })
    }
}
