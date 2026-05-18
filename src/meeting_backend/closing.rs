//! Closing flow: summary generation, action item extraction, persistence.

use chrono::Utc;
use tracing::{info, warn};

use crate::error::{SimardError, SimardResult};

use super::MeetingBackend;
use super::persist;
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

        // Prepend explicit action items recorded inline via `/action` so
        // operator-supplied items always appear first and are never lost
        // to extractor heuristics. Dedup against heuristic items by
        // case-insensitive description match. Issue #1730 seam (b).
        if !self.explicit_action_items.is_empty() {
            let mut explicit_first: Vec<super::types::HandoffActionItem> =
                self.explicit_action_items.clone();
            for inferred in action_items.drain(..) {
                let lower = inferred.description.to_lowercase();
                if !explicit_first
                    .iter()
                    .any(|a| a.description.to_lowercase() == lower)
                {
                    explicit_first.push(inferred);
                }
            }
            action_items = explicit_first;
        }

        // ── Goal linkage ──
        let goal_titles = self.load_active_goal_titles();
        if !goal_titles.is_empty() {
            persist::link_action_items_to_goals(&mut action_items, &goal_titles);
        }

        // ── Decision extraction ──
        let mut decisions = persist::extract_decisions(&self.history);

        // Prepend explicit decisions recorded inline via `/decision`.
        // Dedup against heuristic-extracted decisions by case-insensitive
        // string equality. Issue #1730 seam (b).
        if !self.explicit_decisions.is_empty() {
            let mut explicit_first: Vec<String> = self.explicit_decisions.clone();
            for inferred in decisions.drain(..) {
                let lower = inferred.to_lowercase();
                if !explicit_first.iter().any(|d| d.to_lowercase() == lower) {
                    explicit_first.push(inferred);
                }
            }
            decisions = explicit_first;
        }

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

        // ── Extract open questions and themes for the summary ──
        // Done *before* writing the handoff so explicit questions flow
        // into the JSON artifact too. Issue #1730 seam (b).
        let inferred_questions = persist::extract_open_questions(&self.history);
        let mut open_questions: Vec<String> = Vec::new();
        // Prepend explicit questions recorded via `/question` so they
        // always appear first; dedup against heuristic ones by
        // case-insensitive equality.
        for q in &self.explicit_questions {
            open_questions.push(q.clone());
        }
        for q in inferred_questions {
            let lower = q.text.to_lowercase();
            if !open_questions.iter().any(|e| e.to_lowercase() == lower) {
                open_questions.push(q.text);
            }
        }
        // Track which questions are explicit (operator-recorded) for the
        // bundle artifact's per-question `explicit` flag.
        let explicit_question_set: std::collections::HashSet<String> = self
            .explicit_questions
            .iter()
            .map(|q| q.to_lowercase())
            .collect();

        // Write MeetingHandoff artifact for OODA integration.
        if let Err(e) = persist::write_handoff_with_explicit(
            &self.topic,
            &summary_text,
            &self.history,
            &action_items,
            &decisions,
            &self.explicit_questions,
        ) {
            warn!("Failed to write meeting handoff: {e}");
        }

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
            &self.applied_templates,
        ) {
            Ok(p) => Some(p.to_string_lossy().to_string()),
            Err(e) => {
                warn!("Failed to write handoff markdown report: {e}");
                None
            }
        };

        // ── Per-meeting structured handoff bundle ──
        // Writes ~/.simard/meetings/<meeting_id>/{meeting_handoff.json,
        // meeting_handoff.md, transcript.json}. Independent of the legacy
        // OODA artifact above so existing downstream consumers keep working
        // while new consumers can rely on the canonical layout.
        let bundle_open_questions: Vec<crate::meeting_facilitator::OpenQuestion> = open_questions
            .iter()
            .cloned()
            .map(|text| {
                let is_explicit = explicit_question_set.contains(&text.to_lowercase());
                crate::meeting_facilitator::OpenQuestion {
                    text,
                    explicit: is_explicit,
                }
            })
            .collect();
        let bundle_dir = match persist::write_handoff_bundle(
            &self.topic,
            &summary_text,
            Some(&self.started_at),
            &self.history,
            &action_items,
            &decisions,
            bundle_open_questions,
            themes.clone(),
            participants.clone(),
        ) {
            Ok(p) => Some(p.to_string_lossy().to_string()),
            Err(e) => {
                warn!("Failed to write meeting handoff bundle: {e}");
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
            applied_templates: self.applied_templates.clone(),
            bundle_dir,
        })
    }
}
