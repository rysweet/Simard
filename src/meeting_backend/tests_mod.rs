use super::*;
use crate::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

/// Mock agent that returns a canned response.
struct MockAgent {
    descriptor: BaseTypeDescriptor,
    response: String,
    is_open: bool,
    is_closed: bool,
}

impl MockAgent {
    fn new(response: &str) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("mock-meeting-backend"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock",
                    "test:mock-meeting-backend",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            response: response.to_string(),
            is_open: true,
            is_closed: false,
        }
    }
}

impl BaseTypeSession for MockAgent {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }
    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }
    fn run_turn(&mut self, _input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: self.response.clone(),
            evidence: Vec::new(),
        })
    }
    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

#[test]
fn new_session_creates_open_session() {
    let agent = MockAgent::new("hello");
    let backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
    assert!(backend.is_open());
    assert_eq!(backend.topic(), "Test");
    let status = backend.status();
    assert_eq!(status.message_count, 0);
    assert!(status.is_open);
}

#[test]
fn send_message_accumulates_history() {
    let agent = MockAgent::new("I understand");
    let mut backend = MeetingBackend::new_session("Sprint", Box::new(agent), None, String::new());

    let resp = backend.send_message("Let's discuss the roadmap").unwrap();
    assert_eq!(resp.content, "I understand");
    assert_eq!(resp.message_count, 2); // user + assistant

    let resp2 = backend.send_message("What about testing?").unwrap();
    assert_eq!(resp2.message_count, 4); // 2 more
}

#[test]
fn send_empty_message_returns_empty() {
    let agent = MockAgent::new("response");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
    let resp = backend.send_message("   ").unwrap();
    assert!(resp.content.is_empty());
    assert_eq!(resp.message_count, 0);
}

#[test]
fn close_produces_summary() {
    let agent = MockAgent::new("Here is the summary of our meeting.");
    let mut backend = MeetingBackend::new_session("Retro", Box::new(agent), None, String::new());

    backend.send_message("How did the sprint go?").unwrap();
    let summary = backend.close().unwrap();

    assert_eq!(summary.topic, "Retro");
    assert!(!summary.summary_text.is_empty());
    assert_eq!(summary.message_count, 2);
    assert!(!backend.is_open());
}

#[test]
fn send_message_after_close_fails() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
    backend.close().unwrap();

    let result = backend.send_message("hello");
    assert!(result.is_err());
}

#[test]
fn send_message_returns_sentinel_when_response_empty() {
    // Whitespace-only LLM output sanitises to empty; backend must
    // surface the explicit sentinel rather than an empty bubble.
    let agent = MockAgent::new("   \n\n   ");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
    let resp = backend.send_message("ping").unwrap();
    assert_eq!(resp.content, EMPTY_RESPONSE_SENTINEL);
    assert_eq!(resp.message_count, 2);
}

#[test]
fn double_close_fails() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
    backend.close().unwrap();
    let result = backend.close();
    assert!(result.is_err());
}

#[test]
fn status_reflects_message_count() {
    let agent = MockAgent::new("noted");
    let mut backend = MeetingBackend::new_session("Planning", Box::new(agent), None, String::new());

    assert_eq!(backend.status().message_count, 0);
    backend.send_message("Item 1").unwrap();
    assert_eq!(backend.status().message_count, 2);
    backend.send_message("Item 2").unwrap();
    assert_eq!(backend.status().message_count, 4);
}

#[test]
fn conversation_preamble_includes_topic() {
    let agent = MockAgent::new("ok");
    let backend =
        MeetingBackend::new_session("Sprint Planning", Box::new(agent), None, String::new());
    let preamble = backend.build_conversation_preamble();
    assert!(preamble.contains("Sprint Planning"));
}

#[test]
fn extract_response_trims_whitespace() {
    let outcome = BaseTypeOutcome {
        plan: String::new(),
        execution_summary: "  hello world  ".to_string(),
        evidence: Vec::new(),
    };
    assert_eq!(extract_response(&outcome), "hello world");
}

#[test]
fn extract_response_unwraps_action_envelope_em_dash() {
    // Regression: dashboard chat T2+ replies arrived as ACTION/EXPLANATION/CONFIDENCE
    // protocol envelopes; the sanitizer stripped every line and returned empty.
    let outcome = BaseTypeOutcome {
        plan: String::new(),
        execution_summary:
            "ACTION: respond — 6.\nEXPLANATION: Direct arithmetic answer.\nCONFIDENCE: 1.0"
                .to_string(),
        evidence: Vec::new(),
    };
    assert_eq!(extract_response(&outcome), "6.");
}

#[test]
fn extract_response_unwraps_action_envelope_alt_verb() {
    let outcome = BaseTypeOutcome {
        plan: String::new(),
        execution_summary: "ACTION: reply — 100.\nEXPLANATION: Multiplication.\nCONFIDENCE: 0.99"
            .to_string(),
        evidence: Vec::new(),
    };
    assert_eq!(extract_response(&outcome), "100.");
}

#[test]
fn extract_response_unwraps_action_envelope_multiline_body() {
    let outcome = BaseTypeOutcome {
        plan: String::new(),
        execution_summary: "ACTION: respond — Line one of the reply.\nLine two of the reply.\nEXPLANATION: detail\nCONFIDENCE: 0.9".to_string(),
        evidence: Vec::new(),
    };
    assert_eq!(
        extract_response(&outcome),
        "Line one of the reply.\nLine two of the reply."
    );
}

#[test]
fn sanitize_strips_tool_call_lines() {
    let input = "Here is the answer.\n[tool_call: search_files]\n[tool_result: found 3 files]\nThe files are ready.";
    let result = sanitize_agent_output(input);
    assert!(result.contains("Here is the answer."), "result: {result}");
    assert!(result.contains("The files are ready."), "result: {result}");
    assert!(!result.contains("[tool_call:"), "result: {result}");
    assert!(!result.contains("[tool_result:"), "result: {result}");
}

#[test]
fn sanitize_strips_xml_tool_blocks() {
    let input = "Before block.\n<tool_call>\ninternal stuff\n</tool_call>\nAfter block.";
    let result = sanitize_agent_output(input);
    assert!(result.contains("Before block."), "result: {result}");
    assert!(result.contains("After block."), "result: {result}");
    assert!(!result.contains("internal stuff"), "result: {result}");
}

#[test]
fn sanitize_passes_clean_text() {
    let input = "Normal response.\nWith multiple lines.\n\nAnd paragraphs.";
    let result = sanitize_agent_output(input);
    assert!(result.contains("Normal response."), "result: {result}");
    assert!(result.contains("With multiple lines."), "result: {result}");
    assert!(result.contains("And paragraphs."), "result: {result}");
}

#[test]
fn sanitize_collapses_excessive_blanks() {
    let input = "Line 1\n\n\n\n\n\nLine 2";
    let result = sanitize_agent_output(input);
    assert!(!result.contains("\n\n\n\n"), "too many blanks: {result}");
    assert!(result.contains("Line 1"), "result: {result}");
    assert!(result.contains("Line 2"), "result: {result}");
}

#[test]
fn sanitize_handles_empty_input() {
    assert_eq!(sanitize_agent_output(""), "");
    assert_eq!(sanitize_agent_output("   "), "");
}

#[test]
fn sanitize_strips_ansi_escape_codes() {
    let input = "\x1b[33mWarning\x1b[0m: something \x1b[2mhappened\x1b[0m";
    let result = sanitize_agent_output(input);
    assert_eq!(result, "Warning: something happened");
}

#[test]
fn sanitize_strips_agent_noise_lines() {
    let input = "Hello user.\n2026-04-18T18:23:41.151133Z INFO launching copilot binary=copilot\nACTION: search\nEXPLANATION: looking\nCONFIDENCE: 0.95\nChanges +0 -0 Requests 7.5 Premium (16s)\nTokens \u{2191} 64.7k \u{2193} 12k\nA newer version of amplihack is available.\n\u{2139} Loading...\n\u{2713} Done\nNODE_OPTIONS=--max-old-space\nGoodbye user.";
    let result = sanitize_agent_output(input);
    assert!(result.contains("Hello user."), "result: {result}");
    assert!(result.contains("Goodbye user."), "result: {result}");
    assert!(!result.contains("INFO"), "result: {result}");
    assert!(!result.contains("ACTION:"), "result: {result}");
    assert!(!result.contains("EXPLANATION:"), "result: {result}");
    assert!(!result.contains("CONFIDENCE:"), "result: {result}");
    assert!(!result.contains("Changes +0"), "result: {result}");
    assert!(!result.contains("Tokens"), "result: {result}");
    assert!(!result.contains("amplihack"), "result: {result}");
    assert!(!result.contains("NODE_OPTIONS"), "result: {result}");
}

#[test]
fn auto_save_does_not_panic() {
    let agent = MockAgent::new("noted");
    let mut backend =
        MeetingBackend::new_session("AutoSave Test", Box::new(agent), None, String::new());
    backend.send_message("Test message").unwrap();
    assert_eq!(backend.status().message_count, 2);
}
