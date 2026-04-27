//! Unified meeting backend — one conversational engine, two thin frontends.
//!
//! `MeetingBackend` owns the conversation history, system prompt, LLM
//! delegation (via `BaseTypeSession::run_turn()`), and persistence. The CLI
//! REPL and dashboard WebSocket are thin adapters around this struct.

pub mod command;
pub mod lightweight;
pub mod persist;
#[cfg(test)]
mod tests_persist;
#[cfg(test)]
mod tests_persist_extra;
pub mod types;

mod closing;
mod messaging;

use std::fmt::Write as _;
use std::sync::mpsc;
use std::time::Duration;

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::base_types::{BaseTypeSession, BaseTypeTurnInput};
use crate::cognitive_memory::CognitiveMemoryOps;

pub use command::{MeetingCommand, parse_command};
pub use types::{
    ConversationMessage, HandoffActionItem, MeetingResponse, MeetingSummary, MeetingTranscript,
    Role, SessionStatus,
};

/// Maximum messages kept in conversation history.
pub(super) const MAX_HISTORY: usize = 500;

/// Number of recent messages included verbatim in the LLM prompt.
const RECENT_WINDOW: usize = 30;

/// Maximum time to wait for LLM summary generation before falling back.
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// Sentinel content returned when the LLM yields no usable text after
/// sanitisation. Surfacing this to the dashboard is preferable to an empty
/// bubble — operators can see that the turn completed but produced nothing.
pub const EMPTY_RESPONSE_SENTINEL: &str = "[empty response]";

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
    /// Explicit themes recorded via the `/theme` command.
    themes: Vec<String>,
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
            themes: Vec::new(),
        }
    }

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

    /// Record an explicit theme for this meeting.
    ///
    /// Themes recorded here are merged with inferred themes on `close()`.
    pub fn push_theme(&mut self, theme: String) {
        let lower = theme.to_lowercase();
        if !self.themes.iter().any(|t| t.to_lowercase() == lower) {
            self.themes.push(theme);
        }
    }

    /// Read the explicit themes recorded so far.
    pub fn explicit_themes(&self) -> &[String] {
        &self.themes
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
    /// call fails or returns empty — this is the
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
            .map(|start| Utc::now().signed_duration_since(start).num_seconds().max(0) as u64)
            .unwrap_or(0)
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

mod sanitize;
pub use sanitize::*;

#[cfg(test)]
mod tests_mod;
