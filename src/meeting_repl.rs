//! Interactive meeting REPL — reads operator input from stdin and builds a
//! `MeetingSession` with decisions, action items, and notes.
//!
//! The REPL produces a durable `MeetingSession` (with `MeetingRecord` summary)
//! when the operator types `/close` or stdin reaches EOF.

use std::io::{BufRead, Write};

use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingSession, add_note, close_meeting, record_action_item,
    record_decision, start_meeting,
};
use crate::memory_bridge::CognitiveMemoryBridge;

const PROMPT: &str = "meeting> ";

/// Parsed REPL command from a single input line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeetingCommand {
    /// `/decision <description> | <rationale>`
    Decision {
        description: String,
        rationale: String,
    },
    /// `/action <description> | <owner> [| <priority>]`
    Action {
        description: String,
        owner: String,
        priority: u32,
    },
    /// `/note <text>`
    Note(String),
    /// `/close` — end the meeting
    Close,
    /// `/help` — show available commands
    Help,
    /// Empty line — skip
    Empty,
    /// Unrecognized input
    Unknown(String),
}

/// Parse a single line of REPL input into a `MeetingCommand`.
pub fn parse_meeting_command(line: &str) -> MeetingCommand {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return MeetingCommand::Empty;
    }

    if let Some(rest) = trimmed.strip_prefix("/decision ") {
        let parts: Vec<&str> = rest.splitn(2, '|').collect();
        if parts.len() == 2 {
            let description = parts[0].trim().to_string();
            let rationale = parts[1].trim().to_string();
            if !description.is_empty() && !rationale.is_empty() {
                return MeetingCommand::Decision {
                    description,
                    rationale,
                };
            }
        }
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("/action ") {
        let parts: Vec<&str> = rest.splitn(3, '|').collect();
        if parts.len() >= 2 {
            let description = parts[0].trim().to_string();
            let owner = parts[1].trim().to_string();
            let priority = if parts.len() == 3 {
                parts[2].trim().parse::<u32>().unwrap_or(1)
            } else {
                1
            };
            if !description.is_empty() && !owner.is_empty() && priority >= 1 {
                return MeetingCommand::Action {
                    description,
                    owner,
                    priority,
                };
            }
        }
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("/note ") {
        let text = rest.trim().to_string();
        if !text.is_empty() {
            return MeetingCommand::Note(text);
        }
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    if trimmed == "/close" {
        return MeetingCommand::Close;
    }

    if trimmed == "/help" {
        return MeetingCommand::Help;
    }

    MeetingCommand::Unknown(trimmed.to_string())
}

const HELP_TEXT: &str = "\
Commands:
  /decision <description> | <rationale>   Record a decision
  /action <description> | <owner> [| <priority>]  Record an action item
  /note <text>                            Add a free-form note
  /close                                  Close the meeting and persist summary
  /help                                   Show this help
";

/// Run the interactive meeting REPL, reading from `input` and writing to `output`.
///
/// Returns the closed `MeetingSession` with its durable summary, or an error
/// if the meeting could not be started or closed.
pub fn run_meeting_repl<R: BufRead, W: Write>(
    topic: &str,
    bridge: &CognitiveMemoryBridge,
    input: &mut R,
    output: &mut W,
) -> SimardResult<MeetingSession> {
    let mut session = start_meeting(topic, bridge)?;

    writeln!(output, "Meeting started: {topic}").ok();
    writeln!(output, "Type /help for available commands.").ok();

    let mut line = String::new();
    loop {
        write!(output, "{PROMPT}").ok();
        output.flush().ok();

        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => {
                // EOF — close the meeting
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

        match parse_meeting_command(&line) {
            MeetingCommand::Decision {
                description,
                rationale,
            } => {
                let decision = MeetingDecision {
                    description: description.clone(),
                    rationale,
                    participants: Vec::new(),
                };
                match record_decision(&mut session, decision) {
                    Ok(()) => {
                        writeln!(output, "Recorded decision: {description}").ok();
                    }
                    Err(e) => {
                        writeln!(output, "Error: {e}").ok();
                    }
                }
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
                match record_action_item(&mut session, item) {
                    Ok(()) => {
                        writeln!(output, "Recorded action: {description} (owner={owner})").ok();
                    }
                    Err(e) => {
                        writeln!(output, "Error: {e}").ok();
                    }
                }
            }
            MeetingCommand::Note(text) => match add_note(&mut session, &text) {
                Ok(()) => {
                    writeln!(output, "Note added.").ok();
                }
                Err(e) => {
                    writeln!(output, "Error: {e}").ok();
                }
            },
            MeetingCommand::Close => {
                writeln!(output, "Closing meeting.").ok();
                break;
            }
            MeetingCommand::Help => {
                write!(output, "{HELP_TEXT}").ok();
            }
            MeetingCommand::Empty => {}
            MeetingCommand::Unknown(input) => {
                writeln!(output, "Unknown command: {input}").ok();
                writeln!(output, "Type /help for available commands.").ok();
            }
        }
    }

    let closed = close_meeting(session, bridge)?;
    let summary = closed.durable_summary();
    writeln!(output, "Meeting record: {summary}").ok();
    Ok(closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::meeting_facilitator::MeetingSessionStatus;
    use serde_json::json;

    fn mock_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-meeting-repl", |method, _params| match method {
                "memory.record_sensory" => Ok(json!({"id": "sen_r1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_r1"})),
                "memory.store_fact" => Ok(json!({"id": "sem_r1"})),
                "memory.store_prospective" => Ok(json!({"id": "pro_r1"})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    #[test]
    fn parse_decision_command() {
        assert_eq!(
            parse_meeting_command("/decision Ship phase 8 | Unblocks goal curation"),
            MeetingCommand::Decision {
                description: "Ship phase 8".to_string(),
                rationale: "Unblocks goal curation".to_string(),
            }
        );
    }

    #[test]
    fn parse_action_command_with_priority() {
        assert_eq!(
            parse_meeting_command("/action Write tests | bob | 2"),
            MeetingCommand::Action {
                description: "Write tests".to_string(),
                owner: "bob".to_string(),
                priority: 2,
            }
        );
    }

    #[test]
    fn parse_action_command_default_priority() {
        assert_eq!(
            parse_meeting_command("/action Fix bug | alice"),
            MeetingCommand::Action {
                description: "Fix bug".to_string(),
                owner: "alice".to_string(),
                priority: 1,
            }
        );
    }

    #[test]
    fn parse_note_command() {
        assert_eq!(
            parse_meeting_command("/note Remember to check CI"),
            MeetingCommand::Note("Remember to check CI".to_string())
        );
    }

    #[test]
    fn parse_close_command() {
        assert_eq!(parse_meeting_command("/close"), MeetingCommand::Close);
    }

    #[test]
    fn parse_help_command() {
        assert_eq!(parse_meeting_command("/help"), MeetingCommand::Help);
    }

    #[test]
    fn parse_empty_line() {
        assert_eq!(parse_meeting_command(""), MeetingCommand::Empty);
        assert_eq!(parse_meeting_command("   "), MeetingCommand::Empty);
    }

    #[test]
    fn parse_unknown_command() {
        assert_eq!(
            parse_meeting_command("hello world"),
            MeetingCommand::Unknown("hello world".to_string())
        );
    }

    #[test]
    fn repl_records_decision_and_closes() {
        let bridge = mock_bridge();
        let input = b"/decision Ship it | Ready for production\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session =
            run_meeting_repl("Sprint planning", &bridge, &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.decisions.len(), 1);
        assert_eq!(session.decisions[0].description, "Ship it");
        assert_eq!(session.decisions[0].rationale, "Ready for production");
    }

    #[test]
    fn repl_records_action_item_and_closes() {
        let bridge = mock_bridge();
        let input = b"/action Write tests | bob | 2\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl("Retro", &bridge, &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.action_items.len(), 1);
        assert_eq!(session.action_items[0].description, "Write tests");
        assert_eq!(session.action_items[0].owner, "bob");
        assert_eq!(session.action_items[0].priority, 2);
    }

    #[test]
    fn repl_records_note_and_closes_on_eof() {
        let bridge = mock_bridge();
        let input = b"/note Check CI before merge\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl("Standup", &bridge, &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.notes, vec!["Check CI before merge"]);
    }

    #[test]
    fn repl_produces_durable_summary_in_output() {
        let bridge = mock_bridge();
        let input = b"/decision Use Rust | Performance\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Architecture", &bridge, &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Meeting record:"));
        assert!(output_str.contains("Use Rust"));
    }

    #[test]
    fn repl_full_session_with_multiple_items() {
        let bridge = mock_bridge();
        let input = b"/decision Adopt TDD | Reduces regressions\n\
                      /action Migrate tests | alice | 1\n\
                      /note Sprint velocity improving\n\
                      /close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl("Sprint review", &bridge, &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        assert_eq!(session.decisions.len(), 1);
        assert_eq!(session.action_items.len(), 1);
        assert_eq!(session.notes.len(), 1);
    }

    #[test]
    fn repl_shows_help() {
        let bridge = mock_bridge();
        let input = b"/help\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        run_meeting_repl("Help test", &bridge, &mut reader, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("/decision"));
        assert!(output_str.contains("/action"));
        assert!(output_str.contains("/note"));
        assert!(output_str.contains("/close"));
    }

    #[test]
    fn repl_handles_unknown_command_gracefully() {
        let bridge = mock_bridge();
        let input = b"gibberish\n/close\n";
        let mut reader = &input[..];
        let mut output = Vec::new();

        let session = run_meeting_repl("Test", &bridge, &mut reader, &mut output).unwrap();

        assert_eq!(session.status, MeetingSessionStatus::Closed);
        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Unknown command"));
    }
}
