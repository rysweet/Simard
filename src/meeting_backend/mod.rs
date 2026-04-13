//! Unified meeting backend — one conversational engine, two thin frontends.
//!
//! `MeetingBackend` owns the conversation history, system prompt, LLM
//! delegation (via `BaseTypeSession::run_turn()`), and persistence. The CLI
//! REPL and dashboard WebSocket are thin adapters around this struct.

pub mod command;
pub mod persist;
pub mod templates;
pub mod types;

use std::fmt::Write as _;
use std::time::Instant;

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::base_types::{BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

pub use command::{MeetingCommand, parse_command};
pub use templates::{find_template, format_template, list_templates};
pub use types::{
    ConversationMessage, MeetingProgress, MeetingResponse, MeetingSummary, MeetingTemplateKind,
    MeetingTranscript, Role, SessionStatus,
};

// Free functions for dashboard/external consumers.
pub use persist::write_markdown_export as export_transcript_markdown_inner;

/// Format the list of available templates as a user-facing string.
pub fn format_template_list() -> String {
    list_templates()
}

/// Export a `MeetingTranscript` as formatted markdown. Convenience wrapper
/// around `persist::write_markdown_export` for callers that have a transcript.
pub fn export_transcript_markdown(
    transcript: &MeetingTranscript,
) -> SimardResult<std::path::PathBuf> {
    persist::write_markdown_export(
        &transcript.topic,
        &transcript.started_at,
        transcript.duration_secs,
        &transcript.messages,
    )
}

/// Maximum messages kept in conversation history.
const MAX_HISTORY: usize = 500;

/// Number of recent messages included verbatim in the LLM prompt.
const RECENT_WINDOW: usize = 30;

/// The unified meeting backend.
///
/// Maintains conversation state, delegates to an LLM agent, and handles
/// persistence. All methods are synchronous (matching `BaseTypeSession`).
pub struct MeetingBackend {
    topic: String,
    history: Vec<ConversationMessage>,
    system_prompt: String,
    agent: Box<dyn BaseTypeSession>,
    bridge: Option<Box<dyn CognitiveMemoryOps>>,
    started_at: String,
    started_instant: Instant,
    is_open: bool,
    /// Active meeting template, if one was applied via `/template`.
    template: Option<MeetingTemplateKind>,
}

impl MeetingBackend {
    /// Create a new meeting session.
    ///
    /// - `topic`: the meeting subject (used in prompts and persistence).
    /// - `agent`: an opened `BaseTypeSession` for LLM calls.
    /// - `bridge`: optional cognitive memory bridge for enrichment and storage.
    /// - `system_prompt`: pre-built system prompt (identity + live context).
    pub fn new_session(
        topic: &str,
        agent: Box<dyn BaseTypeSession>,
        bridge: Option<Box<dyn CognitiveMemoryOps>>,
        system_prompt: String,
    ) -> Self {
        let started_at = Utc::now().to_rfc3339();
        info!(topic, "Meeting session created");
        Self {
            topic: topic.to_string(),
            history: Vec::new(),
            system_prompt,
            agent,
            bridge,
            started_at,
            started_instant: Instant::now(),
            is_open: true,
            template: None,
        }
    }

    /// Send a user message and get Simard's response.
    ///
    /// Appends both the user message and the assistant response to history.
    /// The full conversation context is sent to the LLM on each turn.
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
        let response_text = extract_response(&outcome);

        // Append assistant response
        self.push_message(Role::Assistant, response_text.clone());

        debug!(messages = self.history.len(), "Meeting turn completed");

        Ok(MeetingResponse {
            content: response_text,
            message_count: self.history.len(),
        })
    }

    /// Close the meeting session: summarize, persist, and store to memory.
    ///
    /// Returns a `MeetingSummary` with the LLM-generated summary text.
    pub fn close(&mut self) -> SimardResult<MeetingSummary> {
        if !self.is_open {
            return Err(SimardError::ActionExecutionFailed {
                action: "close-meeting".to_string(),
                reason: "meeting session is already closed".to_string(),
            });
        }

        self.is_open = false;
        let duration_secs = self.elapsed_secs();

        // Generate summary via LLM (internal prompt, not visible to operator)
        let summary_text = self.generate_summary();

        // Write JSON transcript to ~/.simard/meetings/
        let transcript = MeetingTranscript {
            topic: self.topic.clone(),
            started_at: self.started_at.clone(),
            closed_at: Utc::now().to_rfc3339(),
            duration_secs,
            summary: summary_text.clone(),
            messages: self.history.clone(),
        };
        let transcript_path = match persist::write_transcript(&transcript) {
            Ok(p) => Some(p.to_string_lossy().to_string()),
            Err(e) => {
                warn!("Failed to write transcript: {e}");
                None
            }
        };

        // Write MeetingHandoff artifact for OODA integration
        if let Err(e) = persist::write_handoff(&self.topic, &summary_text, &self.history) {
            warn!("Failed to write meeting handoff: {e}");
        }

        // Store to cognitive memory via bridge
        if let Some(ref bridge) = self.bridge {
            persist::store_cognitive_memory(&**bridge, &self.topic, &summary_text, &self.history);
        }

        // Close the agent session
        if let Err(e) = self.agent.close() {
            warn!("Failed to close agent session: {e}");
        }

        info!(
            topic = self.topic,
            messages = self.history.len(),
            duration_secs,
            "Meeting session closed"
        );

        Ok(MeetingSummary {
            topic: self.topic.clone(),
            summary_text,
            message_count: self.history.len(),
            duration_secs,
            transcript_path,
        })
    }

    /// Get current session status.
    pub fn status(&self) -> SessionStatus {
        SessionStatus {
            topic: self.topic.clone(),
            message_count: self.history.len(),
            started_at: self.started_at.clone(),
            is_open: self.is_open,
            duration_display: Some(self.elapsed_display()),
            active_template: self.template.as_ref().map(|t| t.agenda().to_string()),
        }
    }

    /// Convenience: get the topic.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Convenience: check if the session is still open.
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// Generate a progress report from the current conversation state.
    pub fn progress(&self) -> MeetingProgress {
        let operator_messages = self.history.iter().filter(|m| m.role == Role::User).count();
        let agent_messages = self
            .history
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count();

        let topics = extract_topics(&self.history);
        let action_items = extract_patterns(&self.history, ACTION_ITEM_MARKERS);
        let pending_decisions = extract_patterns(&self.history, DECISION_MARKERS);

        MeetingProgress {
            duration_display: self.elapsed_display(),
            operator_messages,
            agent_messages,
            topics,
            action_items,
            pending_decisions,
        }
    }

    /// Message count in the conversation history.
    pub fn message_count(&self) -> usize {
        self.history.len()
    }

    /// Elapsed time since meeting start, as a human-readable string (e.g. "5m 32s").
    pub fn elapsed_display(&self) -> String {
        let secs = self.elapsed_secs();
        if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m {}s", secs / 60, secs % 60)
        }
    }

    /// Export the transcript as formatted markdown to the current directory.
    ///
    /// Filename format: `meeting-YYYY-MM-DD-HHMMSS.md`.
    pub fn export_markdown(&self) -> SimardResult<std::path::PathBuf> {
        persist::write_markdown_export(
            &self.topic,
            &self.started_at,
            self.elapsed_secs(),
            &self.history,
        )
    }

    /// Apply a meeting template: send its opening prompt to the LLM agent.
    pub fn apply_template(&mut self, template: &templates::MeetingTemplate) -> SimardResult<()> {
        self.template = MeetingTemplateKind::from_name(template.name);
        self.send_message(template.opening_prompt)?;
        Ok(())
    }

    /// Set the active template kind directly.
    pub fn set_template(&mut self, kind: MeetingTemplateKind) {
        self.template = Some(kind);
    }

    /// Get the active template, if any.
    pub fn active_template(&self) -> Option<&MeetingTemplateKind> {
        self.template.as_ref()
    }

    /// Build a `MeetingTranscript` snapshot of the current state.
    pub fn transcript_snapshot(&self, summary: &str) -> MeetingTranscript {
        MeetingTranscript {
            topic: self.topic.clone(),
            started_at: self.started_at.clone(),
            closed_at: String::new(),
            duration_secs: self.elapsed_secs(),
            summary: summary.to_string(),
            messages: self.history.clone(),
        }
    }

    // --- Private helpers ---

    fn push_message(&mut self, role: Role, content: String) {
        if self.history.len() >= MAX_HISTORY {
            warn!("Conversation history at cap ({MAX_HISTORY}), dropping oldest message");
            self.history.remove(0);
        }
        self.history.push(ConversationMessage {
            role,
            content,
            timestamp: Utc::now().to_rfc3339(),
        });
    }

    /// Build the prompt preamble that carries conversation context.
    ///
    /// - Last `RECENT_WINDOW` messages are included verbatim.
    /// - Older messages are summarized as a rolling context paragraph.
    fn build_conversation_preamble(&self) -> String {
        let mut preamble = String::with_capacity(4096);
        let _ = writeln!(preamble, "Meeting topic: {}\n", self.topic);

        let total = self.history.len();
        if total == 0 {
            return preamble;
        }

        let recent_start = total.saturating_sub(RECENT_WINDOW);

        // Summarize older messages if any
        if recent_start > 0 {
            let _ = writeln!(
                preamble,
                "[Earlier conversation: {} messages exchanged about {}]\n",
                recent_start, self.topic
            );
        }

        // Include recent messages verbatim
        let _ = writeln!(preamble, "Recent conversation:");
        for msg in &self.history[recent_start..] {
            let role_label = match msg.role {
                Role::User => "Operator",
                Role::Assistant => "Simard",
                Role::System => "System",
            };
            let _ = writeln!(preamble, "{role_label}: {}", msg.content);
        }

        preamble
    }

    /// Ask the LLM for a summary of the conversation.
    fn generate_summary(&mut self) -> String {
        if self.history.is_empty() {
            return "Empty meeting — no messages exchanged.".to_string();
        }

        let summary_prompt = format!(
            "Please provide a concise summary of this meeting about \"{}\". \
             Include key discussion points, any decisions made, action items \
             mentioned, and important takeaways. Be brief but thorough.",
            self.topic
        );

        let preamble = self.build_conversation_preamble();
        let turn_input = BaseTypeTurnInput {
            objective: summary_prompt,
            identity_context: self.system_prompt.clone(),
            prompt_preamble: preamble,
        };

        match self.agent.run_turn(turn_input) {
            Ok(outcome) => {
                let text = extract_response(&outcome);
                if text.is_empty() {
                    warn!("LLM returned empty summary — using metadata summary");
                    self.metadata_summary()
                } else {
                    text
                }
            }
            Err(e) => {
                warn!("LLM summarization failed: {e} — using metadata summary");
                self.metadata_summary()
            }
        }
    }

    /// Metadata-only summary (no LLM involved). Used when the LLM summary
    /// call fails or returns empty — this is NOT a silent fallback, it's the
    /// structural record of what happened.
    fn metadata_summary(&self) -> String {
        let user_count = self.history.iter().filter(|m| m.role == Role::User).count();
        format!(
            "Meeting on \"{}\" — {} messages ({} from operator), duration {}s. [LLM summary unavailable]",
            self.topic,
            self.history.len(),
            user_count,
            self.elapsed_secs()
        )
    }

    fn elapsed_secs(&self) -> u64 {
        self.started_instant.elapsed().as_secs()
    }
}

/// Extract the conversational response text from a `BaseTypeOutcome`.
///
/// The `execution_summary` field is used by all adapters to return the LLM's
/// natural-language response.
fn extract_response(outcome: &BaseTypeOutcome) -> String {
    outcome.execution_summary.trim().to_string()
}

/// Markers that indicate an action item in conversation text.
const ACTION_ITEM_MARKERS: &[&str] = &[
    "action item:",
    "todo:",
    "TODO:",
    "[ ]",
    "will do",
    "need to",
    "should do",
    "assigned to",
    "take ownership",
];

/// Markers that indicate a pending decision in conversation text.
const DECISION_MARKERS: &[&str] = &[
    "decision:",
    "decided:",
    "we need to decide",
    "open question",
    "pending decision",
    "needs decision",
    "to be determined",
    "TBD",
];

/// Extract likely discussion topics from user messages.
///
/// Uses a simple heuristic: user messages that look like topic headers
/// (short, or containing common framing words). Returns up to 10.
fn extract_topics(messages: &[ConversationMessage]) -> Vec<String> {
    let topic_starters = ["let's discuss", "about ", "regarding ", "topic:", "next:"];
    let mut topics = Vec::new();

    for msg in messages.iter().filter(|m| m.role == Role::User) {
        let lower = msg.content.to_ascii_lowercase();
        let is_topic = topic_starters.iter().any(|s| lower.starts_with(s))
            || (msg.content.len() < 80 && !lower.starts_with('/'));
        if is_topic && !msg.content.trim().is_empty() {
            // Truncate long content for display
            let display = if msg.content.len() > 60 {
                format!("{}…", &msg.content[..60])
            } else {
                msg.content.clone()
            };
            topics.push(display);
        }
        if topics.len() >= 10 {
            break;
        }
    }
    topics
}

/// Extract sentences containing any of the given marker phrases.
fn extract_patterns(messages: &[ConversationMessage], markers: &[&str]) -> Vec<String> {
    let mut results = Vec::new();
    for msg in messages {
        let lower = msg.content.to_ascii_lowercase();
        for marker in markers {
            if lower.contains(*marker) {
                // Find the sentence containing the marker
                for sentence in msg.content.split(['.', '\n']) {
                    let s = sentence.trim();
                    if !s.is_empty()
                        && s.to_ascii_lowercase().contains(*marker)
                        && !results.contains(&s.to_string())
                    {
                        results.push(s.to_string());
                    }
                }
            }
        }
        if results.len() >= 20 {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::{
        BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
        ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
        standard_session_capabilities,
    };
    use crate::metadata::{BackendDescriptor, Freshness};
    use crate::runtime::RuntimeTopology;

    /// Mock agent that returns a canned response.
    struct MockAgent {
        descriptor: BaseTypeDescriptor,
        response: String,
        is_open: bool,
        is_closed: bool,
    }

    impl MockAgent {
        fn new(response: &str) -> Self {
            Self {
                descriptor: BaseTypeDescriptor {
                    id: BaseTypeId::new("mock-meeting-backend"),
                    backend: BackendDescriptor::for_runtime_type::<Self>(
                        "mock",
                        "test:mock-meeting-backend",
                        Freshness::now().unwrap(),
                    ),
                    capabilities: standard_session_capabilities(),
                    supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
                },
                response: response.to_string(),
                is_open: true,
                is_closed: false,
            }
        }
    }

    impl BaseTypeSession for MockAgent {
        fn descriptor(&self) -> &BaseTypeDescriptor {
            &self.descriptor
        }
        fn open(&mut self) -> SimardResult<()> {
            ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
            ensure_session_not_already_open(&self.descriptor, self.is_open)?;
            self.is_open = true;
            Ok(())
        }
        fn run_turn(&mut self, _input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
            ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
            ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
            Ok(BaseTypeOutcome {
                plan: String::new(),
                execution_summary: self.response.clone(),
                evidence: Vec::new(),
            })
        }
        fn close(&mut self) -> SimardResult<()> {
            ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
            self.is_closed = true;
            Ok(())
        }
    }

    #[test]
    fn new_session_creates_open_session() {
        let agent = MockAgent::new("hello");
        let backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        assert!(backend.is_open());
        assert_eq!(backend.topic(), "Test");
        let status = backend.status();
        assert_eq!(status.message_count, 0);
        assert!(status.is_open);
    }

    #[test]
    fn send_message_accumulates_history() {
        let agent = MockAgent::new("I understand");
        let mut backend =
            MeetingBackend::new_session("Sprint", Box::new(agent), None, String::new());

        let resp = backend.send_message("Let's discuss the roadmap").unwrap();
        assert_eq!(resp.content, "I understand");
        assert_eq!(resp.message_count, 2); // user + assistant

        let resp2 = backend.send_message("What about testing?").unwrap();
        assert_eq!(resp2.message_count, 4); // 2 more
    }

    #[test]
    fn send_empty_message_returns_empty() {
        let agent = MockAgent::new("response");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        let resp = backend.send_message("   ").unwrap();
        assert!(resp.content.is_empty());
        assert_eq!(resp.message_count, 0);
    }

    #[test]
    fn close_produces_summary() {
        let agent = MockAgent::new("Here is the summary of our meeting.");
        let mut backend =
            MeetingBackend::new_session("Retro", Box::new(agent), None, String::new());

        backend.send_message("How did the sprint go?").unwrap();
        let summary = backend.close().unwrap();

        assert_eq!(summary.topic, "Retro");
        assert!(!summary.summary_text.is_empty());
        assert_eq!(summary.message_count, 2);
        assert!(!backend.is_open());
    }

    #[test]
    fn send_message_after_close_fails() {
        let agent = MockAgent::new("ok");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        backend.close().unwrap();

        let result = backend.send_message("hello");
        assert!(result.is_err());
    }

    #[test]
    fn double_close_fails() {
        let agent = MockAgent::new("ok");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        backend.close().unwrap();
        let result = backend.close();
        assert!(result.is_err());
    }

    #[test]
    fn status_reflects_message_count() {
        let agent = MockAgent::new("noted");
        let mut backend =
            MeetingBackend::new_session("Planning", Box::new(agent), None, String::new());

        assert_eq!(backend.status().message_count, 0);
        backend.send_message("Item 1").unwrap();
        assert_eq!(backend.status().message_count, 2);
        backend.send_message("Item 2").unwrap();
        assert_eq!(backend.status().message_count, 4);
    }

    #[test]
    fn conversation_preamble_includes_topic() {
        let agent = MockAgent::new("ok");
        let backend =
            MeetingBackend::new_session("Sprint Planning", Box::new(agent), None, String::new());
        let preamble = backend.build_conversation_preamble();
        assert!(preamble.contains("Sprint Planning"));
    }

    #[test]
    fn extract_response_trims_whitespace() {
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "  hello world  ".to_string(),
            evidence: Vec::new(),
        };
        assert_eq!(extract_response(&outcome), "hello world");
    }

    #[test]
    fn progress_empty_session() {
        let agent = MockAgent::new("ok");
        let backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        let p = backend.progress();
        assert_eq!(p.operator_messages, 0);
        assert_eq!(p.agent_messages, 0);
        assert!(p.topics.is_empty());
        assert!(p.action_items.is_empty());
        assert!(p.pending_decisions.is_empty());
    }

    #[test]
    fn progress_counts_messages() {
        let agent = MockAgent::new("Action item: fix the tests");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        backend.send_message("Let's discuss testing").unwrap();
        backend.send_message("What about coverage?").unwrap();
        let p = backend.progress();
        assert_eq!(p.operator_messages, 2);
        assert_eq!(p.agent_messages, 2);
    }

    #[test]
    fn progress_extracts_action_items() {
        let agent = MockAgent::new("Action item: deploy the fix by Friday");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        backend.send_message("What needs to happen?").unwrap();
        let p = backend.progress();
        assert!(
            !p.action_items.is_empty(),
            "should extract action items from agent response"
        );
    }

    #[test]
    fn progress_extracts_decisions() {
        let agent = MockAgent::new("We need to decide on the API format");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        backend.send_message("What's pending?").unwrap();
        let p = backend.progress();
        assert!(
            !p.pending_decisions.is_empty(),
            "should extract pending decisions"
        );
    }

    #[test]
    fn progress_extracts_topics() {
        let agent = MockAgent::new("ok");
        let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
        backend.send_message("API design").unwrap();
        backend.send_message("Database schema").unwrap();
        let p = backend.progress();
        assert!(
            p.topics.len() >= 2,
            "should extract short user messages as topics"
        );
    }
}
