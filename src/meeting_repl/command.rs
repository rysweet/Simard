//! REPL command parsing for meeting mode.

/// Parsed REPL command from a single input line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeetingCommand {
    /// `/decision <description> | <rationale>`
    Decision {
        description: String,
        rationale: String,
    },
    /// `/action <description> | <owner> [| <priority>] [due:<date-or-text>]`
    Action {
        description: String,
        owner: String,
        priority: u32,
        due_description: Option<String>,
    },
    /// `/note <text>` — explicit note (not sent to agent)
    Note(String),
    /// `/question <text>` — explicit open question
    Question(String),
    /// Natural language — sent to the agent for a conversational response
    Conversation(String),
    /// `/status` — show meeting status summary
    Status,
    /// `/recap` — show formatted summary of all captured items
    Recap,
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
    /// `/preview` — show handoff preview without closing
    Preview,
    /// `/close` — end the meeting
    Close,
    /// `/help` — show available commands
    Help,
    /// Empty line — skip
    Empty,
    /// Unrecognized slash-command
    Unknown(String),
}

/// Extract an optional `due:...` suffix from action-command text.
///
/// Returns `(cleaned_text, Option<due_description>)`.  The `due:` token is
/// recognised case-insensitively and may appear anywhere after the first `|`
/// separator (i.e. it won't accidentally match inside the description).
fn extract_due_suffix(raw: &str) -> (String, Option<String>) {
    // Search for the last occurrence of ` due:` (case-insensitive).
    let lower = raw.to_lowercase();
    if let Some(idx) = lower.rfind(" due:") {
        let due_text = raw[idx + 5..].trim().to_string();
        let cleaned = raw[..idx].to_string();
        if due_text.is_empty() {
            (cleaned, None)
        } else {
            (cleaned, Some(due_text))
        }
    } else {
        (raw.to_string(), None)
    }
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
        // Extract optional `due:...` suffix before pipe-splitting.
        let (rest_clean, due_description) = extract_due_suffix(rest);
        let parts: Vec<&str> = rest_clean.splitn(3, '|').collect();
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
                    due_description,
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

    if let Some(rest) = trimmed.strip_prefix("/question ") {
        let text = rest.trim().to_string();
        if !text.is_empty() {
            return MeetingCommand::Question(text);
        }
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    if trimmed == "/preview" {
        return MeetingCommand::Preview;
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

    if trimmed == "/recap" {
        return MeetingCommand::Recap;
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

    // Any line starting with `/` that didn't match above is an unknown command.
    if trimmed.starts_with('/') {
        return MeetingCommand::Unknown(trimmed.to_string());
    }

    // Non-command input is natural language — route to the agent.
    MeetingCommand::Conversation(trimmed.to_string())
}

/// Dynamically generated help text — stays in sync with the parser.
pub fn help_text() -> String {
    "\
Simard meeting — speak naturally and Simard will respond.

Commands (optional):
  /decision <description> | <rationale>                Record a formal decision
  /action <desc> | <owner> [| <priority>] [due:<date>] Record an action item
  /note <text>                                         Add an explicit note
  /question <text>                                     Add an explicit open question
  /list                                                Show numbered list of all items
  /edit <type> <number> <new text>                     Edit an item (type: decision, action, note)
  /delete <type> <number>                              Delete an item (type: decision, action, note)
  /status                                              Show meeting status summary
  /recap                                               Show formatted summary of all captured items
  /preview                                             Preview handoff artifact without closing
  /participants                                        List current participants
  /participants add <name>                             Add a participant
  /close or /done                                      Close the meeting and persist summary
  /help                                                Show this help

Anything else you type is a conversation with Simard.
"
    .to_string()
}

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
                due_description: None,
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
    fn parse_question_command() {
        assert_eq!(
            parse_meeting_command("/question What is the release timeline?"),
            MeetingCommand::Question("What is the release timeline?".to_string())
        );
    }

    #[test]
    fn parse_question_empty_is_unknown() {
        assert!(matches!(
            parse_meeting_command("/question "),
            MeetingCommand::Unknown(_)
        ));
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
    fn parse_recap_command() {
        assert_eq!(parse_meeting_command("/recap"), MeetingCommand::Recap);
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
    fn parse_unknown_slash_command() {
        assert_eq!(
            parse_meeting_command("/foobar"),
            MeetingCommand::Unknown("/foobar".to_string())
        );
        assert_eq!(
            parse_meeting_command("/status check"),
            MeetingCommand::Unknown("/status check".to_string())
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

    #[test]
    fn help_text_contains_all_commands() {
        let text = help_text();
        for cmd in &[
            "/decision",
            "/action",
            "/note",
            "/question",
            "/list",
            "/edit",
            "/delete",
            "/close",
            "/done",
            "/help",
            "/status",
            "/recap",
            "/participants",
            "/preview",
        ] {
            assert!(text.contains(cmd), "help_text() should mention {cmd}");
        }
    }

    #[test]
    fn parse_preview_command() {
        assert_eq!(parse_meeting_command("/preview"), MeetingCommand::Preview);
    }

    #[test]
    fn parse_action_with_date_due() {
        assert_eq!(
            parse_meeting_command("/action Write tests | bob | 2 due:2026-04-15"),
            MeetingCommand::Action {
                description: "Write tests".to_string(),
                owner: "bob".to_string(),
                priority: 2,
                due_description: Some("2026-04-15".to_string()),
            }
        );
    }

    #[test]
    fn parse_action_with_text_due() {
        assert_eq!(
            parse_meeting_command("/action Deploy staging | alice due:next sprint"),
            MeetingCommand::Action {
                description: "Deploy staging".to_string(),
                owner: "alice".to_string(),
                priority: 1,
                due_description: Some("next sprint".to_string()),
            }
        );
    }

    #[test]
    fn parse_action_without_due() {
        assert_eq!(
            parse_meeting_command("/action Fix bug | carol | 3"),
            MeetingCommand::Action {
                description: "Fix bug".to_string(),
                owner: "carol".to_string(),
                priority: 3,
                due_description: None,
            }
        );
    }

    #[test]
    fn extract_due_suffix_returns_none_when_absent() {
        let (cleaned, due) = extract_due_suffix("Write tests | bob | 2");
        assert_eq!(cleaned, "Write tests | bob | 2");
        assert_eq!(due, None);
    }

    #[test]
    fn extract_due_suffix_parses_date() {
        let (cleaned, due) = extract_due_suffix("Write tests | bob | 2 due:2026-04-15");
        assert_eq!(cleaned, "Write tests | bob | 2");
        assert_eq!(due, Some("2026-04-15".to_string()));
    }

    #[test]
    fn extract_due_suffix_parses_freeform_text() {
        let (cleaned, due) = extract_due_suffix("Deploy | alice due:end of week");
        assert_eq!(cleaned, "Deploy | alice");
        assert_eq!(due, Some("end of week".to_string()));
    }

    #[test]
    fn extract_due_suffix_empty_value_is_none() {
        let (cleaned, due) = extract_due_suffix("Deploy | alice due:");
        assert_eq!(cleaned, "Deploy | alice");
        assert_eq!(due, None);
    }
}
