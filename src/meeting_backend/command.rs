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
    /// Re-display the running list of decisions, open questions, and action items
    /// extracted from the live meeting transcript. Read-only — does not close.
    State,
    /// Operator marks a decision deterministically (e.g. `/decision Adopt TDD`).
    /// Bypasses post-hoc heuristic extraction so the item cannot be missed.
    Decision(String),
    /// Operator records an action item inline (e.g.
    /// `/action Bob will write tests by friday`). The text is parsed for
    /// assignee/deadline using the same extractors as the heuristic path.
    Action(String),
    /// Operator marks an open question deterministically (e.g.
    /// `/question What is our SLO target?`).
    Question(String),
    /// Operator names the agent / persona / human expected to action this
    /// handoff (e.g. `/owner engineer`, `/owner ooda-curate`, `/owner alice`).
    /// Empty payload (a bare `/owner`) falls through to conversation so
    /// the operator's intent isn't silently coerced. Added in issue #1954.
    Owner(String),
    /// Operator sets the meeting's overarching objective (e.g.
    /// `/goal Agree on the release plan for v2`). Empty payload falls
    /// through to conversation. Added in issue #1987.
    Goal(String),
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
        "/state" => MeetingCommand::State,
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
        _ if lower.starts_with("/decision ") => {
            let arg = trimmed["/decision ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Decision(arg)
            }
        }
        _ if lower.starts_with("/action ") => {
            let arg = trimmed["/action ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Action(arg)
            }
        }
        _ if lower.starts_with("/question ") => {
            let arg = trimmed["/question ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Question(arg)
            }
        }
        _ if lower.starts_with("/owner ") => {
            let arg = trimmed["/owner ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Owner(arg)
            }
        }
        _ if lower.starts_with("/goal ") => {
            let arg = trimmed["/goal ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Goal(arg)
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
        // Unrecognised slash commands fall through to Conversation so the
        // operator can still type things like file paths or markdown lists
        // that happen to start with `/`.
        assert_eq!(
            parse_command("/notarealcommand foo bar"),
            MeetingCommand::Conversation("/notarealcommand foo bar".to_string()),
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

    // ── /state command (issue #1646 — TDD red phase) ─────────────────

    #[test]
    fn parse_state_exact_token() {
        // "/state" with no surplus tokens parses to State variant.
        assert_eq!(parse_command("/state"), MeetingCommand::State);
    }

    #[test]
    fn parse_state_case_and_whitespace_insensitive() {
        // Mirrors /help, /close, /status conventions.
        assert_eq!(parse_command("  /STATE  "), MeetingCommand::State);
        assert_eq!(parse_command("/State"), MeetingCommand::State);
    }

    #[test]
    fn parse_state_with_surplus_tokens_is_conversation() {
        // Security M4 / S5: /state takes no arguments. Surplus tokens must
        // NOT be silently coerced into a State command — they fall through
        // to Conversation so the operator's intent isn't misread.
        assert_eq!(
            parse_command("/state foo"),
            MeetingCommand::Conversation("/state foo".to_string()),
        );
        assert_eq!(
            parse_command("/state extra args"),
            MeetingCommand::Conversation("/state extra args".to_string()),
        );
    }

    // ── Inline /decision /action /question (issue #1730 seam (b)) ─────

    #[test]
    fn parse_decision_with_arg() {
        assert_eq!(
            parse_command("/decision Adopt TDD for new modules"),
            MeetingCommand::Decision("Adopt TDD for new modules".to_string()),
        );
        assert_eq!(
            parse_command("  /Decision   Ship phase 8  "),
            MeetingCommand::Decision("Ship phase 8".to_string()),
        );
    }

    #[test]
    fn parse_decision_empty_arg_is_conversation() {
        // Mirrors the /theme empty-arg behaviour: a bare `/decision` (or
        // `/decision ` with only whitespace) is not a valid recording —
        // surface as conversation so the operator's intent isn't lost.
        assert_eq!(
            parse_command("/decision"),
            MeetingCommand::Conversation("/decision".to_string()),
        );
        assert_eq!(
            parse_command("/decision   "),
            MeetingCommand::Conversation("/decision".to_string()),
        );
    }

    #[test]
    fn parse_action_with_arg() {
        assert_eq!(
            parse_command("/action Bob will write tests by friday"),
            MeetingCommand::Action("Bob will write tests by friday".to_string()),
        );
        assert_eq!(
            parse_command("  /ACTION  Update docs  "),
            MeetingCommand::Action("Update docs".to_string()),
        );
    }

    #[test]
    fn parse_action_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/action"),
            MeetingCommand::Conversation("/action".to_string()),
        );
        assert_eq!(
            parse_command("/action    "),
            MeetingCommand::Conversation("/action".to_string()),
        );
    }

    #[test]
    fn parse_question_with_arg() {
        assert_eq!(
            parse_command("/question What is our SLO target?"),
            MeetingCommand::Question("What is our SLO target?".to_string()),
        );
        assert_eq!(
            parse_command("  /Question   Who owns rollout?  "),
            MeetingCommand::Question("Who owns rollout?".to_string()),
        );
    }

    #[test]
    fn parse_question_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/question"),
            MeetingCommand::Conversation("/question".to_string()),
        );
        assert_eq!(
            parse_command("/question  "),
            MeetingCommand::Conversation("/question".to_string()),
        );
    }

    // ── Inline /owner (issue #1954) ──────────────────────────────────

    #[test]
    fn parse_owner_with_arg() {
        assert_eq!(
            parse_command("/owner engineer"),
            MeetingCommand::Owner("engineer".to_string()),
        );
        assert_eq!(
            parse_command("  /Owner  alice  "),
            MeetingCommand::Owner("alice".to_string()),
        );
    }

    #[test]
    fn parse_owner_preserves_case() {
        // GitHub handles are case-sensitive; the parser must preserve
        // operator-typed case rather than lowercasing.
        assert_eq!(
            parse_command("/owner RyanSweet"),
            MeetingCommand::Owner("RyanSweet".to_string()),
        );
    }

    #[test]
    fn parse_owner_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/owner"),
            MeetingCommand::Conversation("/owner".to_string()),
        );
        assert_eq!(
            parse_command("/owner    "),
            MeetingCommand::Conversation("/owner".to_string()),
        );
    }

    // ── Inline /goal (issue #1987) ───────────────────────────────────

    #[test]
    fn parse_goal_with_arg() {
        assert_eq!(
            parse_command("/goal Agree on the release plan for v2"),
            MeetingCommand::Goal("Agree on the release plan for v2".to_string()),
        );
        assert_eq!(
            parse_command("  /Goal  Ship the feature  "),
            MeetingCommand::Goal("Ship the feature".to_string()),
        );
    }

    #[test]
    fn parse_goal_preserves_case() {
        assert_eq!(
            parse_command("/goal Finalize OAuth Flow"),
            MeetingCommand::Goal("Finalize OAuth Flow".to_string()),
        );
    }

    #[test]
    fn parse_goal_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/goal"),
            MeetingCommand::Conversation("/goal".to_string()),
        );
        assert_eq!(
            parse_command("/goal    "),
            MeetingCommand::Conversation("/goal".to_string()),
        );
    }
}
