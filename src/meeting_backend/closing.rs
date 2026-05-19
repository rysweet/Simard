//! Closing flow: summary generation, action item extraction, persistence.
//!
//! The close pipeline is bounded by a master + inner timeout pair (issue
//! #1908). See `docs/reference/meeting-close-lifecycle.md` for the public
//! contract and `close_guard` for the underlying `with_timeout` primitive.

use std::time::Instant;

use chrono::Utc;
use tracing::{info, warn};

use crate::error::{SimardError, SimardResult};

use super::MeetingBackend;
use super::close_guard::{self, PartialReason};
use super::persist;
use super::types::MeetingSummary;
use super::types::MeetingTranscript;
use super::types::Role;

/// Summary text persisted when the LLM summarizer phase timed out.
/// Matches the literal documented in
/// `docs/reference/meeting-close-lifecycle.md` so the docs and the runtime
/// stay in sync.
const PARTIAL_SUMMARY_FALLBACK: &str = "(partial — close timed out; full summary unavailable)";

impl MeetingBackend {
    /// Close the meeting session: summarize, extract action items, link goals,
    /// auto-export markdown report, persist, and store to memory.
    ///
    /// Bounded by `SIMARD_MEETING_CLOSE_TIMEOUT_SECS` (default 60s,
    /// clamped to `[1, 600]`) plus an inner `agent.close()` budget of
    /// `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS` (default 15s, clamped to
    /// `[1, 120]`). On a timeout the close still returns
    /// `Ok(MeetingSummary)` with `partial_reason = Some(_)` and a
    /// deserialize-valid handoff bundle written to disk (issue #1908).
    ///
    /// Returns a `MeetingSummary` with the summary text and structured
    /// action items.
    #[tracing::instrument(skip(self))]
    pub fn close(&mut self) -> SimardResult<MeetingSummary> {
        if !self.is_open {
            return Err(SimardError::ActionExecutionFailed {
                action: "close-meeting".to_string(),
                reason: "meeting session is already closed".to_string(),
            });
        }

        self.is_open = false;
        let close_started = Instant::now();
        let master_budget = super::resolve_close_timeout();
        let agent_close_budget = super::resolve_agent_close_timeout();
        let duration_secs = self.elapsed_secs();
        let mut partial_reason: Option<PartialReason> = None;

        info!(
            target: "simard::meeting_backend::closing",
            budget_secs = master_budget.as_secs(),
            topic = %self.topic,
            "meeting.close.start"
        );

        // ── Phase 1: summary generation (needs the agent alive) ──
        // Skip the LLM call entirely if the master budget is already spent
        // — the partial-handoff write below still happens.
        let summary_text = if close_started.elapsed() >= master_budget {
            partial_reason.get_or_insert(PartialReason::CloseTimeout);
            warn!(
                target: "simard::meeting_backend::closing",
                phase = "summary",
                outcome = "skipped",
                reason = PartialReason::CloseTimeout.as_wire_str(),
                "meeting.close.phase skipped — master budget already spent"
            );
            PARTIAL_SUMMARY_FALLBACK.to_string()
        } else {
            let (text, summary_reason) = self.generate_summary();
            if let Some(r) = summary_reason {
                partial_reason.get_or_insert(r);
                if matches!(r, PartialReason::SummaryTimeout) {
                    // Use the documented fallback string instead of the
                    // metadata summary so consumers see a stable signal.
                    return self.finalize_partial(
                        PARTIAL_SUMMARY_FALLBACK.to_string(),
                        duration_secs,
                        partial_reason,
                        close_started,
                    );
                }
            }
            text
        };

        // ── Phase 2: agent shutdown (inner budget) ──
        // Done after the summarizer so the agent is alive when its
        // `run_turn` is invoked. If `generate_summary` already
        // abandoned the agent to a detached worker (SummaryTimeout
        // → take()), `self.agent` is `None` and this is a no-op.
        if let Some(mut agent) = self.agent.take() {
            let phase_start = Instant::now();
            let close_outcome = close_guard::with_timeout(agent_close_budget, move || {
                let r = agent.close();
                (agent, r)
            });
            match close_outcome {
                Ok((agent, Ok(()))) => {
                    self.agent = Some(agent);
                    info!(
                        target: "simard::meeting_backend::closing",
                        phase = "agent_close",
                        ms = phase_start.elapsed().as_millis() as u64,
                        outcome = "ok",
                        "meeting.close.phase"
                    );
                }
                Ok((agent, Err(e))) => {
                    self.agent = Some(agent);
                    warn!(
                        target: "simard::meeting_backend::closing",
                        phase = "agent_close",
                        ms = phase_start.elapsed().as_millis() as u64,
                        outcome = "error",
                        error = %e,
                        "meeting.close.phase failed to close agent session"
                    );
                }
                Err(_) => {
                    // Agent is now abandoned in the detached worker
                    // thread (see close_guard module docs). `self.agent`
                    // stays `None` so subsequent close-time
                    // observers do not touch it.
                    warn!(
                        target: "simard::meeting_backend::closing",
                        phase = "agent_close",
                        ms = phase_start.elapsed().as_millis() as u64,
                        outcome = "timeout",
                        budget_secs = agent_close_budget.as_secs(),
                        handoff_partial = true,
                        reason = PartialReason::AgentCloseTimeout.as_wire_str(),
                        "meeting.close.phase agent.close exceeded budget; abandoning worker"
                    );
                    partial_reason = Some(PartialReason::AgentCloseTimeout);
                }
            }
        }

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
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_transcript",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write transcript"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
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
            warn!(
                target: "simard::meeting_backend::closing",
                phase = "persist_handoff",
                outcome = "error",
                error = %e,
                handoff_partial = true,
                reason = PartialReason::PersistenceError.as_wire_str(),
                "Failed to write meeting handoff"
            );
            partial_reason.get_or_insert(PartialReason::PersistenceError);
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
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_markdown",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write handoff markdown report"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
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
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_bundle",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write meeting handoff bundle"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
                None
            }
        };

        // ── Memory consolidation ── (no-op in current production; bridge
        // is always `None`. Kept for forward compatibility; bounded with
        // the agent-close budget if a future caller wires a bridge in.)
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

        // ── Final partial-reason gate ──
        // If we have spent past the master budget by this point but
        // every phase still succeeded, that's still a partial close.
        if partial_reason.is_none() && close_started.elapsed() > master_budget {
            partial_reason = Some(PartialReason::CloseTimeout);
        }

        info!(
            target: "simard::meeting_backend::closing",
            topic = self.topic,
            messages = self.history.len(),
            action_items = action_items.len(),
            decisions = decisions.len(),
            duration_secs,
            partial = partial_reason.is_some(),
            total_ms = close_started.elapsed().as_millis() as u64,
            bundle_dir = bundle_dir.as_deref().unwrap_or(""),
            "meeting.close.done"
        );

        if let Some(r) = partial_reason {
            warn!(
                target: "simard::meeting_backend::closing",
                handoff_partial = true,
                reason = r.as_wire_str(),
                meeting_id = %crate::meeting_facilitator::derive_meeting_id(&self.started_at, &self.topic),
                wrote = bundle_dir.as_deref().unwrap_or(""),
                "meeting.close partial handoff written"
            );
        }

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
            partial_reason,
        })
    }

    /// Fast-finish a close that timed out during summary generation —
    /// skips the rest of the heuristic-extraction and goal-linkage
    /// phases, writes a minimal-but-deserialize-valid handoff bundle,
    /// and returns. The on-disk schema is identical to a full close
    /// (no new required fields). Used by `close()` when the summarizer
    /// inner budget fires (issue #1908).
    fn finalize_partial(
        &mut self,
        summary_text: String,
        duration_secs: u64,
        mut partial_reason: Option<PartialReason>,
        close_started: Instant,
    ) -> SimardResult<MeetingSummary> {
        // Always populate explicit items so an operator who typed
        // `/decision` or `/action` before `/close` doesn't lose them
        // to the partial-fast-path.
        let action_items: Vec<super::types::HandoffActionItem> = self.explicit_action_items.clone();
        let decisions: Vec<String> = self.explicit_decisions.clone();
        let open_questions: Vec<String> = self.explicit_questions.clone();
        let themes: Vec<String> = self.themes.clone();

        // Build participants from the live history so the partial bundle
        // is still useful for operator review.
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
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_transcript",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write transcript on partial close"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
                None
            }
        };

        if let Err(e) = persist::write_handoff_with_explicit(
            &self.topic,
            &summary_text,
            &self.history,
            &action_items,
            &decisions,
            &self.explicit_questions,
        ) {
            warn!(
                target: "simard::meeting_backend::closing",
                phase = "persist_handoff",
                outcome = "error",
                error = %e,
                handoff_partial = true,
                reason = PartialReason::PersistenceError.as_wire_str(),
                "Failed to write partial meeting handoff"
            );
            partial_reason.get_or_insert(PartialReason::PersistenceError);
        }

        let explicit_question_set: std::collections::HashSet<String> = self
            .explicit_questions
            .iter()
            .map(|q| q.to_lowercase())
            .collect();
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
        let structured_decisions: Vec<crate::meeting_facilitator::MeetingDecision> = decisions
            .iter()
            .map(|d| crate::meeting_facilitator::MeetingDecision {
                description: d.clone(),
                rationale: String::new(),
                participants: Vec::new(),
            })
            .collect();
        let markdown_report_path = persist::write_handoff_markdown_report(
            &self.topic,
            &self.started_at,
            &summary_text,
            &self.history,
            &action_items,
            &structured_decisions,
            &self.applied_templates,
        )
        .ok()
        .map(|p| p.to_string_lossy().to_string());

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
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_bundle",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write partial bundle"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
                None
            }
        };

        info!(
            target: "simard::meeting_backend::closing",
            topic = self.topic,
            messages = self.history.len(),
            duration_secs,
            partial = true,
            total_ms = close_started.elapsed().as_millis() as u64,
            bundle_dir = bundle_dir.as_deref().unwrap_or(""),
            "meeting.close.done"
        );
        if let Some(r) = partial_reason {
            warn!(
                target: "simard::meeting_backend::closing",
                handoff_partial = true,
                reason = r.as_wire_str(),
                meeting_id = %crate::meeting_facilitator::derive_meeting_id(&self.started_at, &self.topic),
                wrote = bundle_dir.as_deref().unwrap_or(""),
                "meeting.close partial handoff written"
            );
        }

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
            partial_reason,
        })
    }
}
