//! Send-message turn handling: prompt construction + LLM dispatch.

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::base_types::{BaseTypeOutcome, BaseTypeTurnInput};
use crate::error::{SimardError, SimardResult};

use super::sanitize::extract_response;
use super::types::{MeetingResponse, Role};
use super::{EMPTY_RESPONSE_SENTINEL, MAX_HISTORY, MeetingBackend};

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

        let outcome = match self.agent.run_turn(turn_input) {
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
        let response_text = {
            let extracted = extract_response(&outcome);
            if extracted.trim().is_empty() {
                EMPTY_RESPONSE_SENTINEL.to_string()
            } else {
                extracted
            }
        };

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
