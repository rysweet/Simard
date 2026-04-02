//! Interactive meeting REPL — a **conversation** with Simard that also captures
//! decisions, action items, and notes.
//!
//! Natural-language lines are sent to the active base type agent (RustyClawd,
//! Copilot, Claude CLI, etc.) via `run_turn`. The agent's text response is
//! displayed and also recorded as a meeting note so the transcript is preserved.
//! Structured slash-commands (`/decision`, `/action`, `/note`, `/close`) bypass
//! the agent and record directly.
//!
//! The REPL produces a durable `MeetingSession` (with `MeetingRecord` summary)
//! when the operator types `/close` or stdin reaches EOF.

use std::io::{BufRead, Write};

use crate::base_types::{BaseTypeSession, BaseTypeTurnInput};
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingSession, add_note, close_meeting, record_action_item,
    record_decision, start_meeting,
};
use crate::memory_bridge::CognitiveMemoryBridge;

const PROMPT: &str = "simard:meeting> ";

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

    // Any non-command input is natural language — route to the agent.
    MeetingCommand::Conversation(trimmed.to_string())
}

const HELP_TEXT: &str = "\
Simard meeting — speak naturally and Simard will respond.

Commands (optional):
  /decision <description> | <rationale>   Record a formal decision
  /action <description> | <owner> [| <priority>]  Record an action item
  /note <text>                            Add an explicit note
  /close or /done                         Close the meeting and persist summary
  /help                                   Show this help

Anything else you type is a conversation with Simard.
";

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

    writeln!(
        output,
        "Simard v{} — meeting mode",
        env!("CARGO_PKG_VERSION")
    )
    .ok();
    writeln!(output, "Topic: {topic}").ok();
    if agent.is_some() {
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
                match record_action_item(&mut session, item) {
                    Ok(()) => {
                        writeln!(output, "Recorded action: {description} (owner={owner})").ok()
                    }
                    Err(e) => writeln!(output, "Error: {e}").ok(),
                };
            }
            MeetingCommand::Note(text) => match add_note(&mut session, &text) {
                Ok(()) => {
                    writeln!(output, "Note added.").ok();
                }
                Err(e) => {
                    writeln!(output, "Error: {e}").ok();
                }
            },
            MeetingCommand::Conversation(text) => {
                if let Some(ref mut agent_session) = agent {
                    let turn_input = BaseTypeTurnInput {
                        objective: text.clone(),
                        identity_context: meeting_system_prompt.to_string(),
                        prompt_preamble: format!("Meeting topic: {topic}"),
                    };
                    match agent_session.run_turn(turn_input) {
                        Ok(outcome) => {
                            let response = outcome.execution_summary.trim();
                            writeln!(output, "\n{response}\n").ok();
                            add_note(&mut session, &format!("operator: {text}")).ok();
                            add_note(&mut session, &format!("simard: {response}")).ok();
                        }
                        Err(e) => {
                            writeln!(output, "[agent error: {e}]").ok();
                            add_note(&mut session, &text).ok();
                        }
                    }
                } else {
                    // No agent — fall back to note-taking
                    add_note(&mut session, &text).ok();
                    writeln!(output, "Note added.").ok();
                }
            }
            MeetingCommand::Close => {
                writeln!(output, "Closing meeting.").ok();
                break;
            }
            MeetingCommand::Help => {
                write!(output, "{HELP_TEXT}").ok();
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

    let closed = close_meeting(session, bridge)?;
    let summary = closed.durable_summary();
    writeln!(output, "Meeting record: {summary}").ok();
    Ok(closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::{
        BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeTurnInput,
        ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
        standard_session_capabilities,
    };
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::meeting_facilitator::MeetingSessionStatus;
    use crate::metadata::{BackendDescriptor, Freshness};
    use crate::runtime::RuntimeTopology;
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

    /// Mock agent that returns a canned response for every turn.
    struct MockAgentSession {
        descriptor: BaseTypeDescriptor,
        is_open: bool,
        is_closed: bool,
        canned_response: String,
    }

    impl MockAgentSession {
        fn new(response: &str) -> Self {
            Self {
                descriptor: BaseTypeDescriptor {
                    id: BaseTypeId::new("mock-meeting-agent"),
                    backend: BackendDescriptor::for_runtime_type::<Self>(
                        "mock-agent",
                        "test:mock-meeting-agent",
                        Freshness::now().unwrap(),
                    ),
                    capabilities: standard_session_capabilities(),
                    supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
                },
                is_open: true,
                is_closed: false,
                canned_response: response.to_string(),
            }
        }
    }

    impl BaseTypeSession for MockAgentSession {
        fn descriptor(&self) -> &BaseTypeDescriptor {
            &self.descriptor
        }

        fn open(&mut self) -> crate::error::SimardResult<()> {
            ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
            ensure_session_not_already_open(&self.descriptor, self.is_open)?;
            self.is_open = true;
            Ok(())
        }

        fn run_turn(
            &mut self,
            _input: BaseTypeTurnInput,
        ) -> crate::error::SimardResult<BaseTypeOutcome> {
            ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
            ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
            Ok(BaseTypeOutcome {
                plan: String::new(),
                execution_summary: self.canned_response.clone(),
                evidence: Vec::new(),
            })
        }

        fn close(&mut self) -> crate::error::SimardResult<()> {
            ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
            self.is_closed = true;
            Ok(())
        }
    }

    // --- Parser tests ---

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

    // --- REPL tests without agent (note-taking fallback) ---

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

    // --- REPL tests WITH agent (conversational mode) ---

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
}
