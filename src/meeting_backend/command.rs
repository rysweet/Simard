//! Simple command parsing for the unified meeting backend.
//!
//! Only slash commands are special — everything else is natural conversation.

/// Parsed command from user input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeetingCommand {
    Help,
    Close,
    Status,
    /// Show or apply a meeting template (standup, 1on1, retro, planning).
    /// Empty string means "list available templates".
    Template(String),
    /// Export the current meeting as markdown to ~/.simard/meetings/.
    Export,
    /// Record an explicit theme for the meeting (e.g. `/theme performance`).
    Theme(String),
    /// Show a color-coded recap of the current session (decisions, actions, questions, themes).
    Recap,
    /// Preview what the handoff artifact will look like when the meeting closes.
    Preview,
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
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "/help" => MeetingCommand::Help,
        "/close" | "/done" => MeetingCommand::Close,
        "/status" => MeetingCommand::Status,
        "/export" => MeetingCommand::Export,
        "/recap" => MeetingCommand::Recap,
        "/preview" => MeetingCommand::Preview,
        "/template" => MeetingCommand::Template(String::new()),
        _ if lower.starts_with("/template ") => {
            let arg = trimmed["/template ".len()..].trim().to_string();
            MeetingCommand::Template(arg)
        }
        _ if lower.starts_with("/theme ") => {
            let arg = trimmed["/theme ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Theme(arg)
            }
        }
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
    fn parse_template_no_arg() {
        assert_eq!(
            parse_command("/template"),
            MeetingCommand::Template(String::new())
        );
        assert_eq!(
            parse_command("  /TEMPLATE  "),
            MeetingCommand::Template(String::new())
        );
    }

    #[test]
    fn parse_template_with_arg() {
        assert_eq!(
            parse_command("/template standup"),
            MeetingCommand::Template("standup".to_string()),
        );
        assert_eq!(
            parse_command("  /Template  1on1  "),
            MeetingCommand::Template("1on1".to_string()),
        );
    }

    #[test]
    fn parse_export() {
        assert_eq!(parse_command("/export"), MeetingCommand::Export);
        assert_eq!(parse_command("  /EXPORT  "), MeetingCommand::Export);
    }

    #[test]
    fn parse_theme_with_arg() {
        assert_eq!(
            parse_command("/theme performance"),
            MeetingCommand::Theme("performance".to_string()),
        );
        assert_eq!(
            parse_command("  /Theme  scalability  "),
            MeetingCommand::Theme("scalability".to_string()),
        );
    }

    #[test]
    fn parse_theme_empty_arg_is_conversation() {
        // "/theme" with only whitespace after — not a valid theme, treated as conversation
        // parse_command trims input, so "/theme   " becomes "/theme" in the Conversation payload
        assert_eq!(
            parse_command("/theme   "),
            MeetingCommand::Conversation("/theme".to_string()),
        );
    }

    #[test]
    fn parse_recap() {
        assert_eq!(parse_command("/recap"), MeetingCommand::Recap);
        assert_eq!(parse_command("  /RECAP  "), MeetingCommand::Recap);
    }

    #[test]
    fn parse_preview() {
        assert_eq!(parse_command("/preview"), MeetingCommand::Preview);
        assert_eq!(parse_command("  /PREVIEW  "), MeetingCommand::Preview);
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
