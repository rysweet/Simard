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
    /// `/list` — show numbered list of all decisions, action items, and notes
    List,
    /// `/edit <type> <number> <new text>` — edit an existing item
    Edit {
        item_type: String,
        index: usize,
        new_text: String,
    },
    /// `/delete <type> <number>` — remove an item
    Delete { item_type: String, index: usize },
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

    if trimmed == "/list" {
        return MeetingCommand::List;
    }

    if let Some(rest) = trimmed.strip_prefix("/edit ") {
        let parts: Vec<&str> = rest.splitn(3, ' ').collect();
        if parts.len() == 3 {
            let item_type = parts[0].to_string();
            if let Ok(num) = parts[1].parse::<usize>()
                && num >= 1
            {
                let new_text = parts[2].trim().to_string();
                if !new_text.is_empty() {
                    return MeetingCommand::Edit {
                        item_type,
                        index: num - 1,
                        new_text,
                    };
                }
            }
        }
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("/delete ") {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if parts.len() == 2 {
            let item_type = parts[0].to_string();
            if let Ok(num) = parts[1].trim().parse::<usize>()
                && num >= 1
            {
                return MeetingCommand::Delete {
                    item_type,
                    index: num - 1,
                };
            }
        }
        return MeetingCommand::Unknown(trimmed.to_string());
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
  /list                                   Show numbered list of all items
  /edit <type> <number> <new text>        Edit an item (type: decision, action, note)
  /delete <type> <number>                 Delete an item (type: decision, action, note)
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

    #[test]
    fn parse_list_command() {
        assert_eq!(parse_meeting_command("/list"), MeetingCommand::List);
    }

    #[test]
    fn parse_edit_decision() {
        assert_eq!(
            parse_meeting_command("/edit decision 1 Updated wording"),
            MeetingCommand::Edit {
                item_type: "decision".to_string(),
                index: 0,
                new_text: "Updated wording".to_string(),
            }
        );
    }

    #[test]
    fn parse_edit_action() {
        assert_eq!(
            parse_meeting_command("/edit action 3 New description here"),
            MeetingCommand::Edit {
                item_type: "action".to_string(),
                index: 2,
                new_text: "New description here".to_string(),
            }
        );
    }

    #[test]
    fn parse_edit_note() {
        assert_eq!(
            parse_meeting_command("/edit note 2 Corrected note"),
            MeetingCommand::Edit {
                item_type: "note".to_string(),
                index: 1,
                new_text: "Corrected note".to_string(),
            }
        );
    }

    #[test]
    fn parse_edit_missing_text_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/edit decision 1"),
            MeetingCommand::Unknown(_)
        ));
    }

    #[test]
    fn parse_edit_zero_index_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/edit decision 0 text"),
            MeetingCommand::Unknown(_)
        ));
    }

    #[test]
    fn parse_edit_bad_number_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/edit decision abc text"),
            MeetingCommand::Unknown(_)
        ));
    }

    #[test]
    fn parse_delete_decision() {
        assert_eq!(
            parse_meeting_command("/delete decision 2"),
            MeetingCommand::Delete {
                item_type: "decision".to_string(),
                index: 1,
            }
        );
    }

    #[test]
    fn parse_delete_action() {
        assert_eq!(
            parse_meeting_command("/delete action 1"),
            MeetingCommand::Delete {
                item_type: "action".to_string(),
                index: 0,
            }
        );
    }

    #[test]
    fn parse_delete_missing_number_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/delete decision"),
            MeetingCommand::Unknown(_)
        ));
    }

    #[test]
    fn parse_delete_zero_index_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/delete action 0"),
            MeetingCommand::Unknown(_)
        ));
    }
}
