//! Thin CLI REPL for meeting mode — delegates all logic to `MeetingBackend`.
//!
//! This is a ~80-line stdin/stdout loop. All meeting intelligence, persistence,
//! and memory integration lives in `meeting_backend`.

use std::io::{BufRead, Write};

use crate::base_types::BaseTypeSession;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::{
    MeetingBackend, MeetingCommand, find_template, format_template, list_templates, parse_command,
};
use crate::meeting_facilitator::MeetingSession;

const PROMPT: &str = "simard:meeting> ";

/// Spinner frames for progress indication during LLM calls.
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn show_progress(label: &str) {
    eprint!("  {} {}...", SPINNER[0], label);
}

fn clear_progress() {
    eprint!("\r\x1b[K");
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

    let mut line = String::new();
    loop {
        write!(output, "{PROMPT}").ok();
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
                    "Commands:\n  /status          — show session info\n  /progress        — show meeting progress (duration, topics, action items)\n  /export          — save transcript as markdown\n  /template <name> — load a meeting template (standup, retro, planning, 1on1, bug-triage)\n  /close           — end meeting and persist\n  /help            — this message\n\nEverything else is natural conversation with Simard."
                )
                .ok();
            }
            MeetingCommand::Status => {
                let status = backend.status();
                writeln!(output, "Meeting: {}", status.topic).ok();
                writeln!(output, "  Messages: {}", status.message_count).ok();
                writeln!(output, "  Started:  {}", status.started_at).ok();
                if let Some(ref dur) = status.duration_display {
                    writeln!(output, "  Meeting duration: {dur}").ok();
                }
                if let Some(ref tmpl) = status.active_template {
                    writeln!(output, "  Template: {tmpl}").ok();
                }
            }
            MeetingCommand::Progress => {
                let p = backend.progress();
                writeln!(output, "── Meeting Progress ──").ok();
                writeln!(output, "  Duration:   {}", p.duration_display).ok();
                writeln!(
                    output,
                    "  Messages:   {} operator, {} agent",
                    p.operator_messages, p.agent_messages
                )
                .ok();
                if !p.topics.is_empty() {
                    writeln!(output, "  Topics discussed:").ok();
                    for t in &p.topics {
                        writeln!(output, "    • {t}").ok();
                    }
                }
                if !p.action_items.is_empty() {
                    writeln!(output, "  Action items:").ok();
                    for a in &p.action_items {
                        writeln!(output, "    ☐ {a}").ok();
                    }
                }
                if !p.pending_decisions.is_empty() {
                    writeln!(output, "  Pending decisions:").ok();
                    for d in &p.pending_decisions {
                        writeln!(output, "    ⚑ {d}").ok();
                    }
                }
                if p.topics.is_empty()
                    && p.action_items.is_empty()
                    && p.pending_decisions.is_empty()
                {
                    writeln!(
                        output,
                        "  No topics, action items, or decisions extracted yet."
                    )
                    .ok();
                }
            }
            MeetingCommand::Close => {
                writeln!(output, "Closing meeting.").ok();
                break;
            }
            MeetingCommand::Export => match backend.export_markdown() {
                Ok(path) => {
                    writeln!(output, "✓ Markdown exported: {}", path.display()).ok();
                }
                Err(e) => {
                    writeln!(output, "[export error: {e}]").ok();
                }
            },
            MeetingCommand::Template(name) => {
                if name.is_empty() {
                    writeln!(output, "{}", list_templates()).ok();
                } else if let Some(tmpl) = find_template(&name) {
                    writeln!(output, "{}", format_template(tmpl)).ok();
                    show_progress("Loading template");
                    match backend.send_message(tmpl.opening_prompt) {
                        Ok(resp) => {
                            clear_progress();
                            if !resp.content.is_empty() {
                                writeln!(output, "\n{}\n", resp.content).ok();
                            }
                        }
                        Err(e) => {
                            clear_progress();
                            writeln!(output, "[agent error: {e}]").ok();
                        }
                    }
                } else {
                    writeln!(output, "Unknown template: '{name}'").ok();
                    writeln!(output, "{}", list_templates()).ok();
                }
            }
            MeetingCommand::Conversation(text) => {
                if text.is_empty() {
                    continue;
                }
                show_progress("Thinking");
                match backend.send_message(&text) {
                    Ok(resp) => {
                        clear_progress();
                        if !resp.content.is_empty() {
                            writeln!(output, "\n{}\n", resp.content).ok();
                        }
                        // Brief inline progress after each turn
                        writeln!(
                            output,
                            "  [{} messages · {}]",
                            backend.message_count(),
                            backend.elapsed_display(),
                        )
                        .ok();
                    }
                    Err(e) => {
                        clear_progress();
                        writeln!(output, "[agent error: {e}]").ok();
                    }
                }
            }
        }
    }

    // Close the backend (summarize, persist, memory)
    show_progress("Generating summary");
    match backend.close() {
        Ok(summary) => {
            clear_progress();
            writeln!(output, "\n── Meeting Summary ──").ok();
            writeln!(output, "{}", summary.summary_text).ok();
            writeln!(
                output,
                "\n{} messages, {}s duration.",
                summary.message_count, summary.duration_secs
            )
            .ok();
            if let Some(ref path) = summary.transcript_path {
                writeln!(output, "Transcript: {path}").ok();
            }
        }
        Err(e) => {
            clear_progress();
            writeln!(output, "[warn] Failed to close meeting cleanly: {e}").ok();
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
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{MockAgentSession, mock_bridge};
    use super::*;
    use crate::meeting_facilitator::MeetingSessionStatus;
    use serial_test::serial;

    #[test]
    #[serial]
    fn repl_sends_message_and_closes() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("I understand the concern.");
        let input = b"Let's discuss testing\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl(
            "Sprint planning",
            &bridge,
            Some(Box::new(agent)),
            "You are Simard.",
            &mut reader,
            &mut output,
        )
        .unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("I understand the concern"),
            "should show agent response: {output_str}"
        );
    }

    #[test]
    #[serial]
    fn repl_shows_help() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("ok");
        let input = b"/help\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl(
            "Help test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("/status"),
            "help should mention /status: {output_str}"
        );
        assert!(
            output_str.contains("/close"),
            "help should mention /close: {output_str}"
        );
    }

    #[test]
    #[serial]
    fn repl_shows_status() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("noted");
        let input = b"First message\n/status\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl(
            "Status test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Messages: 2"),
            "status should show 2 messages: {output_str}"
        );
    }

    #[test]
    #[serial]
    fn repl_eof_closes() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("ok");
        let input = b"just one line\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl(
            "EOF test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
    }

    #[test]
    #[serial]
    fn repl_no_agent_returns_error() {
        let bridge = mock_bridge();
        let input = b"some note\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let result = run_meeting_repl("No-agent test", &bridge, None, "", &mut reader, &mut output);

        assert!(
            result.is_err(),
            "should fail without agent, not silently degrade"
        );
    }
}
