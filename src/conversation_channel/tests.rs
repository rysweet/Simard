//! TDD tests for the conversation abstraction (issue #2527).
//!
//! These are written **first**: [`apply_record`] and [`run_conversation`] are
//! `todo!()` stubs at this point, so the tests here are the "red" phase — they
//! pin the exact behavior the build step must deliver. `MockConversationChannel`
//! is already implemented, so the mock self-tests pass immediately and validate
//! the test double itself.
//!
//! The acknowledgement strings asserted below are the **actual** strings the CLI
//! REPL (`meeting_repl/repl.rs`) and dashboard chat (`operator_commands_dashboard/
//! chat.rs`) already emit — the whole point of the extraction is that they do not
//! change. If a future edit changes any of these strings, that is a behavior
//! regression, not a refactor.

use serial_test::serial;

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use crate::error::SimardResult;
use crate::meeting_backend::MeetingBackend;
use crate::meeting_backend::command::MeetingCommand;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

use super::dispatch::apply_record;
use super::driver::run_conversation;
use super::{ConversationChannel, MockConversationChannel, OutKind};

// ── Test fixtures ───────────────────────────────────────────────────────────

/// Minimal mock agent returning a canned response for every turn. Local to this
/// module so the tests are self-contained (mirrors `meeting_repl::test_support`).
struct AlwaysOkAgent {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    canned_response: String,
}

impl AlwaysOkAgent {
    fn new(response: &str) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("mock-conversation-agent"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock-agent",
                    "test:mock-conversation-agent",
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

impl BaseTypeSession for AlwaysOkAgent {
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
            execution_summary: self.canned_response.clone(),
            evidence: Vec::new(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

fn test_backend(topic: &str) -> MeetingBackend {
    MeetingBackend::new_session(
        topic,
        Box::new(AlwaysOkAgent::new("ok")),
        None,
        "test-system-prompt".to_string(),
    )
}

/// Run an async body on a fresh current-thread runtime (so `spawn_blocking` in
/// the eventual driver still works) under a hermetic handoff dir.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

// ── apply_record: pure record commands (mutation + canonical text) ───────────

#[test]
fn apply_record_theme_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Theme("performance".into()))
        .expect("theme is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Theme recorded: performance");
    assert_eq!(b.explicit_themes(), &["performance".to_string()]);
}

#[test]
fn apply_record_decision_without_rationale() {
    let mut b = test_backend("t");
    let rec = apply_record(
        &mut b,
        &MeetingCommand::Decision {
            text: "Adopt TDD".into(),
            rationale: None,
        },
    )
    .expect("decision is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Decision recorded: Adopt TDD");
    assert_eq!(b.explicit_decisions().len(), 1);
}

#[test]
fn apply_record_decision_with_rationale() {
    let mut b = test_backend("t");
    let rec = apply_record(
        &mut b,
        &MeetingCommand::Decision {
            text: "Use signal-cli".into(),
            rationale: Some("no embedded protocol".into()),
        },
    )
    .expect("decision is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(
        rec.text,
        "Decision recorded: Use signal-cli (rationale: no embedded protocol)"
    );
    assert_eq!(b.explicit_decisions().len(), 1);
}

#[test]
fn apply_record_action_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Action("write the tests".into()))
        .expect("action is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Action recorded: write the tests");
    assert_eq!(b.explicit_action_items().len(), 1);
}

#[test]
fn apply_record_question_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Question("what is our SLO?".into()))
        .expect("question is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Question recorded: what is our SLO?");
    assert_eq!(b.explicit_questions().len(), 1);
}

#[test]
fn apply_record_owner_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Owner("engineer".into()))
        .expect("owner is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Next owner recorded: engineer");
    assert_eq!(b.explicit_next_owner(), Some("engineer"));
}

#[test]
fn apply_record_goal_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(
        &mut b,
        &MeetingCommand::Goal("ship the signal channel".into()),
    )
    .expect("goal is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Goal recorded: ship the signal channel");
    assert_eq!(b.explicit_goal(), Some("ship the signal channel"));
}

#[test]
fn apply_record_risk_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Risk("unstable API".into()))
        .expect("risk is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Risk recorded: unstable API");
    assert_eq!(b.explicit_risks().len(), 1);
}

#[test]
fn apply_record_disagreement_mutates_and_returns_canonical_text() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Disagree("prefer Python".into()))
        .expect("disagree is a record command");
    assert_eq!(rec.kind, OutKind::Recorded);
    assert_eq!(rec.text, "Disagreement recorded: prefer Python");
    assert_eq!(b.explicit_disagreements().len(), 1);
}

// ── apply_record: read-only status commands (no mutation, Status kind) ───────

#[test]
fn apply_record_status_is_status_kind_and_does_not_mutate() {
    let mut b = test_backend("status topic");
    let rec = apply_record(&mut b, &MeetingCommand::Status).expect("status returns a message");
    assert_eq!(rec.kind, OutKind::Status);
    assert!(!rec.text.is_empty());
    // Read-only: nothing was captured.
    assert_eq!(b.explicit_decisions().len(), 0);
    assert_eq!(b.explicit_action_items().len(), 0);
    assert!(b.explicit_goal().is_none());
}

#[test]
fn apply_record_help_is_status_kind() {
    let mut b = test_backend("t");
    let rec = apply_record(&mut b, &MeetingCommand::Help).expect("help returns a message");
    assert_eq!(rec.kind, OutKind::Status);
    assert!(!rec.text.is_empty());
}

#[test]
fn apply_record_unknown_is_status_kind() {
    let mut b = test_backend("t");
    let rec = apply_record(
        &mut b,
        &MeetingCommand::Unknown {
            input: "/colse".into(),
            suggestion: Some("/close".into()),
        },
    )
    .expect("unknown returns a did-you-mean message");
    assert_eq!(rec.kind, OutKind::Status);
    assert!(!rec.text.is_empty());
}

// ── apply_record: driver-handled commands return None ────────────────────────

#[test]
fn apply_record_conversation_is_driver_handled() {
    let mut b = test_backend("t");
    assert!(apply_record(&mut b, &MeetingCommand::Conversation("hi".into())).is_none());
}

#[test]
fn apply_record_close_is_driver_handled() {
    let mut b = test_backend("t");
    assert!(apply_record(&mut b, &MeetingCommand::Close).is_none());
}

#[test]
fn apply_record_export_is_driver_handled() {
    let mut b = test_backend("t");
    assert!(apply_record(&mut b, &MeetingCommand::Export).is_none());
}

#[test]
fn apply_record_template_is_driver_handled() {
    let mut b = test_backend("t");
    assert!(apply_record(&mut b, &MeetingCommand::Template("standup".into())).is_none());
}

// ── run_conversation over the mock channel ───────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn run_conversation_routes_records_and_fires_hook() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: serial_test serializes env access across the test binary.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", tmp.path()) };

    let mut ch = MockConversationChannel::with_script(vec![
        "/goal ship the signal channel",
        "/decision use signal-cli JSON-RPC --rationale no embedded protocol",
        "let's wrap up",
        "/close",
    ]);
    let mut backend = test_backend("signal channel");

    block_on(run_conversation(&mut ch, &mut backend)).unwrap();

    // The goal ack is delivered as a Recorded outbound with the canonical text.
    assert!(
        ch.sent()
            .iter()
            .any(|o| o.kind == OutKind::Recorded && o.text.contains("Goal recorded")),
        "expected a Recorded outbound acknowledging the goal; got {:?}",
        ch.sent()
    );
    // on_recorded fires once per record command: /goal + /decision = 2.
    assert_eq!(ch.recorded_hook_calls(), 2);
}

#[test]
#[serial(cognitive_memory)]
fn run_conversation_answers_a_conversation_turn() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: serial_test serializes env access across the test binary.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", tmp.path()) };

    let mut ch = MockConversationChannel::with_script(vec!["hello simard", "/close"]);
    let mut backend = test_backend("chat");

    block_on(run_conversation(&mut ch, &mut backend)).unwrap();

    // A plain conversation turn produces an Assistant outbound carrying the
    // agent's reply ("ok" from AlwaysOkAgent).
    assert!(
        ch.sent()
            .iter()
            .any(|o| o.kind == OutKind::Assistant && o.text.contains("ok")),
        "expected an Assistant reply outbound; got {:?}",
        ch.sent()
    );
    // A conversation turn is NOT a record command, so the hook never fires.
    assert_eq!(ch.recorded_hook_calls(), 0);
}

#[test]
#[serial(cognitive_memory)]
fn run_conversation_closes_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: serial_test serializes env access across the test binary.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", tmp.path()) };

    let mut ch = MockConversationChannel::with_script(vec!["/close"]);
    let mut backend = test_backend("close only");

    let result = block_on(run_conversation(&mut ch, &mut backend));
    assert!(
        result.is_ok(),
        "clean /close should return Ok, got {result:?}"
    );
}

// ── MockConversationChannel self-tests (mock is implemented → these pass) ─────

#[test]
fn mock_recv_replays_script_then_ends() {
    let mut ch = MockConversationChannel::with_script(vec!["one", "two"]);
    let first = block_on(ch.recv()).unwrap().unwrap();
    assert_eq!(first.text, "one");
    assert!(first.from.authorized);
    let second = block_on(ch.recv()).unwrap().unwrap();
    assert_eq!(second.text, "two");
    assert!(block_on(ch.recv()).unwrap().is_none());
}

#[test]
fn mock_send_captures_outbounds_in_order() {
    use super::Outbound;
    let mut ch = MockConversationChannel::with_script(vec![]);
    block_on(ch.send(Outbound {
        kind: OutKind::Status,
        text: "a".into(),
    }))
    .unwrap();
    block_on(ch.send(Outbound {
        kind: OutKind::Notice,
        text: "b".into(),
    }))
    .unwrap();
    assert_eq!(ch.sent().len(), 2);
    assert_eq!(ch.sent()[0].text, "a");
    assert_eq!(ch.sent()[1].kind, OutKind::Notice);
}
