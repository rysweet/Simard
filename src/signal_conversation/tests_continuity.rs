//! Tests for the continuous, multi-turn, per-operator Signal conversation
//! (issue #2577).
//!
//! Written test-first: [`super::channel::run_continuous`] and the
//! [`super::session_store`] functions are pinned by these integration tests.
//! They inject a fake transport ([`MockTransport`] — scripted signal-cli
//! JSON-RPC lines), a fake reasoner ([`RecordingAgent`], which captures the
//! per-turn prompt so we can prove the backend carried accumulated history), and
//! an explicit `tempdir` state root — no network, no live signal-cli, no
//! `~/.simard`, no real LLM.
//!
//! Scenarios pinned (matching the issue's required tests):
//!   1. Two messages from the SAME operator share one session; the second turn
//!      sees the first turn's content (continuity).
//!   2. A DIFFERENT operator address gets its own separate session (isolation).
//!   3. `/new` (and its `/reset` alias) starts a fresh session, retaining the
//!      previous one on disk and resetting the reasoner's context.
//!   4. The conversation persists and RESUMES across a simulated daemon restart
//!      (history replayed into a fresh backend, same session id).
//!   5. Linked-device loop-prevention still holds: Simard's own outbound synced
//!      back is NOT re-consumed as a new turn (device gate + echo suppression).
//!   6. `/help` is a lifecycle command — never persisted as a turn, never resets.

use std::sync::{Arc, Mutex};

use serde_json::json;
use serial_test::serial;

use super::config::SignalConfig;
use super::session_store;
use super::transport::MockTransport;
use super::{RuntimeCommandHandler, SignalConversation, run_continuous};
use crate::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use crate::error::SimardResult;
use crate::meeting_backend::{ConversationMessage, MeetingBackend, Role};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

const ACCOUNT: &str = "+15551230000";
const OPERATOR: &str = "+15557654321";
const OPERATOR_A: &str = "+12065551111";
const OPERATOR_B: &str = "+12065552222";
/// A distinctive canned reasoner reply so an echo of it is unambiguous.
const REPLY: &str = "SIMARD-REPLY-MARKER";

/// Shared capture of `(objective, prompt_preamble)` for every reasoner turn.
type Captured = Arc<Mutex<Vec<(String, String)>>>;

// ── Fake reasoner: records the prompt it was handed each turn ─────────────────

/// A [`BaseTypeSession`] that returns a canned reply and records the
/// `prompt_preamble` (which carries the accumulated conversation history) so a
/// test can prove continuity — that turn N's prompt contains turn N-1's content.
struct RecordingAgent {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
    reply: String,
    captured: Captured,
}

impl RecordingAgent {
    fn new(reply: &str, captured: Captured) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("signal-continuity-agent"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "recording-agent",
                    "test:signal-continuity-agent",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            is_open: true,
            is_closed: false,
            reply: reply.to_string(),
            captured,
        }
    }
}

impl BaseTypeSession for RecordingAgent {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
        self.captured
            .lock()
            .unwrap()
            .push((input.objective.clone(), input.prompt_preamble.clone()));
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: self.reply.clone(),
            evidence: Vec::new(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

// ── Test harness helpers ─────────────────────────────────────────────────────

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// A normal delivered `dataMessage` from `sender` (dedicated-number path).
fn receive_line(sender: &str, text: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "receive",
        "params": {"envelope": {"sourceNumber": sender, "dataMessage": {"message": text}}}
    })
    .to_string()
}

/// A `syncMessage.sentMessage` (Note-to-Self) line: the account sent `body` to
/// `destination` from device `source_device`.
fn sync_sent_line(source_device: u32, destination: &str, body: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "receive",
        "params": {"envelope": {
            "source": ACCOUNT,
            "sourceNumber": ACCOUNT,
            "sourceDevice": source_device,
            "syncMessage": {"sentMessage": {"destinationNumber": destination, "message": body}}
        }}
    })
    .to_string()
}

fn config(allowlist: &[&str], account: &str, own_device_id: Option<u32>) -> SignalConfig {
    SignalConfig {
        endpoint: "127.0.0.1:0".to_string(),
        account: account.to_string(),
        allowlist: allowlist.iter().map(|s| (*s).to_string()).collect(),
        read_only_unknown: false,
        own_device_id,
    }
}

fn channel(
    lines: Vec<String>,
    allowlist: &[&str],
    account: &str,
    own_device_id: Option<u32>,
) -> SignalConversation<MockTransport, RuntimeCommandHandler> {
    SignalConversation::new(
        MockTransport::with_lines(lines),
        RuntimeCommandHandler::new(),
        &config(allowlist, account, own_device_id),
    )
}

fn recording_backend(reply: &str, captured: &Captured) -> MeetingBackend {
    MeetingBackend::new_session(
        "signal",
        Box::new(RecordingAgent::new(reply, captured.clone())),
        None,
        "test-system-prompt".to_string(),
    )
}

/// Point the durable state root + handoff dir at a tempdir so nothing touches
/// `~/.simard`. `run_continuous` is passed the state root explicitly; this is
/// belt-and-suspenders for any ambient resolution.
fn set_hermetic_env(dir: &std::path::Path) {
    // SAFETY: every test that calls this is `#[serial(cognitive_memory)]`.
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", dir);
        std::env::set_var("SIMARD_HANDOFF_DIR", dir);
    }
}

fn joined(history: &[ConversationMessage]) -> String {
    history
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn user_turns(history: &[ConversationMessage]) -> Vec<&ConversationMessage> {
    history.iter().filter(|m| m.role == Role::User).collect()
}

// ── 1. Continuity: same operator, one session, second turn sees the first ────

#[test]
#[serial(cognitive_memory)]
fn same_operator_two_messages_share_one_continuous_session() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            receive_line(OPERATOR, "the code word is BANANA"),
            receive_line(OPERATOR, "what is the code word"),
        ],
        &[OPERATOR],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    // The reasoner saw accumulated history: turn 2's prompt carries turn 1's
    // user content AND turn 1's assistant reply, even though turn 2's own
    // message never mentions the code word.
    let turns = captured.lock().unwrap();
    assert_eq!(turns.len(), 2, "both turns reached the reasoner");
    let (obj2, preamble2) = &turns[1];
    assert!(
        preamble2.contains("BANANA"),
        "turn 2 must carry turn 1's content (continuity); prompt was {preamble2:?}"
    );
    assert!(
        preamble2.contains(REPLY),
        "turn 2 must carry the prior assistant reply too"
    );
    assert!(
        !obj2.contains("BANANA"),
        "sanity: turn 2's own message does not mention the code word"
    );

    // One durable session holds all four messages under a single session id.
    let sid = session_store::active_session_for(tmp.path(), OPERATOR)
        .unwrap()
        .expect("the operator has an active session");
    let sess = session_store::load_session_at(tmp.path(), &sid)
        .unwrap()
        .expect("the session was persisted");
    assert_eq!(
        sess.history.len(),
        4,
        "user1 + assistant1 + user2 + assistant2 accumulate in ONE session"
    );
    assert_eq!(
        session_store::list_sessions_at(tmp.path()).unwrap().len(),
        1,
        "one operator -> exactly one session"
    );
}

// ── 2. Isolation: a different operator gets a separate session ───────────────

#[test]
#[serial(cognitive_memory)]
fn distinct_operators_get_isolated_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            receive_line(OPERATOR_A, "APPLE from alpha"),
            receive_line(OPERATOR_B, "BANANA from bravo"),
        ],
        &[OPERATOR_A, OPERATOR_B],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    let sid_a = session_store::active_session_for(tmp.path(), OPERATOR_A)
        .unwrap()
        .expect("operator A has a session");
    let sid_b = session_store::active_session_for(tmp.path(), OPERATOR_B)
        .unwrap()
        .expect("operator B has a session");
    assert_ne!(sid_a, sid_b, "each operator gets a distinct session id");

    let a = session_store::load_session_at(tmp.path(), &sid_a)
        .unwrap()
        .unwrap();
    let b = session_store::load_session_at(tmp.path(), &sid_b)
        .unwrap()
        .unwrap();
    assert!(joined(&a.history).contains("APPLE"));
    assert!(
        !joined(&a.history).contains("BANANA"),
        "operator A's session must not contain operator B's message (no bleed)"
    );
    assert!(joined(&b.history).contains("BANANA"));
    assert!(!joined(&b.history).contains("APPLE"));

    assert_eq!(
        session_store::list_sessions_at(tmp.path()).unwrap().len(),
        2,
        "two operators -> two sessions"
    );
}

// ── 3. `/new` starts a fresh session, retaining the old one ──────────────────

#[test]
#[serial(cognitive_memory)]
fn new_command_starts_a_fresh_session_and_retains_the_old_one() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            receive_line(OPERATOR, "the code word is BANANA"),
            receive_line(OPERATOR, "/new"),
            receive_line(OPERATOR, "what is the code word"),
        ],
        &[OPERATOR],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    // The active session is the post-/new one and carries no pre-/new context.
    let active_sid = session_store::active_session_for(tmp.path(), OPERATOR)
        .unwrap()
        .expect("operator has an active session after /new");
    let active = session_store::load_session_at(tmp.path(), &active_sid)
        .unwrap()
        .unwrap();
    assert!(joined(&active.history).contains("what is the code word"));
    assert!(
        !joined(&active.history).contains("BANANA"),
        "the fresh session must not inherit the pre-/new history"
    );

    // The previous session is RETAINED on disk (two sessions total), and the
    // BANANA history lives in exactly one NON-active session.
    let all = session_store::list_sessions_at(tmp.path()).unwrap();
    assert_eq!(all.len(), 2, "/new mints a new session, keeps the old one");
    let banana_sessions: Vec<String> = all
        .iter()
        .filter(|m| {
            let s = session_store::load_session_at(tmp.path(), &m.session_id)
                .unwrap()
                .unwrap();
            joined(&s.history).contains("BANANA")
        })
        .map(|m| m.session_id.clone())
        .collect();
    assert_eq!(
        banana_sessions.len(),
        1,
        "the old history is preserved once"
    );
    assert_ne!(
        banana_sessions[0], active_sid,
        "the retained BANANA history is NOT the active session"
    );

    // `/new` never reaches the reasoner; context is reset for the next turn.
    let turns = captured.lock().unwrap();
    assert_eq!(
        turns.len(),
        2,
        "only the two real messages hit the reasoner; /new does not"
    );
    let (_obj, preamble_after_new) = &turns[1];
    assert!(
        !preamble_after_new.contains("BANANA"),
        "the reasoner's context is fresh after /new"
    );
}

#[test]
#[serial(cognitive_memory)]
fn reset_is_an_alias_for_new() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            receive_line(OPERATOR, "remember PLUM"),
            receive_line(OPERATOR, "/reset"),
            receive_line(OPERATOR, "hello again"),
        ],
        &[OPERATOR],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    let active_sid = session_store::active_session_for(tmp.path(), OPERATOR)
        .unwrap()
        .unwrap();
    let active = session_store::load_session_at(tmp.path(), &active_sid)
        .unwrap()
        .unwrap();
    assert!(joined(&active.history).contains("hello again"));
    assert!(
        !joined(&active.history).contains("PLUM"),
        "/reset must start a fresh session just like /new"
    );
    assert_eq!(
        session_store::list_sessions_at(tmp.path()).unwrap().len(),
        2,
        "/reset retains the prior session"
    );
}

// ── 4. Persistence + resume across a simulated restart ───────────────────────

#[test]
#[serial(cognitive_memory)]
fn conversation_resumes_after_a_simulated_restart() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    // Run 1: the operator sends one message, then the transport ends — the
    // daemon "stops". Its in-memory backends are dropped at the end of the run.
    let captured1: Captured = Arc::new(Mutex::new(Vec::new()));
    {
        let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured1));
        let mut ch = channel(
            vec![receive_line(OPERATOR, "the code word is BANANA")],
            &[OPERATOR],
            ACCOUNT,
            None,
        );
        block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();
    }
    let sid_before = session_store::active_session_for(tmp.path(), OPERATOR)
        .unwrap()
        .expect("session created in run 1");

    // Run 2: a FRESH process — new channel, brand-new (empty) backends, SAME
    // state root. The next message must resume the existing conversation.
    let captured2: Captured = Arc::new(Mutex::new(Vec::new()));
    {
        let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured2));
        let mut ch = channel(
            vec![receive_line(OPERATOR, "what is the code word")],
            &[OPERATOR],
            ACCOUNT,
            None,
        );
        block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();
    }

    // The reasoner in run 2 saw the REPLAYED prior history on its first turn.
    let turns2 = captured2.lock().unwrap();
    assert_eq!(turns2.len(), 1, "run 2 processed exactly one new message");
    let (_obj, preamble) = &turns2[0];
    assert!(
        preamble.contains("BANANA"),
        "restart must replay persisted history into the fresh backend; prompt was {preamble:?}"
    );

    // Same session id spans the restart; the store holds all four messages.
    let sid_after = session_store::active_session_for(tmp.path(), OPERATOR)
        .unwrap()
        .unwrap();
    assert_eq!(
        sid_before, sid_after,
        "restart RESUMES the same session, it does not start over"
    );
    let sess = session_store::load_session_at(tmp.path(), &sid_after)
        .unwrap()
        .unwrap();
    assert_eq!(sess.history.len(), 4, "both runs' turns are in one session");
    assert_eq!(
        session_store::list_sessions_at(tmp.path()).unwrap().len(),
        1,
        "no phantom second session was created on restart"
    );
}

// ── 5. Loop-prevention still holds under continuity ──────────────────────────

#[test]
#[serial(cognitive_memory)]
fn synced_echo_of_simards_reply_is_not_reconsumed_as_a_turn() {
    // Note-to-Self setup: the operator commands from device 1; Simard's reply,
    // synced back with the SAME body from device 1, must be dropped by echo
    // suppression — never re-consumed as a fresh user turn.
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            sync_sent_line(1, ACCOUNT, "hello from the primary phone"),
            // Simard's own reply, synced back (device 1, body == the reply).
            sync_sent_line(1, ACCOUNT, REPLY),
        ],
        &[ACCOUNT],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    let sid = session_store::active_session_for(tmp.path(), ACCOUNT)
        .unwrap()
        .expect("the note-to-self operator has a session");
    let sess = session_store::load_session_at(tmp.path(), &sid)
        .unwrap()
        .unwrap();
    assert_eq!(
        sess.history.len(),
        2,
        "only the one real turn (user + assistant); the echo was NOT re-consumed"
    );
    let users = user_turns(&sess.history);
    assert_eq!(users.len(), 1, "exactly one user turn");
    assert_eq!(users[0].content, "hello from the primary phone");
    assert_eq!(
        captured.lock().unwrap().len(),
        1,
        "the reasoner ran once; the echo never became a turn"
    );
}

#[test]
#[serial(cognitive_memory)]
fn reply_synced_from_linked_device_is_ignored() {
    // Simard's reply syncs back from the LINKED device (id >= 2); the primary
    // device-1 gate drops it before it can become a turn.
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            sync_sent_line(1, ACCOUNT, "primary phone command"),
            sync_sent_line(3, ACCOUNT, "whatever simard replied"),
        ],
        &[ACCOUNT],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    let sid = session_store::active_session_for(tmp.path(), ACCOUNT)
        .unwrap()
        .unwrap();
    let sess = session_store::load_session_at(tmp.path(), &sid)
        .unwrap()
        .unwrap();
    assert_eq!(
        user_turns(&sess.history).len(),
        1,
        "only the operator's device-1 message is a user turn"
    );
    assert_eq!(captured.lock().unwrap().len(), 1, "reasoner ran once");
}

// ── 6. `/help` is a lifecycle command, not a conversation turn ───────────────

#[test]
#[serial(cognitive_memory)]
fn help_does_not_persist_a_turn_or_reset_the_session() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let mut make_backend = |_op: &str| Ok(recording_backend(REPLY, &captured));

    let mut ch = channel(
        vec![
            receive_line(OPERATOR, "first real message"),
            receive_line(OPERATOR, "/help"),
            receive_line(OPERATOR, "second real message"),
        ],
        &[OPERATOR],
        ACCOUNT,
        None,
    );

    block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend)).unwrap();

    let sid = session_store::active_session_for(tmp.path(), OPERATOR)
        .unwrap()
        .unwrap();
    let sess = session_store::load_session_at(tmp.path(), &sid)
        .unwrap()
        .unwrap();
    let text = joined(&sess.history);
    assert!(text.contains("first real message"));
    assert!(text.contains("second real message"));
    assert!(
        !text.contains("/help"),
        "/help is a command, never persisted as a conversation turn"
    );
    assert_eq!(
        session_store::list_sessions_at(tmp.path()).unwrap().len(),
        1,
        "/help does not reset — the conversation stays continuous"
    );
    assert_eq!(
        captured.lock().unwrap().len(),
        2,
        "only the two real messages reach the reasoner; /help does not"
    );
}

// ── 7. No silent degradation: a make_backend failure propagates ──────────────

/// The real `run` builds each per-operator backend inside `make_backend` by
/// opening that operator's own cognitive-memory store (recall) and building the
/// enriched OODA-context system prompt (issue #2527 follow-up, wired onto the
/// per-operator `run_continuous`/`make_backend` structure of #2575/#2577). Both
/// of those steps can fail, which is why `make_backend` returns
/// `SimardResult<MeetingBackend>`.
///
/// This pins that a `make_backend` failure on a real turn PROPAGATES out of
/// `run_continuous` rather than being swallowed into a bare/empty reply or a
/// half-persisted conversation (PHILOSOPHY.md: no silent degradation).
#[test]
#[serial(cognitive_memory)]
fn make_backend_failure_propagates_for_a_real_turn() {
    let tmp = tempfile::tempdir().unwrap();
    set_hermetic_env(tmp.path());

    // Simulate recall/memory wiring failing while minting the per-operator
    // backend (e.g. the cognitive-memory store or the enriched prompt fails).
    let mut make_backend = |_op: &str| -> SimardResult<MeetingBackend> {
        Err(crate::error::SimardError::BridgeError(
            "simulated recall/memory failure while wiring the Signal backend".to_string(),
        ))
    };

    let mut ch = channel(
        vec![receive_line(OPERATOR, "hello simard")],
        &[OPERATOR],
        ACCOUNT,
        None,
    );

    let result = block_on(run_continuous(&mut ch, tmp.path(), &mut make_backend));
    assert!(
        result.is_err(),
        "a make_backend failure on a real turn must propagate out of run_continuous"
    );

    // Nothing was silently persisted as a completed conversation: no session
    // content file exists (the turn never reached the backend).
    assert!(
        session_store::list_sessions_at(tmp.path())
            .unwrap()
            .is_empty(),
        "a failed backend build must not persist a conversation turn"
    );
}
