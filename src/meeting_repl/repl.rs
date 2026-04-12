//! Thin CLI REPL for meeting mode — delegates all logic to `MeetingBackend`.
//!
//! This is a ~80-line stdin/stdout loop. All meeting intelligence, persistence,
//! and memory integration lives in `meeting_backend`.

use std::io::{BufRead, Write};

use crate::base_types::BaseTypeSession;
use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::{MeetingBackend, MeetingCommand, parse_command};
use crate::meeting_facilitator::MeetingSession;
use crate::memory_bridge::CognitiveMemoryBridge;

const PROMPT: &str = "simard:meeting> ";

/// Run the interactive meeting REPL.
///
/// Creates a `MeetingBackend` and loops on stdin. Returns a `MeetingSession`
/// for backward compatibility with callers that inspect the closed session.
pub fn run_meeting_repl<R: BufRead, W: Write>(
    topic: &str,
    bridge: &CognitiveMemoryBridge,
    agent: Option<Box<dyn BaseTypeSession>>,
    meeting_system_prompt: &str,
    input: &mut R,
    output: &mut W,
) -> SimardResult<MeetingSession> {
    // If no agent is available, run a minimal note-taking fallback
    let Some(boxed_agent) = agent else {
        return run_noteonly_fallback(topic, bridge, input, output);
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
                    "Commands:\n  /status  — show session info\n  /close   — end meeting and persist\n  /help    — this message\n\nEverything else is natural conversation with Simard."
                )
                .ok();
            }
            MeetingCommand::Status => {
                let status = backend.status();
                writeln!(output, "Meeting: {}", status.topic).ok();
                writeln!(output, "  Messages: {}", status.message_count).ok();
                writeln!(output, "  Started:  {}", status.started_at).ok();
            }
            MeetingCommand::Close => {
                writeln!(output, "Closing meeting.").ok();
                break;
            }
            MeetingCommand::Conversation(text) => {
                if text.is_empty() {
                    continue;
                }
                eprint!("  ⏳ Thinking...");
                match backend.send_message(&text) {
                    Ok(resp) => {
                        eprintln!("\r                "); // clear the thinking indicator
                        if !resp.content.is_empty() {
                            writeln!(output, "\n{}\n", resp.content).ok();
                        }
                    }
                    Err(e) => {
                        eprintln!("\r                "); // clear the thinking indicator
                        writeln!(output, "[agent error: {e}]").ok();
                    }
                }
            }
        }
    }

    // Close the backend (summarize, persist, memory)
    match backend.close() {
        Ok(summary) => {
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
            writeln!(output, "[warn] Failed to close meeting cleanly: {e}").ok();
        }
    }

    // Return a compatible MeetingSession for callers that need it.
    Ok(empty_closed_session(topic))
}

/// Minimal fallback when no agent is available — just records notes.
fn run_noteonly_fallback<R: BufRead, W: Write>(
    topic: &str,
    _bridge: &CognitiveMemoryBridge,
    input: &mut R,
    output: &mut W,
) -> SimardResult<MeetingSession> {
    use crate::meeting_facilitator::{add_note, close_meeting, start_meeting};

    let mut session = start_meeting(topic, _bridge)?;

    writeln!(
        output,
        "Simard v{} — meeting mode (note-taking only, no agent backend)",
        env!("CARGO_PKG_VERSION")
    )
    .ok();
    writeln!(output, "Topic: {topic}\n").ok();

    let mut line = String::new();
    loop {
        write!(output, "{PROMPT}").ok();
        output.flush().ok();

        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(SimardError::ActionExecutionFailed {
                    action: "meeting-repl-read".to_string(),
                    reason: e.to_string(),
                });
            }
        }

        let trimmed = line.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "/close" | "/done" => break,
            "/help" => {
                writeln!(
                    output,
                    "Note-taking mode: type anything to add notes, /close to end."
                )
                .ok();
            }
            "/status" => {
                writeln!(output, "Meeting: {} — {} notes", topic, session.notes.len()).ok();
            }
            _ => {
                if !trimmed.is_empty() {
                    add_note(&mut session, trimmed).ok();
                    writeln!(output, "Note added.").ok();
                }
            }
        }
    }

    let closed = close_meeting(session, _bridge)?;
    Ok(closed)
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
    fn repl_no_agent_fallback() {
        let bridge = mock_bridge();
        let input = b"some note\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("No-agent test", &bridge, None, "", &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("note-taking only"),
            "should indicate note-taking mode: {output_str}"
        );
    }
}
