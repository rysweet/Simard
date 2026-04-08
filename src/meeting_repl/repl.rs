//! Core meeting REPL loop, dispatch, and banner.

use std::io::{BufRead, Write};

use crate::base_types::{BaseTypeSession, BaseTypeTurnInput};
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingHandoff, MeetingSession, add_note, add_question,
    close_meeting, default_handoff_dir, edit_item, load_session_wip, record_action_item,
    record_decision, remove_item, remove_session_wip, save_session_wip, start_meeting,
};
use crate::memory_bridge::CognitiveMemoryBridge;

use super::auto_capture::auto_capture_structured_items;
use super::command::{MeetingCommand, help_text, parse_meeting_command};
use super::persist::{persist_meeting_to_memory, write_meeting_handoff_artifact};

const PROMPT: &str = "simard:meeting> ";

/// Auto-save interval for periodic WIP snapshots.
const AUTO_SAVE_INTERVAL_SECS: u64 = 60;

/// Save session WIP, logging warnings on failure without aborting.
fn try_save_wip<W: Write>(session: &MeetingSession, handoff_dir: &std::path::Path, output: &mut W) {
    if let Err(e) = save_session_wip(handoff_dir, session) {
        writeln!(output, "[warn] Auto-save failed: {e}").ok();
    }
}

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
    let handoff_dir = default_handoff_dir();

    // --- Resume logic: check for a WIP session on disk ---
    let mut session = match load_session_wip(&handoff_dir) {
        Ok(Some(wip)) => {
            writeln!(
                output,
                "Found a previous session (topic: \"{}\"). Resume previous session? (y/n)",
                wip.topic
            )
            .ok();
            write!(output, "> ").ok();
            output.flush().ok();

            let mut answer = String::new();
            let resumed = match input.read_line(&mut answer) {
                Ok(n) if n > 0 => answer.trim().eq_ignore_ascii_case("y"),
                _ => false,
            };

            if resumed {
                writeln!(output, "Resuming session: \"{}\"", wip.topic).ok();
                wip
            } else {
                writeln!(output, "Starting fresh session.").ok();
                remove_session_wip(&handoff_dir).ok();
                start_meeting(topic, bridge)?
            }
        }
        _ => start_meeting(topic, bridge)?,
    };

    let mut agent = agent;

    print_banner(&session.topic, agent.is_some(), output);

    // Initial WIP save so even a very early crash is recoverable.
    try_save_wip(&session, &handoff_dir, output);

    let mut last_save = std::time::Instant::now();
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

        let is_state_changing = !matches!(
            cmd,
            MeetingCommand::Empty
                | MeetingCommand::Help
                | MeetingCommand::Status
                | MeetingCommand::Recap
                | MeetingCommand::List
                | MeetingCommand::ListParticipants
                | MeetingCommand::Preview
        );

        let current_topic = session.topic.clone();
        dispatch_command(
            cmd,
            &mut session,
            &mut agent,
            &current_topic,
            meeting_system_prompt,
            output,
        );

        // Save WIP after state-changing commands.
        if is_state_changing {
            try_save_wip(&session, &handoff_dir, output);
            last_save = std::time::Instant::now();
        }

        // Periodic auto-save (60 s).
        if last_save.elapsed().as_secs() >= AUTO_SAVE_INTERVAL_SECS {
            try_save_wip(&session, &handoff_dir, output);
            last_save = std::time::Instant::now();
        }
    }

    let closed = close_meeting(session, bridge)?;
    print_recap("Meeting Closed", &closed, output);
    writeln!(output).ok();

    write_meeting_handoff_artifact(&closed, output);
    persist_meeting_to_memory(&closed, bridge, output);

    // Clean up WIP file after successful close.
    remove_session_wip(&handoff_dir).ok();

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

fn format_elapsed(started_at: &str) -> String {
    if started_at.is_empty() {
        return "unknown".to_string();
    }
    match chrono::DateTime::parse_from_rfc3339(started_at) {
        Ok(start) => {
            let total_secs = chrono::Utc::now()
                .signed_duration_since(start)
                .num_seconds()
                .max(0);
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            if mins > 0 {
                format!("{mins}m {secs}s")
            } else {
                format!("{secs}s")
            }
        }
        Err(_) => "unknown".to_string(),
    }
}

fn print_recap<W: Write>(header: &str, session: &MeetingSession, output: &mut W) {
    let elapsed = format_elapsed(&session.started_at);
    writeln!(output, "── {header} ──").ok();
    writeln!(output, "Topic: {}", session.topic).ok();
    writeln!(output, "Duration: {elapsed}").ok();
    writeln!(output).ok();

    writeln!(output, "Decisions ({}):", session.decisions.len()).ok();
    if session.decisions.is_empty() {
        writeln!(output, "  (none)").ok();
    } else {
        for (i, d) in session.decisions.iter().enumerate() {
            writeln!(output, "  {}. {} — {}", i + 1, d.description, d.rationale).ok();
        }
    }
    writeln!(output).ok();

    writeln!(output, "Action Items ({}):", session.action_items.len()).ok();
    if session.action_items.is_empty() {
        writeln!(output, "  (none)").ok();
    } else {
        for (i, a) in session.action_items.iter().enumerate() {
            let due_suffix = a
                .due_description
                .as_deref()
                .map(|d| format!(" [due: {d}]"))
                .unwrap_or_default();
            writeln!(
                output,
                "  {}. [P{}] {} (owner: {}){due_suffix}",
                i + 1,
                a.priority,
                a.description,
                a.owner
            )
            .ok();
        }
    }
    writeln!(output).ok();

    writeln!(output, "Notes ({}):", session.notes.len()).ok();
    if session.notes.is_empty() {
        writeln!(output, "  (none)").ok();
    } else {
        for n in &session.notes {
            writeln!(output, "  - {n}").ok();
        }
    }
}

fn print_handoff_preview<W: Write>(session: &MeetingSession, output: &mut W) {
    let handoff = MeetingHandoff::from_session(session);
    let elapsed = format_elapsed(&session.started_at);

    writeln!(output, "── Handoff Preview ──").ok();
    writeln!(output, "Topic: {}", handoff.topic).ok();
    writeln!(output, "Duration: {elapsed}").ok();
    writeln!(
        output,
        "Participants: {}",
        if handoff.participants.is_empty() {
            "(none)".to_string()
        } else {
            handoff.participants.join(", ")
        }
    )
    .ok();
    writeln!(output).ok();

    writeln!(output, "Decisions ({}):", handoff.decisions.len()).ok();
    if handoff.decisions.is_empty() {
        writeln!(output, "  (none)").ok();
    } else {
        for (i, d) in handoff.decisions.iter().enumerate() {
            writeln!(output, "  {}. {} — {}", i + 1, d.description, d.rationale).ok();
        }
    }
    writeln!(output).ok();

    writeln!(output, "Action Items ({}):", handoff.action_items.len()).ok();
    if handoff.action_items.is_empty() {
        writeln!(output, "  (none)").ok();
    } else {
        for (i, a) in handoff.action_items.iter().enumerate() {
            let due_suffix = a
                .due_description
                .as_deref()
                .map(|d| format!(" [due: {d}]"))
                .unwrap_or_default();
            writeln!(
                output,
                "  {}. [P{}] {} (owner: {}){due_suffix}",
                i + 1,
                a.priority,
                a.description,
                a.owner
            )
            .ok();
        }
    }
    writeln!(output).ok();

    writeln!(output, "Open Questions ({}):", handoff.open_questions.len()).ok();
    if handoff.open_questions.is_empty() {
        writeln!(output, "  (none)").ok();
    } else {
        for (i, q) in handoff.open_questions.iter().enumerate() {
            let tag = if q.explicit { " [explicit]" } else { "" };
            writeln!(output, "  {}. {}{tag}", i + 1, q.text).ok();
        }
    }

    writeln!(output).ok();
    writeln!(
        output,
        "(This is a preview — the meeting is still open. Use /close to finalize.)"
    )
    .ok();
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
            due_description,
        } => {
            let item = ActionItem {
                description: description.clone(),
                owner: owner.clone(),
                priority,
                due_description: due_description.clone(),
            };
            match record_action_item(session, item) {
                Ok(()) => {
                    let due_suffix = due_description
                        .as_deref()
                        .map(|d| format!(", due: {d}"))
                        .unwrap_or_default();
                    writeln!(
                        output,
                        "Recorded action: {description} (owner={owner}{due_suffix})"
                    )
                    .ok()
                }
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
        MeetingCommand::Question(text) => match add_question(session, &text) {
            Ok(()) => {
                writeln!(output, "Question added: {text}").ok();
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
        MeetingCommand::Preview => {
            print_handoff_preview(session, output);
        }
        MeetingCommand::Help => {
            write!(output, "{}", help_text()).ok();
        }
        MeetingCommand::List => {
            let has_items = !session.decisions.is_empty()
                || !session.action_items.is_empty()
                || !session.notes.is_empty()
                || !session.explicit_questions.is_empty();
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
                        let due_suffix = a
                            .due_description
                            .as_deref()
                            .map(|d| format!(" [due: {d}]"))
                            .unwrap_or_default();
                        writeln!(
                            output,
                            "  {}. {} (owner={}){due_suffix}",
                            i + 1,
                            a.description,
                            a.owner
                        )
                        .ok();
                    }
                }
                if !session.notes.is_empty() {
                    writeln!(output, "Notes:").ok();
                    for (i, n) in session.notes.iter().enumerate() {
                        writeln!(output, "  {}. {}", i + 1, n).ok();
                    }
                }
                if !session.explicit_questions.is_empty() {
                    writeln!(output, "Questions:").ok();
                    for (i, q) in session.explicit_questions.iter().enumerate() {
                        writeln!(output, "  {}. {}", i + 1, q).ok();
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
            let elapsed = format_elapsed(&session.started_at);
            writeln!(output, "Meeting: {}", topic).ok();
            writeln!(output, "  Elapsed:     {elapsed}").ok();
            writeln!(output, "  Decisions:   {}", session.decisions.len()).ok();
            writeln!(output, "  Actions:     {}", session.action_items.len()).ok();
            writeln!(output, "  Notes:       {}", session.notes.len()).ok();
            writeln!(
                output,
                "  Questions:   {}",
                session.explicit_questions.len()
            )
            .ok();
            writeln!(output, "  Participants: {}", session.participants.len()).ok();
        }
        MeetingCommand::Recap => {
            print_recap("Meeting Recap", session, output);
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
            writeln!(output, "Unknown command: {input}").ok();
            writeln!(
                output,
                "Available commands: /decision, /action, /note, /question, /close, /done, /help"
            )
            .ok();
            writeln!(
                output,
                "Type /help for full syntax, or just type naturally to talk with Simard."
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
        assert!(output_str.contains("/question"));
    }

    #[test]
    fn repl_question_command_adds_explicit_question() {
        let bridge = mock_bridge();
        let input = b"/question What is the release timeline?\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Q test", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Question added"));
        assert_eq!(session.explicit_questions.len(), 1);
        assert_eq!(
            session.explicit_questions[0],
            "What is the release timeline?"
        );
    }

    #[test]
    fn repl_question_appears_in_list() {
        let bridge = mock_bridge();
        let input = b"/question Who owns rollback?\n/list\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Q list", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Questions:"));
        assert!(output_str.contains("1. Who owns rollback?"));
    }

    #[test]
    fn repl_preview_shows_handoff_without_closing() {
        let bridge = mock_bridge();
        let input = b"/decision Ship it | Ready\n/action Write tests | bob | 2\n/question ETA?\n/preview\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Preview", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        // Preview header and content
        assert!(
            output_str.contains("Handoff Preview"),
            "should show preview header: {output_str}"
        );
        assert!(output_str.contains("Decisions (1):"));
        assert!(output_str.contains("Action Items (1):"));
        assert!(output_str.contains("Open Questions"));
        assert!(output_str.contains("still open"));
        // Session should still close normally after /preview
        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.decisions.len(), 1);
    }

    #[test]
    fn repl_preview_empty_session() {
        let bridge = mock_bridge();
        let input = b"/preview\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Empty preview", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Handoff Preview"));
        assert!(output_str.contains("Decisions (0):"));
        assert!(output_str.contains("(none)"));
    }

    #[test]
    fn repl_action_with_due_date() {
        let bridge = mock_bridge();
        let input = b"/action Write tests | bob | 2 due:2026-04-15\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Due date", &bridge, None, "", &mut reader, &mut output).unwrap();

        assert_eq!(session.action_items.len(), 1);
        assert_eq!(
            session.action_items[0].due_description,
            Some("2026-04-15".to_string())
        );
        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("due: 2026-04-15"),
            "confirmation should show due date: {output_str}"
        );
    }

    #[test]
    fn repl_action_due_date_shows_in_list() {
        let bridge = mock_bridge();
        let input = b"/action Write tests | bob due:next Friday\n/list\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Due list", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("[due: next Friday]"),
            "list should show due date: {output_str}"
        );
    }

    #[test]
    fn repl_action_due_date_shows_in_preview() {
        let bridge = mock_bridge();
        let input = b"/action Deploy | alice | 1 due:2026-05-01\n/preview\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Due preview", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("[due: 2026-05-01]"),
            "preview should show due date: {output_str}"
        );
    }

    // -----------------------------------------------------------------------
    // WIP persistence integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn repl_saves_wip_on_command_and_cleans_up_on_close() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-only, run with --test-threads=1 to avoid races.
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

        let bridge = mock_bridge();
        let input = b"/decision Ship it | Ready\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("WIP test", &bridge, None, "", &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.decisions.len(), 1);

        // WIP file should be cleaned up after /close.
        let wip_path = dir
            .path()
            .join(crate::meeting_facilitator::MEETING_SESSION_WIP_FILENAME);
        assert!(
            !wip_path.exists(),
            "WIP file should be removed after /close"
        );

        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    }

    #[test]
    fn repl_resumes_from_wip_on_y() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-only, run with --test-threads=1 to avoid races.
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

        // Pre-populate a WIP session.
        use crate::meeting_facilitator::{
            MeetingSession as MS, MeetingSessionStatus as MSS, save_session_wip,
        };
        let wip = MS {
            topic: "Resumed topic".to_string(),
            decisions: vec![MeetingDecision {
                description: "Old decision".to_string(),
                rationale: "From before crash".to_string(),
                participants: vec![],
            }],
            action_items: vec![],
            notes: vec!["old note".to_string()],
            status: MSS::Open,
            started_at: chrono::Utc::now().to_rfc3339(),
            participants: vec!["alice".to_string()],
            explicit_questions: vec![],
        };
        save_session_wip(dir.path(), &wip).unwrap();

        let bridge = mock_bridge();
        // Answer "y" to resume, then close immediately.
        let input = b"y\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Ignored topic", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Resume previous session?"),
            "should prompt for resume: {output_str}"
        );
        assert!(
            output_str.contains("Resuming session"),
            "should confirm resume: {output_str}"
        );
        // Session should carry over the old decision and topic.
        assert_eq!(session.topic, "Resumed topic");
        assert_eq!(session.decisions.len(), 1);
        assert_eq!(session.decisions[0].description, "Old decision");
        assert!(session.notes.contains(&"old note".to_string()));
        assert!(session.participants.contains(&"alice".to_string()));

        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    }

    #[test]
    fn repl_declines_resume_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-only, run with --test-threads=1 to avoid races.
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

        // Pre-populate a stale WIP session.
        use crate::meeting_facilitator::{
            MeetingSession as MS, MeetingSessionStatus as MSS, save_session_wip,
        };
        let wip = MS {
            topic: "Stale topic".to_string(),
            decisions: vec![],
            action_items: vec![],
            notes: vec!["stale note".to_string()],
            status: MSS::Open,
            started_at: chrono::Utc::now().to_rfc3339(),
            participants: vec![],
            explicit_questions: vec![],
        };
        save_session_wip(dir.path(), &wip).unwrap();

        let bridge = mock_bridge();
        // Answer "n" to decline resume, then close.
        let input = b"n\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Fresh topic", &bridge, None, "", &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str.contains("Starting fresh session"),
            "should start fresh: {output_str}"
        );
        // Should use the new topic, not the stale one.
        assert_eq!(session.topic, "Fresh topic");
        assert!(session.notes.is_empty());

        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
    }
}
