//! Thin CLI REPL for meeting mode — delegates all logic to `MeetingBackend`.
//!
//! This is a ~80-line stdin/stdout loop. All meeting intelligence, persistence,
//! and memory integration lives in `meeting_backend`.

use std::io::{BufRead, Write};

use crate::base_types::BaseTypeSession;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::persist::{
    extract_action_items, extract_decisions, extract_open_questions,
};
use crate::meeting_backend::{
    MeetingBackend, MeetingCommand, Role, parse_command, strip_ansi_escapes,
};
use crate::meeting_facilitator::MeetingSession;

use super::color::{cyan, green, yellow};
use super::spinner::Spinner;
use super::transcript_format::format_turn_prefix_now;

const PROMPT_TEXT: &str = "simard:meeting> ";

/// Build the live REPL prompt, optionally color-coded.
///
/// The literal text is always `simard:meeting> ` so non-TTY callers and tests
/// can match on it without dealing with ANSI codes; color is applied via the
/// `color::cyan` helper which already honors `NO_COLOR`.
fn prompt_string() -> String {
    cyan(PROMPT_TEXT)
}

/// Render a structured backend-error banner to the operator.
///
/// Emits a stable, greppable `[meeting:error]` marker (matching the
/// `[meeting] handoff written:` convention) so operators can `grep` terminal
/// scrollback reliably. The banner distinguishes *transient* errors (LLM
/// hiccup — meeting still usable) from *permanent* errors (adapter crashed —
/// meeting degraded) using a simple heuristic: `transient` unless the error
/// string contains keywords indicating a non-recoverable condition.
///
/// Issue #1983.
fn render_backend_error<W: Write>(output: &mut W, source: &str, err: &dyn std::fmt::Display) {
    let err_str = err.to_string();

    let is_permanent = err_str.contains("closed")
        || err_str.contains("no longer available")
        || err_str.contains("empty_adapter_response");
    let severity = if is_permanent {
        "permanent"
    } else {
        "transient"
    };
    let hint = if is_permanent {
        "meeting is degraded — /preview to check state, /close to salvage"
    } else {
        "meeting is still usable — retry your message or /close to end"
    };

    writeln!(
        output,
        "{}",
        yellow(&format!(
            "[meeting:error] WARNING: backend error (source={source}, severity={severity}) — {err_str}"
        ))
    )
    .ok();
    writeln!(output, "{}", yellow(&format!("  ↳ {hint}"))).ok();
}

/// Run the interactive meeting REPL.
///
/// Creates a `MeetingBackend` and loops on stdin. Returns a `MeetingSession`
/// for backward compatibility with callers that inspect the closed session.
pub fn run_meeting_repl<R: BufRead, W: Write>(
    topic: &str,
    _bridge: &dyn CognitiveMemoryOps,
    agent: Option<Box<dyn BaseTypeSession>>,
    meeting_system_prompt: &str,
    input: &mut R,
    output: &mut W,
) -> SimardResult<MeetingSession> {
    // Agent is required. No silent degradation to note-taking mode.
    let Some(boxed_agent) = agent else {
        return Err(SimardError::ActionExecutionFailed {
            action: "meeting-repl".to_string(),
            reason: "No LLM agent backend available. Check SIMARD_LLM_PROVIDER and auth config (gh auth status / ANTHROPIC_API_KEY).".to_string(),
        });
    };

    let mut backend =
        MeetingBackend::new_session(topic, boxed_agent, None, meeting_system_prompt.to_string());

    writeln!(
        output,
        "Simard v{} — meeting mode",
        env!("CARGO_PKG_VERSION")
    )
    .ok();
    writeln!(output, "Topic: {topic}").ok();
    writeln!(
        output,
        "Simard is listening. Speak naturally — /help for commands, /close to end.\n"
    )
    .ok();

    let prompt = prompt_string();
    let mut line = String::new();
    loop {
        write!(output, "{prompt}").ok();
        output.flush().ok();

        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => {
                writeln!(output, "\n[EOF] Closing meeting.").ok();
                break;
            }
            Ok(_) => {}
            Err(e) => {
                return Err(SimardError::ActionExecutionFailed {
                    action: "meeting-repl-read".to_string(),
                    reason: e.to_string(),
                });
            }
        }

        match parse_command(&line) {
            MeetingCommand::Help => {
                writeln!(
                    output,
                    "Commands:\n  /status    — show session info\n  /state     — show current decisions, open questions, action items\n  /template  — list meeting templates\n  /template <name> — apply a template (standup, 1on1, retro, planning)\n  /theme <text>    — record a theme for this meeting\n  /decision <text> — record a decision deterministically (skips heuristic extraction)\n  /action <text>   — record an action item (assignee/deadline parsed inline)\n  /question <text> — record an open question deterministically\n  /owner <name>    — name the next agent/persona/human expected to action this handoff\n  /goal <text>     — set the meeting's overarching objective\n  /recap     — show color-coded session recap\n  /preview   — preview the handoff artifact\n  /export    — export meeting as markdown\n  /close     — end meeting and persist\n  /help      — this message\n\nEverything else is natural conversation with Simard."
                )
                .ok();
            }
            MeetingCommand::Status => {
                let status = backend.status();
                writeln!(output, "Meeting: {}", status.topic).ok();
                writeln!(output, "  Messages: {}", status.message_count).ok();
                writeln!(output, "  Started:  {}", status.started_at).ok();
            }
            MeetingCommand::State => {
                // Re-display the running list of decisions, open questions, and
                // action items extracted from the live transcript. Read-only —
                // does not close or summarize the meeting. Reuses the existing
                // extractors in `meeting_backend::persist::extract` (issue #1646).
                //
                // Explicit items recorded inline via `/decision`, `/action`,
                // and `/question` are prepended (issue #1730 seam (b)) so the
                // operator sees them immediately even though they don't land
                // in the conversation history.
                let messages = backend.history();
                let inferred_decisions = extract_decisions(messages);
                let inferred_questions = extract_open_questions(messages);
                let inferred_actions = extract_action_items(messages);

                let mut decisions: Vec<String> = backend.explicit_decisions().to_vec();
                for d in inferred_decisions {
                    let lower = d.to_lowercase();
                    if !decisions.iter().any(|e| e.to_lowercase() == lower) {
                        decisions.push(d);
                    }
                }

                let explicit_q_set: std::collections::HashSet<String> = backend
                    .explicit_questions()
                    .iter()
                    .map(|q| q.to_lowercase())
                    .collect();
                let mut open_questions: Vec<crate::meeting_facilitator::OpenQuestion> = backend
                    .explicit_questions()
                    .iter()
                    .map(|text| crate::meeting_facilitator::OpenQuestion {
                        text: text.clone(),
                        explicit: true,
                    })
                    .collect();
                for q in inferred_questions {
                    let lower = q.text.to_lowercase();
                    if !explicit_q_set.contains(&lower) {
                        open_questions.push(q);
                    }
                }

                let mut action_items: Vec<crate::meeting_backend::HandoffActionItem> =
                    backend.explicit_action_items().to_vec();
                for a in inferred_actions {
                    let lower = a.description.to_lowercase();
                    if !action_items
                        .iter()
                        .any(|e| e.description.to_lowercase() == lower)
                    {
                        action_items.push(a);
                    }
                }

                // Canonical order per task spec: Decisions → Open Questions →
                // Action Items. (Different from the post-/close summary order.)
                writeln!(output, "\n{}", cyan("── Decisions ──")).ok();
                if decisions.is_empty() {
                    writeln!(output, "  _(none)_").ok();
                } else {
                    for (i, d) in decisions.iter().enumerate() {
                        // Sanitize LLM-sourced content (S1): strip ANSI escapes
                        // before rendering to the terminal so a malicious model
                        // can't reposition the cursor or clear the screen.
                        let safe = strip_ansi_escapes(d);
                        writeln!(output, "  {}. {safe}", i + 1).ok();
                    }
                }

                writeln!(output, "\n{}", yellow("── Open Questions ──")).ok();
                if open_questions.is_empty() {
                    writeln!(output, "  _(none)_").ok();
                } else {
                    for q in &open_questions {
                        let safe = strip_ansi_escapes(&q.text);
                        let tag = if q.explicit { " *(explicit)*" } else { "" };
                        writeln!(output, "  - {safe}{tag}").ok();
                    }
                }

                writeln!(output, "\n{}", green("── Action Items ──")).ok();
                if action_items.is_empty() {
                    writeln!(output, "  _(none)_").ok();
                } else {
                    for (i, item) in action_items.iter().enumerate() {
                        let safe_desc = strip_ansi_escapes(&item.description);
                        let mut line = format!("  {}. {safe_desc}", i + 1);
                        if let Some(ref who) = item.assignee {
                            let safe_who = strip_ansi_escapes(who);
                            line.push_str(&format!(" [→ {safe_who}]"));
                        }
                        if let Some(ref when) = item.deadline {
                            let safe_when = strip_ansi_escapes(when);
                            line.push_str(&format!(" ({safe_when})"));
                        }
                        writeln!(output, "{line}").ok();
                    }
                }
                writeln!(output).ok();
            }
            MeetingCommand::Theme(text) => {
                backend.push_theme(text.clone());
                writeln!(output, "{}", green(&format!("Theme recorded: {text}"))).ok();
            }
            MeetingCommand::Decision(text) => {
                backend.push_explicit_decision(&text);
                writeln!(output, "{}", cyan(&format!("Decision recorded: {text}"))).ok();
            }
            MeetingCommand::Action(text) => {
                backend.push_explicit_action_item(&text);
                writeln!(output, "{}", green(&format!("Action recorded: {text}"))).ok();
            }
            MeetingCommand::Question(text) => {
                backend.push_explicit_question(&text);
                writeln!(output, "{}", yellow(&format!("Question recorded: {text}"))).ok();
            }
            MeetingCommand::Owner(text) => {
                backend.push_next_owner(&text);
                writeln!(output, "{}", cyan(&format!("Next owner recorded: {text}"))).ok();
            }
            MeetingCommand::Goal(text) => {
                backend.set_goal(&text);
                writeln!(output, "{}", cyan(&format!("Goal recorded: {text}"))).ok();
            }
            MeetingCommand::Recap => {
                let status = backend.status();
                writeln!(output, "\n── Meeting Recap ──").ok();
                writeln!(output, "Topic: {}", status.topic).ok();
                writeln!(output, "Messages: {}", status.message_count).ok();
                writeln!(output, "Started: {}", status.started_at).ok();
                let themes = backend.explicit_themes();
                if !themes.is_empty() {
                    writeln!(output, "\n{}", cyan("Themes")).ok();
                    for t in themes {
                        writeln!(output, "  - {t}").ok();
                    }
                }
                writeln!(output).ok();
            }
            MeetingCommand::Preview => {
                let status = backend.status();
                let themes = backend.explicit_themes();
                writeln!(output, "\n── Handoff Preview ──").ok();
                writeln!(output, "Topic: {}", status.topic).ok();
                writeln!(output, "Messages so far: {}", status.message_count).ok();
                if themes.is_empty() {
                    writeln!(output, "Themes: (none recorded yet — use /theme <text>)").ok();
                } else {
                    writeln!(output, "\n{}", cyan("Themes")).ok();
                    for t in themes {
                        writeln!(output, "  - {t}").ok();
                    }
                }
                writeln!(
                    output,
                    "\n(Use /close to generate the full handoff artifact.)\n"
                )
                .ok();
            }
            MeetingCommand::Template(name) => {
                use crate::meeting_backend::persist::{TEMPLATES, find_template};
                if name.is_empty() {
                    writeln!(output, "Available templates:").ok();
                    for t in TEMPLATES {
                        writeln!(output, "  {} — {}", t.name, t.description).ok();
                    }
                    writeln!(output, "\nUsage: /template <name>").ok();
                } else if let Some(tmpl) = find_template(&name) {
                    writeln!(output, "\n{}\n", tmpl.agenda).ok();
                    // Record the agenda on the session so /close can include
                    // an `## Agenda` section in the handoff markdown report.
                    backend.apply_template(tmpl.name, tmpl.agenda);
                    // Inject template as context via a message to the backend
                    let ctx = format!(
                        "The operator has selected the '{}' meeting template. \
                         Please follow this agenda:\n{}",
                        tmpl.name, tmpl.agenda
                    );
                    let spinner = Spinner::after_default_delay("Applying template...");
                    match backend.send_message(&ctx) {
                        Ok(resp) => {
                            spinner.stop();
                            if !resp.content.is_empty() {
                                let prefix = format_turn_prefix_now(&Role::Assistant);
                                writeln!(output, "\n{} {}\n", green(&prefix), resp.content).ok();
                            }
                        }
                        Err(e) => {
                            spinner.stop();
                            backend.increment_orphan_turn_count();
                            render_backend_error(output, "template", &e);
                        }
                    }
                } else {
                    writeln!(output, "Unknown template: {name}").ok();
                    writeln!(output, "Available: standup, 1on1, retro, planning").ok();
                }
            }
            MeetingCommand::Export => {
                use crate::meeting_backend::persist::write_markdown_export;
                let spinner = Spinner::after_default_delay("Exporting...");
                match write_markdown_export(
                    backend.topic(),
                    backend.started_at(),
                    backend.history(),
                ) {
                    Ok(path) => {
                        spinner.stop();
                        writeln!(
                            output,
                            "{}",
                            green(&format!("Meeting exported to: {}", path.display()))
                        )
                        .ok();
                    }
                    Err(e) => {
                        spinner.stop();
                        writeln!(output, "{}", yellow(&format!("[export error: {e}]"))).ok();
                    }
                }
            }
            MeetingCommand::Close => {
                writeln!(output, "Closing meeting...").ok();
                // Spinner for the close/summary will be handled below.
                break;
            }
            MeetingCommand::Conversation(text) => {
                if text.is_empty() {
                    continue;
                }
                // Show user turn prefix
                let user_prefix = format_turn_prefix_now(&Role::User);
                writeln!(output, "{} {}", cyan(&user_prefix), text).ok();

                let spinner = Spinner::after_default_delay("Thinking...");
                match backend.send_message(&text) {
                    Ok(resp) => {
                        spinner.stop();
                        if !resp.content.is_empty() {
                            let asst_prefix = format_turn_prefix_now(&Role::Assistant);
                            writeln!(output, "{} {}\n", green(&asst_prefix), resp.content).ok();
                        }
                    }
                    Err(e) => {
                        spinner.stop();
                        backend.increment_orphan_turn_count();
                        render_backend_error(output, "conversation", &e);
                    }
                }
            }
        }
    }

    // Close the backend (summarize, extract action items, persist, memory)
    let spinner = Spinner::after_default_delay("Generating summary...");
    match backend.close() {
        Ok(summary) => {
            spinner.stop();
            // One-line headline summary, shown before the detailed sections
            // and before any handoff/transcript paths. Helps the operator
            // see the bottom-line outcome at a glance.
            let action_count = summary.action_items.len();
            let plural = if action_count == 1 { "" } else { "s" };
            writeln!(
                output,
                "\n{}",
                cyan(&format!(
                    "✓ Meeting closed: \"{}\" — {} action item{}",
                    summary.topic, action_count, plural
                ))
            )
            .ok();
            writeln!(output, "\n── Meeting Summary ──").ok();
            writeln!(output, "{}", summary.summary_text).ok();

            // Display structured action items
            if !summary.action_items.is_empty() {
                writeln!(output, "\n{}", green("── Action Items ──")).ok();
                for (i, item) in summary.action_items.iter().enumerate() {
                    let mut line = format!("  {}. {}", i + 1, item.description);
                    if let Some(ref who) = item.assignee {
                        line.push_str(&format!(" [→ {who}]"));
                    }
                    if let Some(ref when) = item.deadline {
                        line.push_str(&format!(" ({})", when));
                    }
                    if let Some(ref goal) = item.linked_goal {
                        line.push_str(&format!(" 🎯 {goal}"));
                    }
                    writeln!(output, "{line}").ok();
                }
            }

            // Display decisions
            if !summary.decisions.is_empty() {
                writeln!(output, "\n{}", cyan("── Decisions ──")).ok();
                for (i, d) in summary.decisions.iter().enumerate() {
                    writeln!(output, "  {}. {}", i + 1, d).ok();
                }
            }

            // Display open questions
            if !summary.open_questions.is_empty() {
                writeln!(output, "\n{}", yellow("── Open Questions ──")).ok();
                for q in &summary.open_questions {
                    writeln!(output, "  - {q}").ok();
                }
            }

            // Display themes
            if !summary.themes.is_empty() {
                writeln!(output, "\n── Themes ──").ok();
                for t in &summary.themes {
                    writeln!(output, "  - {t}").ok();
                }
            }

            writeln!(
                output,
                "\n{} messages, {}s duration.",
                summary.message_count, summary.duration_secs
            )
            .ok();
            if let Some(ref path) = summary.transcript_path {
                writeln!(output, "{}", green(&format!("Transcript: {path}"))).ok();
            }
            if let Some(ref path) = summary.markdown_report_path {
                writeln!(output, "{}", green(&format!("Report: {path}"))).ok();
            }
            if let Some(ref dir) = summary.bundle_dir {
                writeln!(
                    output,
                    "{}",
                    green(&format!(
                        "Handoff bundle: {dir}\n  - meeting_handoff.json (structured artifact)\n  - meeting_handoff.md (human-readable)\n  - transcript.json (full conversation)"
                    ))
                )
                .ok();
                // Stable, machine-grep-friendly bundle line for operator
                // scrollback (see
                // `docs/reference/meeting-close-lifecycle.md#repl-exit-banner`).
                writeln!(output, "{}", green(&format!("[meeting] bundle: {dir}"))).ok();
            }
            // Operator-facing handoff path line. Stable literal so
            // operators can `grep '\[meeting\] handoff written:' …`
            // from terminal scrollback (see
            // `docs/reference/meeting-close-lifecycle.md#repl-exit-banner`).
            let handoff_path = crate::meeting_facilitator::default_handoff_dir()
                .join(crate::meeting_facilitator::MEETING_HANDOFF_FILENAME);
            writeln!(
                output,
                "{}",
                green(&format!(
                    "[meeting] handoff written: {}",
                    handoff_path.display()
                ))
            )
            .ok();
            // Partial-close banner: only printed when the close
            // pipeline took a bounded-timeout fast-path (issue #1908).
            // The literal prefix `[meeting] WARNING: partial close
            // (reason=<wire>)` is contractual — change only with a
            // simultaneous update to the howto under
            // `docs/howto/recover-from-meeting-close-timeout.md`.
            if let Some(reason) = summary.partial_reason {
                writeln!(
                    output,
                    "{}",
                    yellow(&format!(
                        "[meeting] WARNING: partial close (reason={}). Review the bundle before relying on extracted decisions/action items.",
                        reason.as_wire_str()
                    ))
                )
                .ok();
            }
            // Orphan-turn banner: printed when at least one
            // `send_message` failed after the user message was already
            // pushed to history, leaving turns with no assistant reply
            // in the transcript. Issue #1983.
            if summary.orphan_turn_count > 0 {
                let plural = if summary.orphan_turn_count == 1 {
                    "turn has"
                } else {
                    "turns have"
                };
                writeln!(
                    output,
                    "{}",
                    yellow(&format!(
                        "[meeting] WARNING: {} orphan {} no assistant reply (backend errors during conversation). Transcript may be incomplete.",
                        summary.orphan_turn_count, plural
                    ))
                )
                .ok();
            }
        }
        Err(e) => {
            // spinner dropped here, which also cleans up
            writeln!(
                output,
                "{}",
                yellow(&format!("[warn] Failed to close meeting cleanly: {e}"))
            )
            .ok();
        }
    }

    // Return a compatible MeetingSession for callers that need it.
    Ok(empty_closed_session(topic))
}

/// Produce an empty closed `MeetingSession` for backward compatibility.
fn empty_closed_session(topic: &str) -> MeetingSession {
    use crate::meeting_facilitator::MeetingSessionStatus;
    MeetingSession {
        topic: topic.to_string(),
        decisions: Vec::new(),
        action_items: Vec::new(),
        notes: Vec::new(),
        status: MeetingSessionStatus::Closed,
        started_at: chrono::Utc::now().to_rfc3339(),
        participants: vec!["operator".to_string()],
        explicit_questions: Vec::new(),
        themes: Vec::new(),
        next_owner: None,
        goal: None,
    }
}
