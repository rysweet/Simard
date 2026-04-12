//! Simple command parsing for the unified meeting backend.
//!
//! Only slash commands are special — everything else is natural conversation.

/// Parsed command from user input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeetingCommand {
    Help,
    Close,
    Status,
    /// Natural language — forwarded to the LLM.
    Conversation(String),
}

/// Parse a single line of input into a `MeetingCommand`.
///
/// Only `/help`, `/close` (and `/done`), and `/status` are recognised as
/// commands. Everything else — including lines that happen to start with `/`
/// but aren't one of the above — is treated as conversation.
pub fn parse_command(input: &str) -> MeetingCommand {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return MeetingCommand::Conversation(String::new());
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "/help" => MeetingCommand::Help,
        "/close" | "/done" => MeetingCommand::Close,
        "/status" => MeetingCommand::Status,
        _ => MeetingCommand::Conversation(trimmed.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_help() {
        assert_eq!(parse_command("/help"), MeetingCommand::Help);
        assert_eq!(parse_command("  /HELP  "), MeetingCommand::Help);
    }

    #[test]
    fn parse_close_variants() {
        assert_eq!(parse_command("/close"), MeetingCommand::Close);
        assert_eq!(parse_command("/done"), MeetingCommand::Close);
        assert_eq!(parse_command("  /Close "), MeetingCommand::Close);
    }

    #[test]
    fn parse_status() {
        assert_eq!(parse_command("/status"), MeetingCommand::Status);
    }

    #[test]
    fn parse_conversation_plain_text() {
        assert_eq!(
            parse_command("Let's discuss the roadmap"),
            MeetingCommand::Conversation("Let's discuss the roadmap".to_string()),
        );
    }

    #[test]
    fn parse_conversation_unknown_slash() {
        assert_eq!(
            parse_command("/decision Ship it | ready"),
            MeetingCommand::Conversation("/decision Ship it | ready".to_string()),
        );
    }

    #[test]
    fn parse_empty_input() {
        assert_eq!(
            parse_command(""),
            MeetingCommand::Conversation(String::new()),
        );
        assert_eq!(
            parse_command("   "),
            MeetingCommand::Conversation(String::new()),
        );
    }
}
