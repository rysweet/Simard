//! Unified meeting backend — one conversational engine, two thin frontends.
//!
//! `MeetingBackend` owns the conversation history, system prompt, LLM
//! delegation (via `BaseTypeSession::run_turn()`), and persistence. The CLI
//! REPL and dashboard WebSocket are thin adapters around this struct.

pub mod close_guard;
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
use std::time::Duration;

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::base_types::{BaseTypeSession, BaseTypeTurnInput};
use crate::cognitive_memory::CognitiveMemoryOps;
// imported for cfg(test) consumers in meeting_backend/tests_mod.rs (false-positive of clippy unused_imports on lib pass — see #1405)
#[allow(unused_imports)]
use crate::error::SimardResult;

pub use close_guard::{PartialReason, Timeout, with_timeout};
pub use command::{MeetingCommand, parse_command};
pub use types::{
    AppliedTemplate, ConversationMessage, HandoffActionItem, MeetingResponse, MeetingSummary,
    MeetingTranscript, Role, SessionStatus,
};
/// Maximum messages kept in conversation history.
pub(super) const MAX_HISTORY: usize = 500;

/// Number of recent messages included verbatim in the LLM prompt.
const RECENT_WINDOW: usize = 30;

/// Default master budget for `MeetingBackend::close()` (issue #1908).
///
/// Clamped to `[1, 600]` seconds; overridable via
/// `SIMARD_MEETING_CLOSE_TIMEOUT_SECS`. See
/// `docs/reference/meeting-close-lifecycle.md` for the full timing
/// contract.
pub(super) const DEFAULT_CLOSE_TIMEOUT_SECS: u64 = 60;

/// Default inner budget for `agent.close()` (issue #1908, #1999).
///
/// Clamped to `[1, 120]` seconds; overridable via
/// `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS`. Used both by the agent
/// shutdown phase and (as the per-LLM-call cap) by the summary
/// generator.
///
/// Raised from 15 → 45 in #1999: the previous 15s default was shorter
/// than a single lightweight-chat LLM turn at p95, causing every
/// real-world `/close` to race the summarizer and produce a partial
/// handoff with no summary.
pub(super) const DEFAULT_AGENT_CLOSE_TIMEOUT_SECS: u64 = 45;

/// Env var for the master close budget.
pub(super) const CLOSE_TIMEOUT_ENV: &str = "SIMARD_MEETING_CLOSE_TIMEOUT_SECS";

/// Env var for the inner agent-close + summarizer budget.
pub(super) const AGENT_CLOSE_TIMEOUT_ENV: &str = "SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS";

/// Resolve the master close-timeout budget from the env, clamping to
/// `[1, 600]` and emitting a WARN on malformed values.
pub(super) fn resolve_close_timeout() -> Duration {
    resolve_env_duration_secs(CLOSE_TIMEOUT_ENV, DEFAULT_CLOSE_TIMEOUT_SECS, 1, 600)
}

/// Resolve the inner agent-close budget from the env, clamping to
/// `[1, 120]` and emitting a WARN on malformed values.
pub(super) fn resolve_agent_close_timeout() -> Duration {
    resolve_env_duration_secs(
        AGENT_CLOSE_TIMEOUT_ENV,
        DEFAULT_AGENT_CLOSE_TIMEOUT_SECS,
        1,
        120,
    )
}

fn resolve_env_duration_secs(
    env_var: &'static str,
    default_secs: u64,
    min_secs: u64,
    max_secs: u64,
) -> Duration {
    let raw = match std::env::var(env_var) {
        Ok(v) => v,
        Err(_) => return Duration::from_secs(default_secs),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Duration::from_secs(default_secs);
    }
    match trimmed.parse::<u64>() {
        Ok(v) => {
            let clamped = v.clamp(min_secs, max_secs);
            if clamped != v {
                warn!(
                    env_var = env_var,
                    requested = v,
                    clamped = clamped,
                    reason = "clamped",
                    "meeting close timeout out of allowed range; clamped"
                );
            }
            Duration::from_secs(clamped)
        }
        Err(e) => {
            warn!(
                env_var = env_var,
                raw = %trimmed,
                error = %e,
                default_secs = default_secs,
                reason = "malformed",
                "meeting close timeout malformed; using default"
            );
            Duration::from_secs(default_secs)
        }
    }
}

/// The unified meeting backend.
///
/// Maintains conversation state, delegates to an LLM agent, and handles
/// persistence. All methods are synchronous (matching `BaseTypeSession`).
pub struct MeetingBackend {
    topic: String,
    history: Vec<ConversationMessage>,
    system_prompt: String,
    /// `None` when the agent has been moved into a detached close worker
    /// that exceeded its budget (issue #1908). On a partial close,
    /// `send_message` and any subsequent `agent.close()` call fall through
    /// to safe no-ops.
    agent: Option<Box<dyn BaseTypeSession>>,
    bridge: Option<Box<dyn CognitiveMemoryOps>>,
    started_at: String,
    is_open: bool,
    /// Explicit themes recorded via the `/theme` command.
    themes: Vec<String>,
    /// Templates applied via the `/template <name>` command. Surfaced in the
    /// handoff markdown report as the `## Agenda` section.
    applied_templates: Vec<AppliedTemplate>,
    /// Decisions recorded inline by the operator via `/decision <text>`.
    /// Bypass post-hoc heuristic extraction so important items cannot be
    /// missed or mangled. Issue #1730 seam (b).
    explicit_decisions: Vec<String>,
    /// Action items recorded inline by the operator via `/action <text>`.
    /// Description is taken verbatim; assignee/deadline are best-effort
    /// extracted using the same helpers as the heuristic path. Issue #1730
    /// seam (b).
    explicit_action_items: Vec<HandoffActionItem>,
    /// Open questions recorded inline by the operator via `/question <text>`.
    /// Marked `explicit=true` in the handoff so downstream consumers can
    /// distinguish operator-supplied items from inferred ones. Issue #1730
    /// seam (b).
    explicit_questions: Vec<String>,
    /// Owner set inline by the operator via `/owner <name>`. Names the next
    /// agent/persona/human expected to action the handoff (e.g. `engineer`,
    /// `ooda-curate`, `act-on-decisions`, a GitHub handle). Surfaced on the
    /// `MeetingHandoff` via the new `next_owner` field. Added in #1954.
    explicit_next_owner: Option<String>,
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
        // Backend contract (issue #1905): callers (`open_meeting_agent_session`,
        // `SessionBuilder::open`) hand the backend an already-opened session.
        // Re-opening here previously produced a spurious WARN on every meeting
        // boot ("session is already open"). The backend never re-opens.
        debug!(
            backend = ?agent.descriptor().id,
            "MeetingBackend: caller-opened session adopted (no re-open)"
        );
        info!(topic, "Meeting session created");
        Self {
            topic: topic.to_string(),
            history: Vec::new(),
            system_prompt,
            agent: Some(agent),
            bridge,
            started_at,
            is_open: true,
            themes: Vec::new(),
            applied_templates: Vec::new(),
            explicit_decisions: Vec::new(),
            explicit_action_items: Vec::new(),
            explicit_questions: Vec::new(),
            explicit_next_owner: None,
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

    /// Append a synthetic conversation message without invoking the
    /// agent. Exists so outside-in integration tests can seed history
    /// before exercising `close()` against blocking-mock sessions
    /// (issue #1908 regression coverage). Not part of the production
    /// REPL flow — use `send_message` for that.
    #[doc(hidden)]
    pub fn push_test_message(&mut self, role: &str, content: &str) {
        let role = match role {
            "operator" | "user" => Role::User,
            "simard" | "assistant" => Role::Assistant,
            _ => Role::System,
        };
        self.history.push(ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: Utc::now().to_rfc3339(),
        });
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

    /// Record that the operator applied a meeting template (e.g. `/template
    /// standup`). Dedupes by template name (case-insensitive) so re-applying
    /// the same template is a no-op. The first application wins so the
    /// `applied_at` timestamp reflects when the agenda was first introduced.
    pub fn apply_template(&mut self, name: &str, agenda: &str) {
        let lower = name.to_lowercase();
        if self
            .applied_templates
            .iter()
            .any(|t| t.name.to_lowercase() == lower)
        {
            return;
        }
        self.applied_templates.push(AppliedTemplate {
            name: name.to_string(),
            agenda: agenda.to_string(),
            applied_at: Utc::now().to_rfc3339(),
        });
    }

    /// Read the templates applied during this meeting.
    pub fn applied_templates(&self) -> &[AppliedTemplate] {
        &self.applied_templates
    }

    // ── Inline /decision /action /question (issue #1730 seam (b)) ─────

    /// Record a decision the operator marked deterministically with
    /// `/decision <text>`. Trailing/leading whitespace is trimmed and empty
    /// values are ignored. Duplicates (case-insensitive) are deduplicated so
    /// re-typing `/decision Adopt TDD` is a no-op.
    pub fn push_explicit_decision(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let lower = trimmed.to_lowercase();
        if self
            .explicit_decisions
            .iter()
            .any(|d| d.to_lowercase() == lower)
        {
            return;
        }
        self.explicit_decisions.push(trimmed.to_string());
    }

    /// Read the decisions the operator recorded inline so far.
    pub fn explicit_decisions(&self) -> &[String] {
        &self.explicit_decisions
    }

    /// Record an action item the operator marked with `/action <text>`. The
    /// description is taken verbatim and the same assignee/deadline
    /// extractors used by the heuristic path are applied for free
    /// structured fields. `priority` is set to `Some(1)` so explicit items
    /// sort ahead of heuristic-extracted ones (which use `None`). Duplicates
    /// (case-insensitive on description) are deduplicated.
    pub fn push_explicit_action_item(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let assignee = persist::extract_assignee_pub(trimmed);
        let deadline = persist::extract_deadline_pub(&trimmed.to_lowercase());
        let description = persist::clean_action_description_pub(trimmed);
        let lower_desc = description.to_lowercase();
        if self
            .explicit_action_items
            .iter()
            .any(|a| a.description.to_lowercase() == lower_desc)
        {
            return;
        }
        self.explicit_action_items.push(HandoffActionItem {
            description,
            assignee,
            deadline,
            linked_goal: None,
            priority: Some(1),
        });
    }

    /// Read the action items the operator recorded inline so far.
    pub fn explicit_action_items(&self) -> &[HandoffActionItem] {
        &self.explicit_action_items
    }

    /// Record an open question the operator marked with `/question <text>`.
    /// Trailing/leading whitespace is trimmed; empty values are ignored.
    /// Duplicates (case-insensitive) are deduplicated.
    pub fn push_explicit_question(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let lower = trimmed.to_lowercase();
        if self
            .explicit_questions
            .iter()
            .any(|q| q.to_lowercase() == lower)
        {
            return;
        }
        self.explicit_questions.push(trimmed.to_string());
    }

    /// Read the open questions the operator recorded inline so far.
    pub fn explicit_questions(&self) -> &[String] {
        &self.explicit_questions
    }

    /// Record the next responsible owner the operator named with
    /// `/owner <name>`. Trimmed; empty values clear the value. Stored
    /// verbatim so case is preserved for GitHub handles and persona names.
    /// Surfaced into the `next_owner` field of the resulting
    /// `MeetingHandoff`. Added in issue #1954.
    pub fn push_next_owner(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.explicit_next_owner = None;
        } else {
            self.explicit_next_owner = Some(trimmed.to_string());
        }
    }

    /// Read the next-owner value the operator set inline (if any).
    pub fn explicit_next_owner(&self) -> Option<&str> {
        self.explicit_next_owner.as_deref()
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

    /// Ask the LLM for a summary with a hard timeout so `/close` never hangs
    /// past its budget (issue #1908).
    ///
    /// Returns the summary text **and** a flag indicating whether the
    /// summarizer phase produced a partial-handoff signal. The agent is
    /// moved into a detached worker thread for the duration of the LLM
    /// call; on success it is restored to `self.agent` and the caller
    /// observes no state change. On timeout it stays in the detached
    /// worker (which continues to drain in the background — see
    /// `close_guard` module docs for the abandon-not-kill trade-off) and
    /// `self.agent` is left as `None` so any subsequent `agent.close()`
    /// call falls through to a no-op rather than touching a poisoned
    /// session.
    pub(super) fn generate_summary(&mut self) -> (String, Option<PartialReason>) {
        if self.history.is_empty() {
            return ("Empty meeting — no messages exchanged.".to_string(), None);
        }

        let agent = match self.agent.take() {
            Some(a) => a,
            None => {
                warn!(
                    handoff_partial = true,
                    reason = PartialReason::SummaryTimeout.as_wire_str(),
                    "Summary skipped — agent already taken by an earlier phase"
                );
                return (self.metadata_summary(), Some(PartialReason::SummaryTimeout));
            }
        };

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

        let summary_budget = resolve_agent_close_timeout();
        info!(
            timeout_secs = summary_budget.as_secs(),
            "meeting.close.phase phase=summary starting"
        );
        let start = std::time::Instant::now();

        // Move the agent into a detached worker. `with_timeout` uses a
        // non-scoped `thread::spawn` so the parent returns promptly when
        // the budget expires (issue #1908 root cause was the old
        // `thread::scope` which joins at scope end and therefore could
        // never honour the timeout).
        let result = with_timeout(summary_budget, move || {
            let mut agent = agent;
            let r = agent.run_turn(turn_input);
            (agent, r)
        });

        match result {
            Ok((agent, Ok(outcome))) => {
                self.agent = Some(agent);
                let elapsed_ms = start.elapsed().as_millis() as u64;
                info!(
                    elapsed_ms,
                    phase = "summary",
                    outcome = "ok",
                    "meeting.close.phase done"
                );
                let text = extract_response(&outcome);
                if text.trim().is_empty() {
                    warn!(
                        handoff_partial = true,
                        reason = PartialReason::SummaryEmpty.as_wire_str(),
                        "LLM returned empty summary — using metadata summary"
                    );
                    (self.metadata_summary(), Some(PartialReason::SummaryEmpty))
                } else {
                    (text, None)
                }
            }
            Ok((agent, Err(e))) => {
                self.agent = Some(agent);
                let elapsed_ms = start.elapsed().as_millis() as u64;
                warn!(
                    elapsed_ms,
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::SummaryEmpty.as_wire_str(),
                    "LLM summarization failed — using metadata summary"
                );
                (self.metadata_summary(), Some(PartialReason::SummaryEmpty))
            }
            Err(_) => {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                // Agent stays in the detached worker; intentionally leave
                // `self.agent = None` so closing.rs guards `agent.close()`
                // with `if let Some(_)` and we never touch a partially
                // shut-down session.
                warn!(
                    timeout_secs = summary_budget.as_secs(),
                    elapsed_ms,
                    handoff_partial = true,
                    reason = PartialReason::SummaryTimeout.as_wire_str(),
                    "Summary generation timed out — saving transcript without LLM summary"
                );
                (self.metadata_summary(), Some(PartialReason::SummaryTimeout))
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
