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
    /// `SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS` (default 45s, clamped to
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
        // Explicit decisions carry operator-supplied rationale (issue #2086)
        // so they are preserved as-is in `structured_decisions` below.
        if !self.explicit_decisions.is_empty() {
            let mut explicit_first: Vec<String> = self
                .explicit_decisions
                .iter()
                .map(|d| d.description.clone())
                .collect();
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

        // ── Themes + participants (computed before persist so the
        // enrichment payload can use them) ──
        let inferred_themes = persist::extract_themes(&self.history);
        let mut themes: Vec<String> = self.themes.clone();
        for t in inferred_themes {
            let lower = t.to_lowercase();
            if !themes.iter().any(|e| e.to_lowercase() == lower) {
                themes.push(t);
            }
        }

        // ── Risks (issue #2084, spec line 637) ──
        let inferred_risks = persist::extract_risks(&self.history);
        let mut risks: Vec<String> = self.explicit_risks.clone();
        for r in inferred_risks {
            let lower = r.to_lowercase();
            if !risks.iter().any(|e| e.to_lowercase() == lower) {
                risks.push(r);
            }
        }

        // ── Disagreements (issue #2084, spec line 645) ──
        let inferred_disagreements = persist::extract_disagreements(&self.history);
        let mut disagreements: Vec<String> = self.explicit_disagreements.clone();
        for d in inferred_disagreements {
            let lower = d.to_lowercase();
            if !disagreements.iter().any(|e| e.to_lowercase() == lower) {
                disagreements.push(d);
            }
        }

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

        // ── Build structured decisions once (issue #1954, #2086) ──
        // Threaded through both the legacy handoff and the bundle so
        // rationale/participants extracted from the live conversation
        // aren't reduced to `String::new()` placeholders downstream.
        // Explicit decisions carry operator-supplied rationale (issue #2086);
        // non-empty rationale from the operator takes precedence over the
        // heuristic extractor.
        let explicit_map: std::collections::HashMap<
            String,
            &crate::meeting_facilitator::MeetingDecision,
        > = self
            .explicit_decisions
            .iter()
            .map(|d| (d.description.to_lowercase(), d))
            .collect();
        let structured_decisions: Vec<crate::meeting_facilitator::MeetingDecision> = decisions
            .iter()
            .map(|d| {
                if let Some(explicit) = explicit_map.get(&d.to_lowercase()) {
                    // Use operator-supplied rationale if non-empty, otherwise
                    // fall back to heuristic extraction from context.
                    let rationale = if explicit.rationale.is_empty() {
                        persist::extract_decision_rationale_pub(d, &self.history)
                    } else {
                        explicit.rationale.clone()
                    };
                    let participants = if explicit.participants.is_empty() {
                        persist::extract_decision_participants_pub(d, &self.history)
                    } else {
                        explicit.participants.clone()
                    };
                    crate::meeting_facilitator::MeetingDecision {
                        description: d.clone(),
                        rationale,
                        participants,
                    }
                } else {
                    let rationale = persist::extract_decision_rationale_pub(d, &self.history);
                    let participants = persist::extract_decision_participants_pub(d, &self.history);
                    crate::meeting_facilitator::MeetingDecision {
                        description: d.clone(),
                        rationale,
                        participants,
                    }
                }
            })
            .collect();

        // ── Write the markdown report first so its path can be carried
        // into the handoff's artifacts list (issue #1954). ──
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

        // ── Derive next_owner (issue #1954) ──
        // Precedence: explicit `/owner <name>` > most-frequent
        // `action_items[].assignee` (when non-empty) > `None`.
        let next_owner_owned = self
            .explicit_next_owner
            .clone()
            .or_else(|| most_frequent_action_owner(&action_items));

        // ── Pre-compute the per-meeting bundle directory (deterministic
        // from `meeting_id`) so the artifacts list can name it before the
        // bundle writer actually runs (issue #1954). ──
        let meeting_id =
            crate::meeting_facilitator::derive_meeting_id(&self.started_at, &self.topic);
        let bundle_dir_expected = crate::meeting_facilitator::meeting_bundle_dir(&meeting_id);

        // ── Build the artifacts list (issue #1954) ──
        let artifacts = build_handoff_artifacts(
            transcript_path.as_deref(),
            Some(&bundle_dir_expected.to_string_lossy()),
            markdown_report_path.as_deref(),
            &self.applied_templates,
        );

        // Compute the effective goal once for both enrichment sites.
        let effective_goal = self.effective_goal();

        let enrichment = persist::HandoffEnrichment {
            next_owner: next_owner_owned.as_deref(),
            artifacts: artifacts.clone(),
            structured_decisions: Some(structured_decisions.clone()),
            goal: effective_goal.as_deref(),
            next_actor: None,
            applied_templates: self.applied_templates.clone(),
            history_truncated_count: self.history_truncated_count,
            partial_reason: partial_reason.map(|r| r.as_wire_str().to_string()),
            risks: risks.clone(),
            disagreements: disagreements.clone(),
        };

        // Write MeetingHandoff artifact for OODA integration.
        if let Err(e) = persist::write_handoff_with_explicit(
            &self.topic,
            &summary_text,
            &self.history,
            &action_items,
            &decisions,
            &self.explicit_questions,
            enrichment.clone(),
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

        // ── Per-meeting structured handoff bundle ──
        // Writes ~/.simard/meetings/<meeting_id>/{meeting_handoff.json,
        // meeting_handoff.md, transcript.json}. Independent of the legacy
        // OODA artifact above so existing downstream consumers keep working
        // while new consumers can rely on the canonical layout.
        //
        // Issue #2086: cap the conversation history in the bundle to
        // MAX_HANDOFF_HISTORY recent turns. The full transcript is preserved
        // in the standalone transcript.json written earlier; the bundle
        // carries concise context, not the bloated transcript.
        let handoff_history: &[super::types::ConversationMessage] =
            if self.history.len() > super::MAX_HANDOFF_HISTORY {
                &self.history[self.history.len() - super::MAX_HANDOFF_HISTORY..]
            } else {
                &self.history
            };
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
            handoff_history,
            &action_items,
            &decisions,
            bundle_open_questions,
            themes.clone(),
            participants.clone(),
            enrichment,
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

        // ── Per-meeting `memory_records.json` — issue #2000 fix. ──
        // The companion `simard meeting read` command opens
        // `<state_root>/memory_records.json` via `FileBackedMemoryStore`
        // and hard-fails if the file is missing. Without this write, every
        // successful REPL meeting produced an unreadable artifact. We
        // write the file even when no decisions/action items were
        // extracted (an empty-but-valid record) so the read path never
        // hard-fails on a successful close.
        if let Some(ref dir) = bundle_dir {
            let bundle_path = std::path::PathBuf::from(dir);
            if let Err(e) = persist::write_meeting_memory_records(
                &bundle_path,
                &self.topic,
                &decisions,
                &action_items,
                &open_questions,
            ) {
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_memory_records",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write meeting memory_records.json"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
            }
        }

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

        // ── Direct goal writes (issue #2182) ──
        // Write GoalRecords directly to the file-backed goal store so goals
        // are available immediately after meeting close, without waiting for
        // the OODA curate cycle to process the handoff artifact.
        // Fallback: when structured_decisions is empty but the meeting topic
        // is goal-related and an explicit /goal was set, synthesize a decision.
        if structured_decisions.is_empty() {
            let fallback =
                fallback_goal_decisions(self.explicit_goal.as_deref(), &self.topic, &self.history);
            write_goals_from_decisions(&fallback);
        } else {
            write_goals_from_decisions(&structured_decisions);
        }

        // ── Goal carryover record (issue #2092, spec line 665) ──
        // Write an explicit carryover record to cognitive memory so the
        // engineer loop can verify it received the goal state the meeting
        // produced. Without this, the handoff is implicit and goals can
        // silently vanish if the state root diverges.
        if let Some(ref bridge) = self.bridge {
            match crate::goal_curation::load_goal_board(&**bridge) {
                Ok(board) => {
                    if let Err(e) =
                        crate::goal_curation::write_goal_carryover(&board, &meeting_id, &**bridge)
                    {
                        warn!(
                            target: "simard::meeting_backend::closing",
                            phase = "goal_carryover",
                            outcome = "error",
                            error = %e,
                            "Failed to write goal carryover record"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        target: "simard::meeting_backend::closing",
                        phase = "goal_carryover",
                        outcome = "error",
                        error = %e,
                        "Failed to load goal board for carryover record"
                    );
                }
            }
        } else {
            // When no bridge is available (most current deployments), try
            // to launch one from the default state root for this write.
            // Compiled out of test builds: `default_state_root()` returns
            // `$HOME/.simard` which trips the hermetic guard (#2092).
            #[cfg(not(test))]
            {
                let state_root = crate::memory_ipc::default_state_root();
                match crate::memory_ipc::launch_writer_bridge(&state_root) {
                    Ok(bridge) => match crate::goal_curation::load_goal_board(bridge.ops()) {
                        Ok(board) => {
                            if let Err(e) = crate::goal_curation::write_goal_carryover(
                                &board,
                                &meeting_id,
                                bridge.ops(),
                            ) {
                                warn!(
                                    target: "simard::meeting_backend::closing",
                                    phase = "goal_carryover",
                                    outcome = "error",
                                    error = %e,
                                    "Failed to write goal carryover record (fallback bridge)"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                target: "simard::meeting_backend::closing",
                                phase = "goal_carryover",
                                outcome = "error",
                                error = %e,
                                "Failed to load goal board for carryover (fallback bridge)"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::debug!(
                            target: "simard::meeting_backend::closing",
                            error = %e,
                            "Could not launch bridge for goal carryover write"
                        );
                    }
                }
            }
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
            orphan_turn_count: self.orphan_turn_count,
            risks,
            disagreements,
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
        let decisions: Vec<String> = self
            .explicit_decisions
            .iter()
            .map(|d| d.description.clone())
            .collect();
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

        // Issue #1954, #2086: extract rationale/participants from the live
        // history rather than emitting placeholder `String::new()` values.
        // Operator-supplied rationale from `/decision --rationale` takes
        // precedence.
        let explicit_map: std::collections::HashMap<
            String,
            &crate::meeting_facilitator::MeetingDecision,
        > = self
            .explicit_decisions
            .iter()
            .map(|d| (d.description.to_lowercase(), d))
            .collect();
        let structured_decisions: Vec<crate::meeting_facilitator::MeetingDecision> = decisions
            .iter()
            .map(|d| {
                if let Some(explicit) = explicit_map.get(&d.to_lowercase()) {
                    let rationale = if explicit.rationale.is_empty() {
                        persist::extract_decision_rationale_pub(d, &self.history)
                    } else {
                        explicit.rationale.clone()
                    };
                    let participants = if explicit.participants.is_empty() {
                        persist::extract_decision_participants_pub(d, &self.history)
                    } else {
                        explicit.participants.clone()
                    };
                    crate::meeting_facilitator::MeetingDecision {
                        description: d.clone(),
                        rationale,
                        participants,
                    }
                } else {
                    let rationale = persist::extract_decision_rationale_pub(d, &self.history);
                    let participants = persist::extract_decision_participants_pub(d, &self.history);
                    crate::meeting_facilitator::MeetingDecision {
                        description: d.clone(),
                        rationale,
                        participants,
                    }
                }
            })
            .collect();

        // ── Direct goal writes — same as close() happy path (issue #2182) ──
        // finalize_partial must also write goals so a summary timeout doesn't
        // silently drop goal-related decisions.
        if structured_decisions.is_empty() {
            let fallback =
                fallback_goal_decisions(self.explicit_goal.as_deref(), &self.topic, &self.history);
            write_goals_from_decisions(&fallback);
        } else {
            write_goals_from_decisions(&structured_decisions);
        }

        // Write the markdown report first so its path can flow into
        // the artifacts list of the handoff JSON (issue #1954).
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

        // Derive `next_owner` (issue #1954): explicit `/owner` value first,
        // then most-frequent action assignee, otherwise None.
        let next_owner_owned = self
            .explicit_next_owner
            .clone()
            .or_else(|| most_frequent_action_owner(&action_items));

        let meeting_id =
            crate::meeting_facilitator::derive_meeting_id(&self.started_at, &self.topic);
        let bundle_dir_expected = crate::meeting_facilitator::meeting_bundle_dir(&meeting_id);

        let artifacts = build_handoff_artifacts(
            transcript_path.as_deref(),
            Some(&bundle_dir_expected.to_string_lossy()),
            markdown_report_path.as_deref(),
            &self.applied_templates,
        );

        let partial_effective_goal = self.effective_goal();
        let enrichment = persist::HandoffEnrichment {
            next_owner: next_owner_owned.as_deref(),
            artifacts: artifacts.clone(),
            structured_decisions: Some(structured_decisions.clone()),
            goal: partial_effective_goal.as_deref(),
            next_actor: None,
            applied_templates: self.applied_templates.clone(),
            history_truncated_count: self.history_truncated_count,
            partial_reason: partial_reason.map(|r| r.as_wire_str().to_string()),
            risks: self.explicit_risks.clone(),
            disagreements: self.explicit_disagreements.clone(),
        };

        if let Err(e) = persist::write_handoff_with_explicit(
            &self.topic,
            &summary_text,
            &self.history,
            &action_items,
            &decisions,
            &self.explicit_questions,
            enrichment.clone(),
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

        // Issue #2086: cap bundle transcript to MAX_HANDOFF_HISTORY recent
        // turns. Full transcript already written above.
        let handoff_history: &[super::types::ConversationMessage] =
            if self.history.len() > super::MAX_HANDOFF_HISTORY {
                &self.history[self.history.len() - super::MAX_HANDOFF_HISTORY..]
            } else {
                &self.history
            };
        let bundle_dir = match persist::write_handoff_bundle(
            &self.topic,
            &summary_text,
            Some(&self.started_at),
            handoff_history,
            &action_items,
            &decisions,
            bundle_open_questions,
            themes.clone(),
            participants.clone(),
            enrichment,
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

        // ── Per-meeting `memory_records.json` — issue #2000 fix. ──
        // Mirrors the happy-path write in `close()` so the read companion
        // works even when the close took the partial fast-path.
        if let Some(ref dir) = bundle_dir {
            let bundle_path = std::path::PathBuf::from(dir);
            if let Err(e) = persist::write_meeting_memory_records(
                &bundle_path,
                &self.topic,
                &decisions,
                &action_items,
                &open_questions,
            ) {
                warn!(
                    target: "simard::meeting_backend::closing",
                    phase = "persist_memory_records",
                    outcome = "error",
                    error = %e,
                    handoff_partial = true,
                    reason = PartialReason::PersistenceError.as_wire_str(),
                    "Failed to write partial meeting memory_records.json"
                );
                partial_reason.get_or_insert(PartialReason::PersistenceError);
            }
        }

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
            orphan_turn_count: self.orphan_turn_count,
            risks: self.explicit_risks.clone(),
            disagreements: self.explicit_disagreements.clone(),
        })
    }
}

/// Pick the most-frequent assignee across action items, breaking ties by
/// first-appearance order. Returns `None` when no action item has an
/// `assignee`.
///
/// Used as the fallback for `MeetingHandoff.next_owner` when the operator
/// did not record an explicit `/owner` (issue #1954).
fn most_frequent_action_owner(action_items: &[super::types::HandoffActionItem]) -> Option<String> {
    use std::collections::HashMap;

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut first_seen: Vec<String> = Vec::new();
    for a in action_items {
        if let Some(ref owner) = a.assignee {
            let trimmed = owner.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = trimmed.to_string();
            if !counts.contains_key(&key) {
                first_seen.push(key.clone());
            }
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    let mut best: Option<&String> = None;
    let mut best_count: usize = 0;
    for owner in &first_seen {
        let c = counts.get(owner).copied().unwrap_or(0);
        if c > best_count {
            best_count = c;
            best = Some(owner);
        }
    }
    best.cloned()
}

/// Build the `artifacts[]` payload for a [`MeetingHandoff`] (issue #1954).
///
/// Emits one entry per known artifact source: transcript JSON, per-meeting
/// bundle directory, ad-hoc markdown report, and one entry per applied
/// meeting template (pointing at the bundle so consumers can read the
/// inlined agenda).
fn build_handoff_artifacts(
    transcript_path: Option<&str>,
    bundle_dir: Option<&str>,
    markdown_report_path: Option<&str>,
    applied_templates: &[crate::meeting_backend::types::AppliedTemplate],
) -> Vec<crate::meeting_facilitator::HandoffArtifact> {
    use crate::meeting_facilitator::{
        ARTIFACT_KIND_BUNDLE, ARTIFACT_KIND_MARKDOWN_REPORT, ARTIFACT_KIND_TEMPLATE_AGENDA,
        ARTIFACT_KIND_TRANSCRIPT, HandoffArtifact,
    };

    let mut out: Vec<HandoffArtifact> = Vec::new();
    if let Some(p) = transcript_path {
        out.push(HandoffArtifact {
            kind: ARTIFACT_KIND_TRANSCRIPT.to_string(),
            uri_or_path: p.to_string(),
            description: Some("Meeting transcript JSON".to_string()),
        });
    }
    if let Some(p) = bundle_dir {
        out.push(HandoffArtifact {
            kind: ARTIFACT_KIND_BUNDLE.to_string(),
            uri_or_path: p.to_string(),
            description: Some("Per-meeting handoff bundle directory".to_string()),
        });
    }
    if let Some(p) = markdown_report_path {
        out.push(HandoffArtifact {
            kind: ARTIFACT_KIND_MARKDOWN_REPORT.to_string(),
            uri_or_path: p.to_string(),
            description: Some("Auto-exported meeting report (markdown)".to_string()),
        });
    }
    for tpl in applied_templates {
        out.push(HandoffArtifact {
            kind: ARTIFACT_KIND_TEMPLATE_AGENDA.to_string(),
            uri_or_path: bundle_dir.unwrap_or("").to_string(),
            description: Some(format!("Applied meeting template: {}", tpl.name)),
        });
    }
    out
}

/// Goal-assignment patterns in transcript text that indicate the meeting
/// established or accepted a goal even when no `/decision` was recorded.
const GOAL_PATTERNS: &[&str] = &[
    "new goal",
    "assign goal",
    "accept this goal",
    "accept that goal",
    "goal accepted",
    "goal assigned",
];

/// Synthesize a fallback `MeetingDecision` when `structured_decisions` is
/// empty but the meeting was clearly goal-related (issue #2182).
///
/// Fires when either:
/// - the topic contains "goal" (case-insensitive) and an explicit `/goal`
///   was recorded, OR
/// - the transcript contains goal-assignment patterns and an explicit `/goal`
///   was recorded.
///
/// Uses `explicit_goal` (not `effective_goal`) to avoid accidentally
/// promoting the first user message into an active goal.
fn fallback_goal_decisions(
    explicit_goal: Option<&str>,
    topic: &str,
    history: &[super::types::ConversationMessage],
) -> Vec<crate::meeting_facilitator::MeetingDecision> {
    let goal_text = match explicit_goal {
        Some(g) if !g.trim().is_empty() => g,
        _ => return Vec::new(),
    };

    let topic_lower = topic.to_lowercase();
    let has_goal_topic = topic_lower.contains("goal");

    let has_goal_transcript = history.iter().any(|m| {
        GOAL_PATTERNS
            .iter()
            .any(|p| contains_ignore_ascii_case(&m.content, p))
    });

    if has_goal_topic || has_goal_transcript {
        vec![crate::meeting_facilitator::MeetingDecision {
            description: goal_text.to_string(),
            rationale: format!("[meeting-fallback] inferred from topic '{topic}'"),
            participants: Vec::new(),
        }]
    } else {
        Vec::new()
    }
}

/// Case-insensitive ASCII substring search without allocation.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Write `MeetingDecision` items directly to the goal store (issue #2182).
///
/// Shared by `close()` and `finalize_partial()` so goals are always
/// persisted regardless of whether the summary phase timed out.
///
/// Since issue #1668, writes flow through `CognitiveMemoryGoalStore`
/// instead of the legacy `FileBackedGoalStore`. A one-time migration
/// reads any existing `state/goal_store.json` into cognitive memory
/// before writing new records.
fn write_goals_from_decisions(decisions: &[crate::meeting_facilitator::MeetingDecision]) {
    if decisions.is_empty() {
        return;
    }
    // `CognitiveMemoryGoalStore::put` calls `launch_writer_bridge` with
    // `simard_state_root()` (`$HOME/.simard`), which trips the hermetic
    // guard in test builds. Skip the real write in tests — goal persistence
    // is validated via dedicated integration tests with HermeticState.
    #[cfg(test)]
    {
        let _ = decisions;
        return;
    }
    #[allow(unreachable_code)]
    let state_root = crate::state_root::simard_state_root();

    // One-time migration: if a legacy goal_store.json exists, import it
    // into cognitive memory before writing new records (issue #1668).
    crate::goals::migrate_file_backed_goal_store_if_present(&state_root);

    match crate::goals::CognitiveMemoryGoalStore::new(state_root) {
        Ok(goal_store) => {
            use crate::goals::{GoalRecord as GR, GoalStatus as GS, GoalStore, GoalUpdate};
            let session_id = crate::session::SessionId::from_uuid(uuid::Uuid::now_v7());
            for (i, decision) in decisions.iter().enumerate() {
                let priority = ((i as u8) + 1).min(5);
                let update = match GoalUpdate::new(
                    &decision.description,
                    format!("[meeting] {}", decision.rationale),
                    GS::Active,
                    priority,
                ) {
                    Ok(u) => u,
                    Err(e) => {
                        warn!(
                            target: "simard::meeting_backend::closing",
                            phase = "goal_write",
                            error = %e,
                            "skipping invalid decision for goal store"
                        );
                        continue;
                    }
                };
                let record = match GR::from_update(
                    update,
                    "simard-meeting-close",
                    session_id.clone(),
                    crate::session::SessionPhase::Persistence,
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            target: "simard::meeting_backend::closing",
                            phase = "goal_write",
                            error = %e,
                            "skipping invalid goal record"
                        );
                        continue;
                    }
                };
                if let Err(e) = goal_store.put(record) {
                    warn!(
                        target: "simard::meeting_backend::closing",
                        phase = "goal_write",
                        error = %e,
                        "failed to write goal record to store"
                    );
                }
            }
            info!(
                target: "simard::meeting_backend::closing",
                phase = "goal_write",
                count = decisions.len(),
                "meeting.close wrote goal records directly to store"
            );
        }
        Err(e) => {
            warn!(
                target: "simard::meeting_backend::closing",
                phase = "goal_write",
                error = %e,
                "failed to open goal store for direct writes — goals will be created by OODA curate"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_backend::types::{ConversationMessage, Role};

    fn msg(role: Role, content: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: String::new(),
        }
    }

    #[test]
    fn fallback_goal_decisions_returns_decision_when_topic_contains_goal() {
        let history = vec![msg(Role::User, "Let's discuss goals")];
        let result =
            fallback_goal_decisions(Some("Ship v2 by Friday"), "goal review meeting", &history);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].description, "Ship v2 by Friday");
        assert!(result[0].rationale.contains("meeting-fallback"));
    }

    #[test]
    fn fallback_goal_decisions_returns_decision_when_transcript_mentions_goal_pattern() {
        let history = vec![msg(Role::User, "Let's accept this goal for the sprint")];
        let result =
            fallback_goal_decisions(Some("Improve test coverage"), "sprint planning", &history);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].description, "Improve test coverage");
    }

    #[test]
    fn fallback_goal_decisions_returns_empty_without_explicit_goal() {
        let history = vec![msg(Role::User, "new goal: do stuff")];
        let result = fallback_goal_decisions(None, "goal review", &history);
        assert!(result.is_empty(), "no explicit goal → no fallback");
    }

    #[test]
    fn fallback_goal_decisions_returns_empty_when_topic_unrelated() {
        let history = vec![msg(Role::User, "Let's discuss the roadmap")];
        let result = fallback_goal_decisions(Some("Ship v2"), "roadmap review", &history);
        assert!(
            result.is_empty(),
            "unrelated topic + no patterns → no fallback"
        );
    }

    #[test]
    fn fallback_goal_decisions_returns_empty_for_blank_explicit_goal() {
        let history = vec![msg(Role::User, "new goal: do stuff")];
        let result = fallback_goal_decisions(Some("  "), "goal review", &history);
        assert!(result.is_empty(), "blank explicit goal → no fallback");
    }

    // -----------------------------------------------------------------------
    // Issue #2182: additional fallback coverage
    // -----------------------------------------------------------------------

    #[test]
    fn fallback_goal_decisions_case_insensitive_topic() {
        let history = vec![msg(Role::User, "Let's discuss it")];
        let result = fallback_goal_decisions(Some("Ship v2"), "GOAL Review Meeting", &history);
        assert_eq!(
            result.len(),
            1,
            "uppercase GOAL in topic should still trigger fallback"
        );
    }

    #[test]
    fn fallback_goal_decisions_case_insensitive_transcript_pattern() {
        let history = vec![msg(Role::User, "Let's ACCEPT THIS GOAL now")];
        let result = fallback_goal_decisions(Some("Deploy to prod"), "sprint planning", &history);
        assert_eq!(
            result.len(),
            1,
            "uppercase 'ACCEPT THIS GOAL' in transcript should match"
        );
    }

    #[test]
    fn fallback_goal_decisions_matches_goal_accepted_pattern() {
        let history = vec![msg(Role::User, "That goal accepted by the team")];
        let result = fallback_goal_decisions(Some("Improve latency"), "standup", &history);
        assert_eq!(result.len(), 1, "'goal accepted' pattern should match");
    }

    #[test]
    fn fallback_goal_decisions_matches_goal_assigned_pattern() {
        let history = vec![msg(Role::User, "goal assigned to the backend team")];
        let result = fallback_goal_decisions(Some("Refactor auth"), "planning", &history);
        assert_eq!(result.len(), 1, "'goal assigned' pattern should match");
    }

    #[test]
    fn fallback_goal_decisions_matches_assign_goal_pattern() {
        let history = vec![msg(Role::User, "Let's assign goal to Ryan")];
        let result = fallback_goal_decisions(Some("Ship API v2"), "backlog", &history);
        assert_eq!(result.len(), 1, "'assign goal' pattern should match");
    }

    #[test]
    fn fallback_goal_decisions_rationale_includes_topic() {
        let history = vec![msg(Role::User, "discussing")];
        let topic = "quarterly goal review";
        let result = fallback_goal_decisions(Some("Hire 2 engineers"), topic, &history);
        assert_eq!(result.len(), 1);
        assert!(
            result[0].rationale.contains(topic),
            "rationale should include the topic: {}",
            result[0].rationale,
        );
    }

    #[test]
    fn fallback_goal_decisions_produces_single_decision_not_duplicates() {
        // Both topic and transcript match — should still produce only 1 decision.
        let history = vec![msg(Role::User, "new goal for Q4")];
        let result = fallback_goal_decisions(
            Some("Improve test coverage"),
            "goal planning session",
            &history,
        );
        assert_eq!(
            result.len(),
            1,
            "should produce exactly 1 decision even with multiple signal matches"
        );
    }

    #[test]
    fn fallback_goal_decisions_returns_empty_for_none_explicit_goal() {
        // Even with goal patterns in transcript, None explicit_goal → empty.
        let history = vec![
            msg(Role::User, "new goal: do X"),
            msg(Role::User, "accept this goal"),
        ];
        let result = fallback_goal_decisions(None, "goal review", &history);
        assert!(
            result.is_empty(),
            "None explicit_goal must always return empty, even with matching signals"
        );
    }

    #[test]
    fn decisions_bypass_fallback_when_nonempty() {
        // Models the selection logic at close() line 519-523:
        //   if structured_decisions.is_empty() { fallback } else { decisions }
        let structured_decisions = vec![crate::meeting_facilitator::MeetingDecision {
            description: "Actual decision".to_string(),
            rationale: "Voted on".to_string(),
            participants: vec!["Alice".to_string()],
        }];
        // Even though conditions would trigger fallback...
        let fallback = fallback_goal_decisions(
            Some("Ship v2"),
            "goal review",
            &[msg(Role::User, "new goal for Q4")],
        );
        assert!(
            !fallback.is_empty(),
            "precondition: fallback would fire if called"
        );
        // ...structured_decisions should be preferred
        let decisions_for_goals = if structured_decisions.is_empty() {
            fallback
        } else {
            structured_decisions.clone()
        };
        assert_eq!(decisions_for_goals.len(), 1);
        assert_eq!(
            decisions_for_goals[0].description, "Actual decision",
            "non-empty structured_decisions must bypass fallback"
        );
    }
}
