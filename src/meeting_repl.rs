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

/// A structured item auto-detected from natural conversation.
#[derive(Clone, Debug)]
enum AutoCaptured {
    Decision(String),
    Action(String),
}

/// Scan user and agent text for implicit decisions and action items.
///
/// Returns a list of `AutoCaptured` items found via simple keyword heuristics.
/// This does NOT replace explicit `/decision` or `/action` commands — it
/// supplements them by catching things that happen in natural conversation.
fn auto_detect_structured_items(user_text: &str, agent_text: &str) -> Vec<AutoCaptured> {
    let mut items = Vec::new();

    // Decision patterns — things the agent completed or confirmed.
    let decision_indicators: &[&str] = &[
        "\u{2705}", // ✅
        "Closed", "closed", "Created", "created", "Merged", "merged", "Done", "Shipped", "shipped",
        "Approved", "approved", "Resolved", "resolved",
    ];

    // Action patterns — things the agent committed to doing.
    let action_indicators: &[&str] = &[
        "I'll ",
        "I will ",
        "Let me ",
        "I'll ", // curly apostrophe
        "Next step",
        "next step",
        "TODO:",
        "Will do",
        "will do",
    ];

    // Skip lines that are clearly table rows, headings, or formatting.
    let is_structural = |line: &str| -> bool {
        let t = line.trim();
        t.starts_with('|')
            || t.starts_with('#')
            || t.starts_with("---")
            || t.starts_with("===")
            || t.starts_with("```")
            || t.starts_with("**")
            || t.chars().filter(|&c| c == '|').count() >= 2
    };

    // Scan agent text for decisions — only prose lines, not tables/formatting.
    for line in agent_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 15 || is_structural(trimmed) {
            continue;
        }
        for indicator in decision_indicators {
            if trimmed.contains(indicator) {
                let desc = truncate_for_capture(trimmed, 120);
                items.push(AutoCaptured::Decision(desc));
                break;
            }
        }
    }

    // Scan agent text for action commitments — only prose lines.
    for line in agent_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.len() < 15 || is_structural(trimmed) {
            continue;
        }
        let dominated = decision_indicators.iter().any(|ind| trimmed.contains(ind));
        if dominated {
            continue;
        }
        for indicator in action_indicators {
            if trimmed.contains(indicator) {
                let desc = truncate_for_capture(trimmed, 120);
                items.push(AutoCaptured::Action(desc));
                break;
            }
        }
    }

    // Also scan user text for explicit priorities stated conversationally.
    // e.g. "Let's prioritize X" or "We decided to Y"
    let user_decision_phrases: &[&str] = &[
        "we decided",
        "let's go with",
        "decision:",
        "agreed:",
        "final answer:",
    ];
    for line in user_text.lines() {
        let lower = line.to_lowercase();
        let trimmed = line.trim();
        if trimmed.len() < 5 {
            continue;
        }
        for phrase in user_decision_phrases {
            if lower.contains(phrase) {
                let desc = truncate_for_capture(trimmed, 120);
                items.push(AutoCaptured::Decision(desc));
                break;
            }
        }
    }

    items
}

/// Record auto-captured items into the meeting session and print notifications.
fn auto_capture_structured_items<W: Write>(
    session: &mut MeetingSession,
    user_text: &str,
    agent_text: &str,
    output: &mut W,
) {
    let items = auto_detect_structured_items(user_text, agent_text);
    for item in items {
        match item {
            AutoCaptured::Decision(ref desc) => {
                let decision = MeetingDecision {
                    description: desc.clone(),
                    rationale: "auto-detected from conversation".to_string(),
                    participants: Vec::new(),
                };
                if record_decision(session, decision).is_ok() {
                    writeln!(
                        output,
                        "  [captured: decision \u{2014} {}]",
                        short_label(desc, 60)
                    )
                    .ok();
                }
            }
            AutoCaptured::Action(ref desc) => {
                let action = ActionItem {
                    description: desc.clone(),
                    owner: "simard".to_string(),
                    priority: 1,
                    due_description: None,
                };
                if record_action_item(session, action).is_ok() {
                    writeln!(
                        output,
                        "  [captured: action \u{2014} {}]",
                        short_label(desc, 60)
                    )
                    .ok();
                }
            }
        }
    }
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate_for_capture(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Short label for notification output.
fn short_label(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

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

fn write_meeting_handoff_artifact<W: Write>(closed: &MeetingSession, output: &mut W) {
    use crate::meeting_facilitator::{MeetingHandoff, default_handoff_dir, write_meeting_handoff};
    let handoff = MeetingHandoff::from_session(closed);
    let handoff_dir = default_handoff_dir();
    if let Err(e) = write_meeting_handoff(&handoff_dir, &handoff) {
        writeln!(output, "[warn] Failed to write meeting handoff: {e}").ok();
    } else {
        writeln!(
            output,
            "Meeting handoff written ({} decisions, {} actions). Run `simard act-on-decisions` to create issues.",
            handoff.decisions.len(),
            handoff.action_items.len(),
        ).ok();
    }
}

fn persist_meeting_to_memory<W: Write>(
    closed: &MeetingSession,
    bridge: &CognitiveMemoryBridge,
    output: &mut W,
) {
    if !closed.notes.is_empty() {
        let transcript_text = closed.notes.join("\n");
        let episode_content = format!(
            "Meeting transcript — topic: {}\n\n{}",
            closed.topic, transcript_text
        );
        if let Err(e) = bridge.store_episode(
            &episode_content,
            "meeting-repl-transcript",
            Some(&serde_json::json!({
                "topic": closed.topic,
                "type": "transcript",
                "decisions": closed.decisions.len(),
                "action_items": closed.action_items.len(),
            })),
        ) {
            writeln!(output, "[warn] Failed to persist transcript: {e}").ok();
        }
    }

    for decision in &closed.decisions {
        let tags = vec![
            "meeting".to_string(),
            "decision".to_string(),
            closed.topic.clone(),
        ];
        if let Err(e) = bridge.store_fact(
            &format!("decision:{}", decision.description),
            &format!(
                "Decision: {} — Rationale: {}",
                decision.description, decision.rationale
            ),
            0.9,
            &tags,
            "meeting-repl",
        ) {
            writeln!(
                output,
                "[warn] Failed to persist decision '{}': {e}",
                decision.description
            )
            .ok();
        }
    }

    for item in &closed.action_items {
        if let Err(e) = bridge.store_prospective(
            &format!("Meeting action: {}", item.description),
            &format!("owner={} begins related work", item.owner),
            &format!(
                "Remind {} to complete: {} (priority {})",
                item.owner, item.description, item.priority
            ),
            i64::from(item.priority),
        ) {
            writeln!(
                output,
                "[warn] Failed to persist action '{}': {e}",
                item.description
            )
            .ok();
        }
    }
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
