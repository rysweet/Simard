//! Core meeting REPL loop, dispatch, and banner.

use std::io::{BufRead, Write};

use crate::base_types::{BaseTypeSession, BaseTypeTurnInput};
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingSession, add_note, close_meeting, edit_item,
    record_action_item, record_decision, remove_item, start_meeting,
};
use crate::memory_bridge::CognitiveMemoryBridge;

use super::auto_capture::auto_capture_structured_items;
use super::command::{HELP_TEXT, MeetingCommand, parse_meeting_command};
use super::persist::{persist_meeting_to_memory, write_meeting_handoff_artifact};

const PROMPT: &str = "simard:meeting> ";

/// Run the interactive meeting REPL.
///
/// When `agent` is `Some`, natural-language lines are forwarded to the base-type
/// session via `run_turn` and the agent's response is displayed.
///
/// When `agent` is `None`, natural language is recorded as notes (fallback).
pub fn run_meeting_repl<R: BufRead, W: Write>(
    topic: &str,
    bridge: &CognitiveMemoryBridge,
    agent: Option<&mut dyn BaseTypeSession>,
    meeting_system_prompt: &str,
    input: &mut R,
    output: &mut W,
) -> SimardResult<MeetingSession> {
    let mut session = start_meeting(topic, bridge)?;
    let mut agent = agent;

    print_banner(topic, agent.is_some(), output);

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

        let cmd = parse_meeting_command(&line);
        if matches!(cmd, MeetingCommand::Close) {
            // Print duration before closing
            if let Ok(start) = chrono::DateTime::parse_from_rfc3339(&session.started_at) {
                let secs = chrono::Utc::now()
                    .signed_duration_since(start)
                    .num_seconds();
                writeln!(output, "Meeting duration: {secs}s").ok();
            }
            writeln!(output, "Closing meeting.").ok();
            break;
        }
        dispatch_command(
            cmd,
            &mut session,
            &mut agent,
            topic,
            meeting_system_prompt,
            output,
        );
    }

    let closed = close_meeting(session, bridge)?;
    let summary = closed.durable_summary();
    writeln!(output, "Meeting record: {summary}").ok();

    write_meeting_handoff_artifact(&closed, output);
    persist_meeting_to_memory(&closed, bridge, output);

    Ok(closed)
}

fn print_banner<W: Write>(topic: &str, has_agent: bool, output: &mut W) {
    writeln!(
        output,
        "Simard v{} — meeting mode",
        env!("CARGO_PKG_VERSION")
    )
    .ok();
    writeln!(output, "Topic: {topic}").ok();
    if has_agent {
        writeln!(
            output,
            "Simard is listening. Speak naturally — /help for commands, /close to end."
        )
    } else {
        writeln!(
            output,
            "Note-taking mode (no agent backend). /help for commands, /close to end."
        )
    }
    .ok();
    writeln!(output).ok();
}

fn dispatch_command<W: Write>(
    cmd: MeetingCommand,
    session: &mut MeetingSession,
    agent: &mut Option<&mut dyn BaseTypeSession>,
    topic: &str,
    meeting_system_prompt: &str,
    output: &mut W,
) {
    match cmd {
        MeetingCommand::Decision {
            description,
            rationale,
        } => {
            let decision = MeetingDecision {
                description: description.clone(),
                rationale,
                participants: Vec::new(),
            };
            match record_decision(session, decision) {
                Ok(()) => writeln!(output, "Recorded decision: {description}").ok(),
                Err(e) => writeln!(output, "Error: {e}").ok(),
            };
        }
        MeetingCommand::Action {
            description,
            owner,
            priority,
        } => {
            let item = ActionItem {
                description: description.clone(),
                owner: owner.clone(),
                priority,
                due_description: None,
            };
            match record_action_item(session, item) {
                Ok(()) => writeln!(output, "Recorded action: {description} (owner={owner})").ok(),
                Err(e) => writeln!(output, "Error: {e}").ok(),
            };
        }
        MeetingCommand::Note(text) => match add_note(session, &text) {
            Ok(()) => {
                writeln!(output, "Note added.").ok();
            }
            Err(e) => {
                writeln!(output, "Error: {e}").ok();
            }
        },
        MeetingCommand::Conversation(text) => {
            if let Some(agent_session) = agent {
                let turn_input = BaseTypeTurnInput {
                    objective: text.clone(),
                    identity_context: meeting_system_prompt.to_string(),
                    prompt_preamble: format!("Meeting topic: {topic}"),
                };
                match agent_session.run_turn(turn_input) {
                    Ok(outcome) => {
                        let response = outcome.execution_summary.trim();
                        writeln!(output, "\n{response}\n").ok();
                        add_note(session, &format!("operator: {text}")).ok();
                        add_note(session, &format!("simard: {response}")).ok();
                        auto_capture_structured_items(session, &text, response, output);
                    }
                    Err(e) => {
                        writeln!(output, "[agent error: {e}]").ok();
                        add_note(session, &text).ok();
                    }
                }
            } else {
                add_note(session, &text).ok();
                writeln!(output, "Note added.").ok();
            }
        }
        MeetingCommand::Close => unreachable!(),
        MeetingCommand::Help => {
            write!(output, "{HELP_TEXT}").ok();
        }
        MeetingCommand::List => {
            let has_items = !session.decisions.is_empty()
                || !session.action_items.is_empty()
                || !session.notes.is_empty();
            if !has_items {
                writeln!(output, "No items recorded yet.").ok();
            } else {
                if !session.decisions.is_empty() {
                    writeln!(output, "Decisions:").ok();
                    for (i, d) in session.decisions.iter().enumerate() {
                        writeln!(output, "  {}. {}", i + 1, d.description).ok();
                    }
                }
                if !session.action_items.is_empty() {
                    writeln!(output, "Action items:").ok();
                    for (i, a) in session.action_items.iter().enumerate() {
                        writeln!(output, "  {}. {} (owner={})", i + 1, a.description, a.owner).ok();
                    }
                }
                if !session.notes.is_empty() {
                    writeln!(output, "Notes:").ok();
                    for (i, n) in session.notes.iter().enumerate() {
                        writeln!(output, "  {}. {}", i + 1, n).ok();
                    }
                }
            }
        }
        MeetingCommand::Edit {
            item_type,
            index,
            new_text,
        } => match edit_item(session, &item_type, index, &new_text) {
            Ok(()) => {
                writeln!(output, "Updated {item_type} {}.", index + 1).ok();
            }
            Err(e) => {
                writeln!(output, "Error: {e}").ok();
            }
        },
        MeetingCommand::Delete { item_type, index } => {
            match remove_item(session, &item_type, index) {
                Ok(()) => {
                    writeln!(output, "Deleted {item_type} {}.", index + 1).ok();
                }
                Err(e) => {
                    writeln!(output, "Error: {e}").ok();
                }
            }
        }
        MeetingCommand::Status => {
            let elapsed = if !session.started_at.is_empty() {
                if let Ok(start) = chrono::DateTime::parse_from_rfc3339(&session.started_at) {
                    let secs = chrono::Utc::now()
                        .signed_duration_since(start)
                        .num_seconds();
                    format!("{secs}s")
                } else {
                    "unknown".to_string()
                }
            } else {
                "unknown".to_string()
            };
            writeln!(output, "Meeting: {}", topic).ok();
            writeln!(output, "  Elapsed:     {elapsed}").ok();
            writeln!(output, "  Decisions:   {}", session.decisions.len()).ok();
            writeln!(output, "  Actions:     {}", session.action_items.len()).ok();
            writeln!(output, "  Notes:       {}", session.notes.len()).ok();
            writeln!(output, "  Participants: {}", session.participants.len()).ok();
        }
        MeetingCommand::AddParticipant(name) => {
            if !session.participants.contains(&name) {
                session.participants.push(name.clone());
            }
            writeln!(output, "Participant added: {name}").ok();
        }
        MeetingCommand::ListParticipants => {
            if session.participants.is_empty() {
                writeln!(output, "No participants recorded yet.").ok();
            } else {
                writeln!(output, "Participants:").ok();
                for p in &session.participants {
                    writeln!(output, "  - {p}").ok();
                }
            }
        }
        MeetingCommand::Empty => {}
        MeetingCommand::Unknown(input) => {
            writeln!(output, "Could not parse command: {input}").ok();
            writeln!(
                output,
                "Try /help for command syntax, or just type naturally."
            )
            .ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{MockAgentSession, mock_bridge};
    use super::*;
    use crate::meeting_facilitator::MeetingSessionStatus;

    #[test]
    fn repl_records_decision_and_closes() {
        let bridge = mock_bridge();
        let input = b"/decision Ship it | Ready for production\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl(
            "Sprint planning",
            &bridge,
            None,
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.decisions.len(), 1);
        assert_eq!(session.decisions[0].description, "Ship it");
    }

    #[test]
    fn repl_records_action_item_and_closes() {
        let bridge = mock_bridge();
        let input = b"/action Write tests | bob | 2\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Retro", &bridge, None, "", &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.action_items.len(), 1);
        assert_eq!(session.action_items[0].owner, "bob");
    }

    #[test]
    fn repl_shows_help() {
        let bridge = mock_bridge();
        let input = b"/help\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Help test", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("/decision"));
        assert!(output_str.contains("/close"));
    }

    #[test]
    fn repl_natural_language_without_agent_falls_back_to_note() {
        let bridge = mock_bridge();
        let input = b"Hello tell me about projects\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Test", &bridge, None, "", &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Note added"));
    }

    #[test]
    fn repl_sends_natural_language_to_agent() {
        let bridge = mock_bridge();
        let mut agent = MockAgentSession::new("Hello! I'm Simard, ready to discuss your project.");
        let input = b"Hello Simard\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl(
            "Test conversation",
            &bridge,
            Some(&mut agent),
            "You are Simard in meeting mode.",
            &mut reader,
            &mut output,
        )
        .unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Hello! I'm Simard"),
            "agent response should be displayed: {output_str}"
        );
        assert!(
            session
                .notes
                .iter()
                .any(|n| n.contains("operator: Hello Simard"))
        );
        assert!(
            session
                .notes
                .iter()
                .any(|n| n.contains("simard: Hello! I'm Simard"))
        );
    }

    #[test]
    fn repl_slash_commands_bypass_agent() {
        let bridge = mock_bridge();
        let mut agent = MockAgentSession::new("Agent response");
        let input = b"/note This is an explicit note\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl(
            "Test",
            &bridge,
            Some(&mut agent),
            "",
            &mut reader,
            &mut output,
        )
        .unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Note added"));
        assert!(!output_str.contains("Agent response"));
        assert_eq!(session.notes, vec!["This is an explicit note"]);
    }

    #[test]
    fn repl_list_shows_items_grouped() {
        let bridge = mock_bridge();
        let input =
            b"/decision Ship it | Ready\n/action Write tests | bob\n/note Remember CI\n/list\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("List test", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Decisions:"));
        assert!(output_str.contains("1. Ship it"));
        assert!(output_str.contains("Action items:"));
        assert!(output_str.contains("1. Write tests (owner=bob)"));
        assert!(output_str.contains("Notes:"));
        assert!(output_str.contains("1. Remember CI"));
    }

    #[test]
    fn repl_list_empty() {
        let bridge = mock_bridge();
        let input = b"/list\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Empty", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("No items recorded yet."));
    }

    #[test]
    fn repl_edit_decision() {
        let bridge = mock_bridge();
        let input = b"/decision Old text | reason\n/edit decision 1 New text\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Edit", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Updated decision 1."));
        assert_eq!(session.decisions[0].description, "New text");
    }

    #[test]
    fn repl_delete_action() {
        let bridge = mock_bridge();
        let input = b"/action Task one | alice\n/action Task two | bob\n/delete action 1\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Delete", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Deleted action 1."));
        assert_eq!(session.action_items.len(), 1);
        assert_eq!(session.action_items[0].description, "Task two");
    }

    #[test]
    fn repl_edit_out_of_bounds_shows_error() {
        let bridge = mock_bridge();
        let input = b"/edit decision 1 text\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("OOB", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Error:"));
        assert!(output_str.contains("out of range"));
    }

    #[test]
    fn repl_delete_out_of_bounds_shows_error() {
        let bridge = mock_bridge();
        let input = b"/delete note 5\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("OOB", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Error:"));
        assert!(output_str.contains("out of range"));
    }

    #[test]
    fn repl_help_includes_new_commands() {
        let bridge = mock_bridge();
        let input = b"/help\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Help", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("/list"));
        assert!(output_str.contains("/edit"));
        assert!(output_str.contains("/delete"));
    }
}
