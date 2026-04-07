//! REPL command parsing for meeting mode.

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
    /// `/note <text>` — explicit note (not sent to agent)
    Note(String),
    /// Natural language — sent to the agent for a conversational response
    Conversation(String),
    /// `/status` — show meeting status summary
    Status,
    /// `/participants add <name>` — add a participant
    AddParticipant(String),
    /// `/participants` — list current participants
    ListParticipants,
    /// `/close` — end the meeting
    Close,
    /// `/help` — show available commands
    Help,
    /// Empty line — skip
    Empty,
    /// Unrecognized slash-command
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

    if trimmed == "/close" || trimmed == "/done" {
        return MeetingCommand::Close;
    }

    if trimmed == "/help" {
        return MeetingCommand::Help;
    }

    if trimmed == "/status" {
        return MeetingCommand::Status;
    }

    if trimmed == "/participants" {
        return MeetingCommand::ListParticipants;
    }

    if let Some(rest) = trimmed.strip_prefix("/participants ") {
        if let Some(name) = rest.strip_prefix("add ") {
            let name = name.trim().to_string();
            if !name.is_empty() {
                return MeetingCommand::AddParticipant(name);
            }
        }
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    // Any non-command input is natural language — route to the agent.
    MeetingCommand::Conversation(trimmed.to_string())
}

pub(super) const HELP_TEXT: &str = "\
Simard meeting — speak naturally and Simard will respond.

Commands (optional):
  /decision <description> | <rationale>   Record a formal decision
  /action <description> | <owner> [| <priority>]  Record an action item
  /note <text>                            Add an explicit note
  /status                                 Show meeting status summary
  /participants                           List current participants
  /participants add <name>                Add a participant
  /close or /done                         Close the meeting and persist summary
  /help                                   Show this help

Anything else you type is a conversation with Simard.
";

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_action_command() {
        assert_eq!(
            parse_meeting_command("/action Write integration tests | bob | 2"),
            MeetingCommand::Action {
                description: "Write integration tests".to_string(),
                owner: "bob".to_string(),
                priority: 2,
            }
        );
    }

    #[test]
    fn parse_note_command() {
        assert_eq!(
            parse_meeting_command("/note Check CI before merge"),
            MeetingCommand::Note("Check CI before merge".to_string())
        );
    }

    #[test]
    fn parse_close_command() {
        assert_eq!(parse_meeting_command("/close"), MeetingCommand::Close);
        assert_eq!(parse_meeting_command("/done"), MeetingCommand::Close);
    }

    #[test]
    fn parse_empty_line() {
        assert_eq!(parse_meeting_command(""), MeetingCommand::Empty);
        assert_eq!(parse_meeting_command("   "), MeetingCommand::Empty);
    }

    #[test]
    fn parse_natural_language_as_conversation() {
        assert_eq!(
            parse_meeting_command("hello world"),
            MeetingCommand::Conversation("hello world".to_string())
        );
    }

    #[test]
    fn parse_status_command() {
        assert_eq!(parse_meeting_command("/status"), MeetingCommand::Status);
    }

    #[test]
    fn parse_participants_list() {
        assert_eq!(
            parse_meeting_command("/participants"),
            MeetingCommand::ListParticipants
        );
    }

    #[test]
    fn parse_participants_add() {
        assert_eq!(
            parse_meeting_command("/participants add Alice"),
            MeetingCommand::AddParticipant("Alice".to_string())
        );
    }

    #[test]
    fn parse_participants_add_empty_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/participants add "),
            MeetingCommand::Unknown(_)
        ));
    }
}
