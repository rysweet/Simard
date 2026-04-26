//! Closing flow: summary generation, action item extraction, persistence.

use chrono::Utc;
use tracing::{info, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::EMPTY_RESPONSE_SENTINEL;
use super::MeetingBackend;
use super::SUMMARY_TIMEOUT;
use super::persist;
use super::types::HandoffActionItem;
use super::types::MeetingSummary;
use super::types::MeetingTranscript;
use super::types::Role;

impl MeetingBackend {
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
        // Explicit /theme entries come first; inferred themes fill in the rest.
        let inferred_themes = persist::extract_themes(&self.history);
        let mut themes: Vec<String> = self.themes.clone();
        for t in inferred_themes {
            let lower = t.to_lowercase();
            if !themes.iter().any(|e| e.to_lowercase() == lower) {
                themes.push(t);
            }
        }

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
        // Convert extracted decision strings to MeetingDecision structs (with rationale).
        let structured_decisions: Vec<crate::meeting_facilitator::MeetingDecision> = decisions
            .iter()
            .map(|d| {
                let rationale = persist::extract_decision_rationale_pub(d, &self.history);
                let participants = persist::extract_decision_participants_pub(d, &self.history);
                crate::meeting_facilitator::MeetingDecision {
                    description: d.clone(),
                    rationale,
                    participants,
                }
            })
            .collect();
        let markdown_report_path = match persist::write_handoff_markdown_report(
            &self.topic,
            &self.started_at,
            &summary_text,
            &self.history,
            &action_items,
            &structured_decisions,
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
}
