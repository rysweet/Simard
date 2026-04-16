//! Unified meeting backend — one conversational engine, two thin frontends.
//!
//! `MeetingBackend` owns the conversation history, system prompt, LLM
//! delegation (via `BaseTypeSession::run_turn()`), and persistence. The CLI
//! REPL and dashboard WebSocket are thin adapters around this struct.

pub mod command;
pub mod lightweight;
pub mod persist;
pub mod types;

use std::fmt::Write as _;
use std::sync::mpsc;
use std::time::Duration;

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::base_types::{BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

pub use command::{MeetingCommand, parse_command};
pub use types::{
    ConversationMessage, HandoffActionItem, MeetingResponse, MeetingSummary, MeetingTranscript,
    Role, SessionStatus,
};

/// Maximum messages kept in conversation history.
const MAX_HISTORY: usize = 500;

/// Number of recent messages included verbatim in the LLM prompt.
const RECENT_WINDOW: usize = 30;

/// Maximum time to wait for LLM summary generation before falling back.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

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
    is_open: bool,
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
        mut agent: Box<dyn BaseTypeSession>,
        bridge: Option<Box<dyn CognitiveMemoryOps>>,
        system_prompt: String,
    ) -> Self {
        let started_at = Utc::now().to_rfc3339();
        if let Err(e) = agent.open() {
            tracing::warn!(%e, "failed to open agent session during meeting creation");
        }
        info!(topic, "Meeting session created");
        Self {
            topic: topic.to_string(),
            history: Vec::new(),
            system_prompt,
            agent,
            bridge,
            started_at,
            is_open: true,
        }
    }

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
        let response_text = extract_response(&outcome);

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

    /// Close the meeting session: summarize, extract action items, link goals,
    /// auto-export markdown report, persist, and store to memory.
    ///
    /// Returns a `MeetingSummary` with the LLM-generated summary text and
    /// structured action items.
    #[tracing::instrument(skip(self))]
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

        // ── Structured action-item extraction ──
        let mut action_items = persist::extract_action_items(&self.history);

        // ── Goal linkage ──
        let goal_titles = self.load_active_goal_titles();
        if !goal_titles.is_empty() {
            persist::link_action_items_to_goals(&mut action_items, &goal_titles);
        }

        // ── Decision extraction ──
        let decisions = persist::extract_decisions(&self.history);

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
        if let Err(e) = persist::write_handoff(
            &self.topic,
            &summary_text,
            &self.history,
            &action_items,
            &decisions,
        ) {
            warn!("Failed to write meeting handoff: {e}");
        }

        // ── Extract open questions and themes for the summary ──
        let open_questions: Vec<String> = persist::extract_open_questions(&self.history)
            .into_iter()
            .map(|q| q.text)
            .collect();
        let themes = persist::extract_themes(&self.history);

        // Collect unique participants from messages and action item assignees.
        let mut participants: Vec<String> = Vec::new();
        for msg in &self.history {
            let role_name = match msg.role {
                Role::User => "operator",
                Role::Assistant => "simard",
                Role::System => "system",
            };
            let s = role_name.to_string();
            if !participants.contains(&s) {
                participants.push(s);
            }
        }
        for a in &action_items {
            if let Some(ref assignee) = a.assignee
                && !participants.contains(assignee)
            {
                participants.push(assignee.clone());
            }
        }

        // ── Auto-export markdown report on /end ──
        let markdown_report_path = match persist::write_handoff_markdown_report(
            &self.topic,
            &self.started_at,
            &summary_text,
            &self.history,
            &action_items,
            &decisions,
        ) {
            Ok(p) => Some(p.to_string_lossy().to_string()),
            Err(e) => {
                warn!("Failed to write handoff markdown report: {e}");
                None
            }
        };

        // ── Memory consolidation ──
        if let Some(ref bridge) = self.bridge {
            persist::store_enriched_cognitive_memory(
                &**bridge,
                &self.topic,
                &summary_text,
                &self.history,
                &action_items,
                &decisions,
            );
        }

        // Close the agent session
        if let Err(e) = self.agent.close() {
            warn!("Failed to close agent session: {e}");
        }

        info!(
            topic = self.topic,
            messages = self.history.len(),
            action_items = action_items.len(),
            decisions = decisions.len(),
            duration_secs,
            "Meeting session closed with structured handoff"
        );

        Ok(MeetingSummary {
            topic: self.topic.clone(),
            summary_text,
            message_count: self.history.len(),
            duration_secs,
            transcript_path,
            action_items,
            decisions,
            markdown_report_path,
            open_questions,
            themes,
            participants,
        })
    }

    /// Get current session status.
    pub fn status(&self) -> SessionStatus {
        SessionStatus {
            topic: self.topic.clone(),
            message_count: self.history.len(),
            started_at: self.started_at.clone(),
            is_open: self.is_open,
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

    /// Access the conversation history (for export).
    pub fn history(&self) -> &[ConversationMessage] {
        &self.history
    }

    /// Access the session start time (for export).
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    // --- Private helpers ---

    /// Load active goal (slug, title) pairs from the default file-backed store.
    ///
    /// Returns an empty vec if the goals file doesn't exist or can't be read.
    /// This is best-effort — goal linkage is optional enrichment.
    fn load_active_goal_titles(&self) -> Vec<(String, String)> {
        use crate::goals::{FileBackedGoalStore, GoalStore};
        use crate::metadata::{BackendDescriptor, Freshness};

        let goals_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".simard/goals.json");

        if !goals_path.exists() {
            return Vec::new();
        }

        let descriptor = match Freshness::now() {
            Ok(f) => BackendDescriptor::for_runtime_type::<MeetingBackend>(
                "goals::file-backed",
                "meeting-goal-linkage",
                f,
            ),
            Err(_) => return Vec::new(),
        };

        match FileBackedGoalStore::new(&goals_path, descriptor) {
            Ok(store) => match store.active_top_goals(50) {
                Ok(goals) => goals.into_iter().map(|g| (g.slug, g.title)).collect(),
                Err(e) => {
                    debug!("Could not load active goals for linkage: {e}");
                    Vec::new()
                }
            },
            Err(e) => {
                debug!("Could not open goal store for linkage: {e}");
                Vec::new()
            }
        }
    }

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

    /// Ask the LLM for a summary with a timeout to prevent `/close` from hanging.
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

        info!(
            timeout_secs = SUMMARY_TIMEOUT.as_secs(),
            "Starting summary generation"
        );
        let start = std::time::Instant::now();

        // Run the LLM call in a scoped thread with a timeout so /close never hangs.
        let result = {
            let (tx, rx) = mpsc::channel();
            let agent = &mut *self.agent;

            std::thread::scope(|s| {
                s.spawn(move || {
                    let r = agent.run_turn(turn_input);
                    let _ = tx.send(r);
                });
                rx.recv_timeout(SUMMARY_TIMEOUT)
            })
        };

        match result {
            Ok(Ok(outcome)) => {
                info!(
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "Summary generated"
                );
                let text = extract_response(&outcome);
                if text.is_empty() {
                    warn!("LLM returned empty summary — using metadata summary");
                    self.metadata_summary()
                } else {
                    text
                }
            }
            Ok(Err(e)) => {
                warn!(elapsed_ms = start.elapsed().as_millis() as u64, error = %e, "LLM summarization failed");
                self.metadata_summary()
            }
            Err(_) => {
                warn!(
                    timeout_secs = SUMMARY_TIMEOUT.as_secs(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "Summary generation timed out — saving transcript without LLM summary"
                );
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
        chrono::DateTime::parse_from_rfc3339(&self.started_at)
            .ok()
            .map_or(0, |start| {
                Utc::now().signed_duration_since(start).num_seconds().max(0) as u64
            })
    }

    /// Save an in-progress transcript after every turn so killed meetings
    /// don't lose data. Errors are logged but not propagated.
    fn auto_save_transcript(&self) {
        let transcript = MeetingTranscript {
            topic: self.topic.clone(),
            started_at: self.started_at.clone(),
            closed_at: String::new(),
            duration_secs: self.elapsed_secs(),
            summary: "[in-progress — meeting still open]".to_string(),
            messages: self.history.clone(),
        };
        match persist::write_auto_save(&transcript) {
            Ok(p) => debug!(path = %p.display(), "Auto-saved transcript"),
            Err(e) => warn!("Auto-save failed (meeting continues): {e}"),
        }
    }
}

/// Extract the conversational response text from a `BaseTypeOutcome`,
/// stripping agentic tool-call log noise that garbles terminal display.
fn extract_response(outcome: &BaseTypeOutcome) -> String {
    sanitize_agent_output(outcome.execution_summary.trim())
}

/// Remove agentic tool-call log lines and XML-style tool blocks from LLM
/// output so the terminal displays only the conversational content.
fn sanitize_agent_output(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut in_tool_block = false;
    let mut consecutive_blank = 0u8;

    for line in raw.lines() {
        let trimmed = line.trim();

        if is_tool_block_open(trimmed) {
            in_tool_block = true;
            continue;
        }

        if in_tool_block {
            if is_tool_block_close(trimmed) {
                in_tool_block = false;
            }
            continue;
        }

        if is_tool_call_line(trimmed) {
            continue;
        }

        if trimmed.is_empty() {
            consecutive_blank += 1;
            if consecutive_blank <= 2 {
                result.push('\n');
            }
            continue;
        }
        consecutive_blank = 0;

        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

fn is_tool_block_open(trimmed: &str) -> bool {
    for tag in &[
        "<tool_call",
        "<tool_result",
        "<function_call",
        "<invoke",
        "<function",
    ] {
        if trimmed.starts_with(tag) {
            return true;
        }
    }
    false
}

fn is_tool_block_close(trimmed: &str) -> bool {
    for tag in &[
        "</tool_call",
        "</tool_result",
        "</function_call",
        "</invoke",
        "</function",
    ] {
        if trimmed.contains(tag) {
            return true;
        }
    }
    false
}

fn is_tool_call_line(trimmed: &str) -> bool {
    if trimmed.starts_with("[tool_call:") || trimmed.starts_with("[tool_result:") {
        return true;
    }
    if trimmed.starts_with("[Tool ") && trimmed.contains("executed") {
        return true;
    }
    if trimmed.starts_with("Running tool:") || trimmed.starts_with("Tool output:") {
        return true;
    }
    false
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
    fn sanitize_strips_tool_call_lines() {
        let input = "Here is the answer.\n[tool_call: search_files]\n[tool_result: found 3 files]\nThe files are ready.";
        let result = sanitize_agent_output(input);
        assert!(result.contains("Here is the answer."), "result: {result}");
        assert!(result.contains("The files are ready."), "result: {result}");
        assert!(!result.contains("[tool_call:"), "result: {result}");
        assert!(!result.contains("[tool_result:"), "result: {result}");
    }

    #[test]
    fn sanitize_strips_xml_tool_blocks() {
        let input = "Before block.\n<tool_call>\ninternal stuff\n</tool_call>\nAfter block.";
        let result = sanitize_agent_output(input);
        assert!(result.contains("Before block."), "result: {result}");
        assert!(result.contains("After block."), "result: {result}");
        assert!(!result.contains("internal stuff"), "result: {result}");
    }

    #[test]
    fn sanitize_passes_clean_text() {
        let input = "Normal response.\nWith multiple lines.\n\nAnd paragraphs.";
        let result = sanitize_agent_output(input);
        assert!(result.contains("Normal response."), "result: {result}");
        assert!(result.contains("With multiple lines."), "result: {result}");
        assert!(result.contains("And paragraphs."), "result: {result}");
    }

    #[test]
    fn sanitize_collapses_excessive_blanks() {
        let input = "Line 1\n\n\n\n\n\nLine 2";
        let result = sanitize_agent_output(input);
        assert!(!result.contains("\n\n\n\n"), "too many blanks: {result}");
        assert!(result.contains("Line 1"), "result: {result}");
        assert!(result.contains("Line 2"), "result: {result}");
    }

    #[test]
    fn sanitize_handles_empty_input() {
        assert_eq!(sanitize_agent_output(""), "");
        assert_eq!(sanitize_agent_output("   "), "");
    }

    #[test]
    fn auto_save_does_not_panic() {
        let agent = MockAgent::new("noted");
        let mut backend =
            MeetingBackend::new_session("AutoSave Test", Box::new(agent), None, String::new());
        backend.send_message("Test message").unwrap();
        assert_eq!(backend.status().message_count, 2);
    }
}
