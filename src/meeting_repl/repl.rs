//! Thin CLI REPL for meeting mode — delegates all logic to `MeetingBackend`.
//!
//! This is a ~80-line stdin/stdout loop. All meeting intelligence, persistence,
//! and memory integration lives in `meeting_backend`.

use std::io::{BufRead, Write};

use crate::base_types::BaseTypeSession;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::{MeetingBackend, MeetingCommand, parse_command};
use crate::meeting_facilitator::MeetingSession;

use super::color::{cyan, green, yellow};
use super::spinner::Spinner;

const PROMPT: &str = "simard:meeting> ";

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
                    "Commands:\n  /status    — show session info\n  /template  — list meeting templates\n  /template <name> — apply a template (standup, 1on1, retro, planning)\n  /theme <text>    — record a theme for this meeting\n  /recap     — show color-coded session recap\n  /preview   — preview the handoff artifact\n  /export    — export meeting as markdown\n  /close     — end meeting and persist\n  /help      — this message\n\nEverything else is natural conversation with Simard."
                )
                .ok();
            }
            MeetingCommand::Status => {
                let status = backend.status();
                writeln!(output, "Meeting: {}", status.topic).ok();
                writeln!(output, "  Messages: {}", status.message_count).ok();
                writeln!(output, "  Started:  {}", status.started_at).ok();
            }
            MeetingCommand::Theme(text) => {
                backend.push_theme(text.clone());
                writeln!(output, "{}", green(&format!("Theme recorded: {text}"))).ok();
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
                                writeln!(output, "{}\n", resp.content).ok();
                            }
                        }
                        Err(e) => {
                            spinner.stop();
                            writeln!(output, "[agent error: {e}]").ok();
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
                let spinner = Spinner::after_default_delay("Thinking...");
                match backend.send_message(&text) {
                    Ok(resp) => {
                        spinner.stop();
                        if !resp.content.is_empty() {
                            writeln!(output, "\n{}\n", resp.content).ok();
                        }
                    }
                    Err(e) => {
                        spinner.stop();
                        writeln!(output, "{}", yellow(&format!("[agent error: {e}]"))).ok();
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

    #[test]
    #[serial]
    fn repl_theme_command_records_theme() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("noted");
        let input = b"/theme performance\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl(
            "Theme test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Theme recorded: performance"),
            "should confirm theme: {output_str}"
        );
    }

    #[test]
    #[serial]
    fn repl_recap_shows_session_info() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("ok");
        let input = b"/theme scalability\n/recap\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl(
            "Recap test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Meeting Recap"),
            "should show recap: {output_str}"
        );
        assert!(
            output_str.contains("scalability"),
            "recap should include recorded theme: {output_str}"
        );
    }

    #[test]
    #[serial]
    fn repl_preview_shows_handoff_preview() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("ok");
        let input = b"/preview\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl(
            "Preview test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Handoff Preview"),
            "should show preview: {output_str}"
        );
    }

    #[test]
    #[serial]
    fn repl_help_includes_theme_recap_preview() {
        let bridge = mock_bridge();
        let agent = MockAgentSession::new("ok");
        let input = b"/help\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl(
            "Help extended test",
            &bridge,
            Some(Box::new(agent)),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("/theme"),
            "help should mention /theme: {output_str}"
        );
        assert!(
            output_str.contains("/recap"),
            "help should mention /recap: {output_str}"
        );
        assert!(
            output_str.contains("/preview"),
            "help should mention /preview: {output_str}"
        );
    }
}
