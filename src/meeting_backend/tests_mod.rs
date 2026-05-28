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
fn send_message_returns_err_on_empty_adapter_response() {
    // Whitespace-only LLM output sanitises to empty; backend must
    // surface a structured error (handoff `failed` with reason
    // `empty_adapter_response`) rather than substituting a sentinel
    // string that downstream code treats as successful content. See #1671.
    let agent = MockAgent::new("   \n\n   ");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());
    let result = backend.send_message("ping");
    let err = result.expect_err("empty adapter response must produce Err");
    match err {
        crate::error::SimardError::ActionExecutionFailed { action, reason } => {
            assert_eq!(action, "send-message");
            assert!(
                reason.starts_with("empty_adapter_response"),
                "reason should be structured 'empty_adapter_response: ...', got: {reason}"
            );
        }
        other => panic!("expected ActionExecutionFailed, got: {other:?}"),
    }

    // Transcript history must NOT contain a sentinel-bearing assistant
    // message for the failed turn — only the user's prompt remains.
    let history = backend.history();
    assert_eq!(
        history.len(),
        1,
        "only the user message should be persisted on a failed turn, got {history:?}"
    );
    assert!(matches!(history[0].role, Role::User));
    let any_sentinel = history
        .iter()
        .any(|m| m.content.contains("[empty response]"));
    assert!(
        !any_sentinel,
        "no sentinel string should leak into the transcript"
    );
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
    let input = "Hello user.\n2026-04-18T18:23:41.151133Z INFO launching copilot binary=copilot\nACTION: search\nEXPLANATION: looking\nCONFIDENCE: 0.95\nChanges   +0 -0\nRequests  7.5 Premium (16s)\nTokens \u{2191} 64.7k \u{2193} 12k\nA newer version of amplihack is available.\nUpdate now? [y/N] (5s timeout):\n\u{2139} Loading...\n\u{2713} Done\nNODE_OPTIONS=--max-old-space\nGoodbye user.";
    let result = sanitize_agent_output(input);
    assert!(result.contains("Hello user."), "result: {result}");
    assert!(result.contains("Goodbye user."), "result: {result}");
    assert!(!result.contains("INFO"), "result: {result}");
    assert!(!result.contains("ACTION:"), "result: {result}");
    assert!(!result.contains("EXPLANATION:"), "result: {result}");
    assert!(!result.contains("CONFIDENCE:"), "result: {result}");
    assert!(!result.contains("Changes"), "result: {result}");
    assert!(!result.contains("Requests"), "result: {result}");
    assert!(!result.contains("Tokens"), "result: {result}");
    assert!(!result.contains("amplihack"), "result: {result}");
    assert!(!result.contains("Update now"), "result: {result}");
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

#[test]
fn apply_template_records_agenda() {
    let agent = MockAgent::new("noted");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());

    assert!(backend.applied_templates().is_empty());
    backend.apply_template("retro", "## Retro\n1. Wins\n2. Losses\n");

    let templates = backend.applied_templates();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].name, "retro");
    assert!(templates[0].agenda.contains("Wins"));
    assert!(
        !templates[0].applied_at.is_empty(),
        "applied_at should be set"
    );
}

#[test]
fn apply_template_dedupes_by_name_case_insensitive() {
    let agent = MockAgent::new("noted");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());

    backend.apply_template("standup", "## Standup A");
    backend.apply_template("STANDUP", "## Standup B");
    backend.apply_template("Standup", "## Standup C");

    let templates = backend.applied_templates();
    assert_eq!(templates.len(), 1, "duplicate template names should dedupe");
    assert_eq!(templates[0].name, "standup", "first applied wins");
    assert!(
        templates[0].agenda.contains("Standup A"),
        "first agenda preserved"
    );
}

#[test]
fn apply_template_records_distinct_templates() {
    let agent = MockAgent::new("noted");
    let mut backend = MeetingBackend::new_session("Test", Box::new(agent), None, String::new());

    backend.apply_template("standup", "agenda 1");
    backend.apply_template("retro", "agenda 2");

    let templates = backend.applied_templates();
    assert_eq!(templates.len(), 2);
    assert_eq!(templates[0].name, "standup");
    assert_eq!(templates[1].name, "retro");
}

#[test]
fn close_summary_includes_applied_templates() {
    let agent = MockAgent::new("Summary text.");
    let mut backend = MeetingBackend::new_session("Sprint", Box::new(agent), None, String::new());
    backend.apply_template(
        "standup",
        "## Standup\n1. Yesterday\n2. Today\n3. Blockers\n",
    );
    backend.send_message("hello").unwrap();

    let summary = backend.close().unwrap();
    assert_eq!(
        summary.applied_templates.len(),
        1,
        "summary should carry applied templates"
    );
    assert_eq!(summary.applied_templates[0].name, "standup");
    assert!(summary.applied_templates[0].agenda.contains("Blockers"));
}

// ── Inline /decision /action /question (issue #1730 seam (b)) ─────────

#[test]
fn push_explicit_decision_appends_and_dedupes() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("Planning", Box::new(agent), None, String::new());
    backend.push_explicit_decision("Adopt TDD", None);
    backend.push_explicit_decision("  Adopt TDD  ", None); // duplicate (case-insensitive after trim)
    backend.push_explicit_decision("ADOPT TDD", None); // case-insensitive duplicate
    backend.push_explicit_decision("Ship phase 8", None);
    backend.push_explicit_decision("   ", None); // empty -> ignored
    let decisions = backend.explicit_decisions();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].description, "Adopt TDD");
    assert_eq!(decisions[1].description, "Ship phase 8");
    // No rationale supplied → stored as empty string
    assert_eq!(decisions[0].rationale, "");
    assert_eq!(decisions[1].rationale, "");
}

#[test]
fn push_explicit_decision_with_rationale() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("Planning", Box::new(agent), None, String::new());
    backend.push_explicit_decision("Adopt TDD", Some("Memory safety"));
    backend.push_explicit_decision("Ship phase 8", None);
    let decisions = backend.explicit_decisions();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].description, "Adopt TDD");
    assert_eq!(decisions[0].rationale, "Memory safety");
    assert_eq!(decisions[1].description, "Ship phase 8");
    assert_eq!(decisions[1].rationale, "");
}

#[test]
fn push_explicit_question_appends_and_dedupes() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("Q", Box::new(agent), None, String::new());
    backend.push_explicit_question("Who owns rollout?");
    backend.push_explicit_question("Who owns rollout?"); // exact duplicate
    backend.push_explicit_question("WHO OWNS ROLLOUT?"); // case-insensitive duplicate
    backend.push_explicit_question("What is the SLO?");
    backend.push_explicit_question(""); // empty ignored
    assert_eq!(
        backend.explicit_questions(),
        &[
            "Who owns rollout?".to_string(),
            "What is the SLO?".to_string(),
        ]
    );
}

#[test]
fn push_explicit_action_item_extracts_assignee_and_deadline() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("A", Box::new(agent), None, String::new());
    backend.push_explicit_action_item("Bob will write tests by friday");
    backend.push_explicit_action_item("Update documentation"); // no assignee/deadline
    backend.push_explicit_action_item("   "); // empty ignored

    let items = backend.explicit_action_items();
    assert_eq!(items.len(), 2);
    // Explicit items get priority Some(1) so they sort ahead of heuristic ones.
    assert_eq!(items[0].priority, Some(1));
    // Assignee/deadline extraction reuses the heuristic helpers.
    assert_eq!(items[0].assignee.as_deref(), Some("Bob"));
    assert!(
        items[0]
            .deadline
            .as_ref()
            .is_some_and(|d| d.contains("friday")),
        "deadline should be parsed from inline text, got {:?}",
        items[0].deadline
    );
    // Plain item without signals: no assignee/deadline.
    assert_eq!(items[1].assignee, None);
    assert_eq!(items[1].deadline, None);
    assert_eq!(items[1].priority, Some(1));
}

#[test]
fn push_explicit_action_item_dedupes_by_description() {
    let agent = MockAgent::new("ok");
    let mut backend = MeetingBackend::new_session("A", Box::new(agent), None, String::new());
    backend.push_explicit_action_item("Update documentation");
    backend.push_explicit_action_item("update DOCUMENTATION"); // case-insensitive dup
    backend.push_explicit_action_item("Set up CI");
    let items = backend.explicit_action_items();
    assert_eq!(items.len(), 2, "duplicate descriptions must be dropped");
    assert_eq!(items[0].description, "Update documentation");
    assert_eq!(items[1].description, "Set up CI");
}

#[test]
fn close_summary_merges_explicit_decisions() {
    let agent = MockAgent::new("Summary text.");
    let mut backend =
        MeetingBackend::new_session("Decisions", Box::new(agent), None, String::new());
    backend.push_explicit_decision("Adopt TDD", Some("Better quality"));
    backend.push_explicit_decision("Use Rust for CLI", None);
    let summary = backend.close().unwrap();
    // Explicit decisions appear first, in registration order.
    assert!(summary.decisions.contains(&"Adopt TDD".to_string()));
    assert!(summary.decisions.contains(&"Use Rust for CLI".to_string()));
    assert_eq!(summary.decisions[0], "Adopt TDD");
}

#[test]
fn close_summary_merges_explicit_questions() {
    let agent = MockAgent::new("Summary text.");
    let mut backend =
        MeetingBackend::new_session("Questions", Box::new(agent), None, String::new());
    backend.push_explicit_question("What is our SLO target?");
    let summary = backend.close().unwrap();
    assert!(
        summary
            .open_questions
            .contains(&"What is our SLO target?".to_string()),
        "explicit question must appear in summary; got {:?}",
        summary.open_questions
    );
    // Explicit questions sort first.
    assert_eq!(summary.open_questions[0], "What is our SLO target?");
}

#[test]
fn close_summary_merges_explicit_action_items() {
    let agent = MockAgent::new("Summary text.");
    let mut backend = MeetingBackend::new_session("Actions", Box::new(agent), None, String::new());
    backend.push_explicit_action_item("Carol will set up CI by next sprint");
    let summary = backend.close().unwrap();
    assert!(
        !summary.action_items.is_empty(),
        "explicit action item must appear in summary"
    );
    let first = &summary.action_items[0];
    assert_eq!(first.priority, Some(1), "explicit items keep priority=1");
    assert_eq!(first.assignee.as_deref(), Some("Carol"));
}

// ── snapshot_session (issue #1984) ──

#[test]
fn snapshot_session_captures_topic_and_status() {
    let agent = MockAgent::new("ok");
    let backend =
        MeetingBackend::new_session("Snapshot Test", Box::new(agent), None, String::new());
    let snap = backend.snapshot_session();
    assert_eq!(snap.topic, "Snapshot Test");
    assert_eq!(
        snap.status,
        crate::meeting_facilitator::MeetingSessionStatus::Open
    );
    assert!(!snap.started_at.is_empty());
}

#[test]
fn snapshot_session_captures_decisions_actions_questions() {
    let agent = MockAgent::new("ok");
    let mut backend =
        MeetingBackend::new_session("Snap Items", Box::new(agent), None, String::new());
    backend.push_explicit_decision("Use Rust", None);
    backend.push_explicit_action_item("Alice will deploy by Friday");
    backend.push_explicit_question("What about caching?");
    backend.push_theme("performance".to_string());
    backend.set_goal("Ship v2");

    let snap = backend.snapshot_session();
    assert_eq!(snap.decisions.len(), 1);
    assert_eq!(snap.decisions[0].description, "Use Rust");
    assert_eq!(snap.action_items.len(), 1);
    assert_eq!(
        snap.action_items[0].description,
        "Alice will deploy by Friday"
    );
    assert_eq!(snap.explicit_questions, vec!["What about caching?"]);
    assert_eq!(snap.themes, vec!["performance"]);
    assert_eq!(snap.goal.as_deref(), Some("Ship v2"));
}

#[test]
fn snapshot_session_round_trips_through_wip_persistence() {
    let agent = MockAgent::new("ok");
    let mut backend =
        MeetingBackend::new_session("Round Trip", Box::new(agent), None, String::new());
    backend.push_explicit_decision("Decided X", None);
    backend.push_explicit_question("Open Q?");

    let snap = backend.snapshot_session();

    let dir = tempfile::tempdir().expect("temp dir");
    crate::meeting_facilitator::save_session_wip(dir.path(), &snap).expect("save WIP");

    let loaded = crate::meeting_facilitator::load_session_wip(dir.path())
        .expect("load WIP")
        .expect("WIP file should exist");

    assert_eq!(loaded.topic, "Round Trip");
    assert_eq!(loaded.decisions.len(), 1);
    assert_eq!(loaded.explicit_questions, vec!["Open Q?"]);

    crate::meeting_facilitator::remove_session_wip(dir.path()).expect("remove WIP");
    let gone = crate::meeting_facilitator::load_session_wip(dir.path()).expect("load after remove");
    assert!(gone.is_none(), "WIP file should be gone after remove");
}
