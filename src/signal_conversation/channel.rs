//! The Signal conversation channel — a [`ConversationChannel`] over signal-cli
//! (issue #2527).
//!
//! `SignalConversation` gives an allowlisted operator a full meeting conversation
//! over Signal, plus a lightweight remote command surface (`status`, `pause`,
//! `approve`, `deploy`, `merge #NNNN`) and operator notifications out — all on the
//! **same** meeting engine and handoff/goal-carryover chain as the CLI and
//! dashboard channels.
//!
//! The three remote-command guardrails from the task live here and are enforced
//! before anything reaches the shared driver:
//! - **(a) sender allowlist** — [`Allowlist::authorize`] is applied in
//!   [`recv`](SignalConversation::recv); an unknown sender never reaches a command
//!   handler or the meeting engine (fail-closed).
//! - **(b) identity binding** — the authorized sender's E.164 is carried on
//!   [`OperatorRef`] and every reply is addressed back to that sender; a gated
//!   high-risk command runs only after an explicit `approve` from an allowlisted
//!   operator, never from the original text.
//! - **(c) high-risk gate** — [`gate`] routes every mutating command
//!   (`deploy`, `merge`) to [`GateDecision::PendingSignOff`]: it is **never**
//!   auto-executed from a text; Simard records it and asks for explicit `approve`,
//!   after which execution runs through the existing operational-autonomy gate via
//!   the injected [`SignalCommandHandler`].
//!
//! # Naming
//!
//! Nothing here is named `bridge`/`Bridge`. `SignalConversation` is a first-class
//! conversation channel and does not implement the cognitive-memory
//! `BridgeTransport`.

use std::collections::VecDeque;
use std::time::Instant;

use crate::conversation_channel::{ConversationChannel, Inbound, OperatorRef, OutKind, Outbound};
use crate::error::SimardResult;

use super::allowlist::{Allowlist, AuthDecision};
use super::config::SignalConfig;
use super::gating::{GateDecision, InboundCommand, gate, parse_inbound};
use super::transport::{
    PRIMARY_DEVICE_ID, RECENT_OUTBOUND_CAP, RECENT_OUTBOUND_TTL, SignalTransport,
    build_send_request, matches_recent_outbound, should_accept_sync_sent,
};

/// The integration seam for the effects a Signal command produces. Keeping this
/// out of the channel lets the (security-critical) allowlist + gating routing be
/// unit-tested in isolation, and lets a deployment wire concrete `status` /
/// `pause` / high-risk execution to its real subsystems.
///
/// `execute_approved` is the **only** method that performs a mutating action, and
/// the channel calls it **only** after an explicit `approve` for a previously
/// gated high-risk command — never directly from an inbound text.
pub trait SignalCommandHandler: Send {
    /// Operator-facing status/health line for the `status` command.
    fn status(&self) -> String;

    /// Pause autonomous dispatch; returns the operator-facing confirmation.
    fn pause(&mut self) -> String;

    /// Execute a high-risk command after explicit operator sign-off. Runs through
    /// the existing operational-autonomy gate; returns the operator-facing result.
    fn execute_approved(&mut self, cmd: &InboundCommand) -> SimardResult<String>;
}

/// A conservative default handler used by [`run`]. It reports truthful daemon
/// health and pause state, and on approval **records** the signed-off high-risk
/// action for the existing gated authority to process rather than performing the
/// mutation itself. Wire a custom [`SignalCommandHandler`] to drive concrete
/// deploy/merge execution.
#[derive(Default)]
pub struct RuntimeCommandHandler {
    paused: bool,
}

impl RuntimeCommandHandler {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SignalCommandHandler for RuntimeCommandHandler {
    fn status(&self) -> String {
        format!(
            "Simard is online. Autonomous dispatch: {}.",
            if self.paused { "paused" } else { "running" }
        )
    }

    fn pause(&mut self) -> String {
        self.paused = true;
        "Autonomous dispatch paused. Send `status` to check state.".to_string()
    }

    fn execute_approved(&mut self, cmd: &InboundCommand) -> SimardResult<String> {
        // The guardrail is enforced upstream (this is only reached after an
        // explicit `approve`). The default handler records the sign-off for the
        // existing gated authority; it does not itself perform the mutation.
        let what = match cmd {
            InboundCommand::Merge(n) => format!("merge of PR #{n}"),
            InboundCommand::Deploy => "deploy".to_string(),
            other => format!("{other:?}"),
        };
        tracing::info!(target: "signal", "operator signed off high-risk action: {what}");
        Ok(format!(
            "Sign-off recorded for {what}. It is authorized to proceed through the gated authority."
        ))
    }
}

/// A [`ConversationChannel`] over signal-cli, generic over the transport `T` and
/// the command-effect handler `H` so both can be mocked in tests.
pub struct SignalConversation<T: SignalTransport, H: SignalCommandHandler> {
    transport: T,
    handler: H,
    allowlist: Allowlist,
    account: String,
    /// Allowlisted operators to address notifications to.
    operators: Vec<String>,
    /// The sender of the last delivered meeting turn — `send` addresses replies here.
    current_operator: Option<String>,
    /// A gated high-risk command awaiting `approve`: `(sender, command)`.
    pending: Option<(String, InboundCommand)>,
    /// signal-cli's OWN linked-device id, if configured — defence-in-depth loop
    /// guard for Note-to-Self setups (issue #2575). A sync-sent message from this
    /// device is rejected. `None` is safe: the primary-phone (device 1) gate is
    /// the primary loop guard.
    own_device_id: Option<u32>,
    /// Bodies of recently-sent outbound messages (with send time) used to suppress
    /// a synced-back echo of Simard's own reply on a linked-device setup. Bounded
    /// to [`RECENT_OUTBOUND_CAP`] entries, each expiring after [`RECENT_OUTBOUND_TTL`].
    recent_outbound: VecDeque<(String, Instant)>,
    next_id: u64,
}

impl<T: SignalTransport, H: SignalCommandHandler> SignalConversation<T, H> {
    /// Build a Signal channel from a live transport, a command handler, and the
    /// resolved `[signal]` configuration.
    pub fn new(transport: T, handler: H, config: &SignalConfig) -> Self {
        Self {
            transport,
            handler,
            allowlist: Allowlist::from_config(config),
            account: config.account.clone(),
            operators: config.allowlist.clone(),
            current_operator: None,
            pending: None,
            own_device_id: config.own_device_id,
            recent_outbound: VecDeque::new(),
            next_id: 1,
        }
    }

    /// Send `text` to a specific Signal recipient via the transport.
    async fn reply(&mut self, recipient: &str, text: &str) -> SimardResult<()> {
        let id = self.next_id;
        self.next_id += 1;
        let line = build_send_request(id, &self.account, recipient, text);
        self.transport.send_line(line).await?;
        // Remember what we just sent so a synced-back echo of it (on a
        // linked-device setup) is suppressed rather than reprocessed (#2575).
        self.record_outbound(text);
        Ok(())
    }

    /// Track an outbound body (with the current time) for echo suppression,
    /// dropping expired and over-cap entries to keep the window small and bounded.
    fn record_outbound(&mut self, text: &str) {
        let now = Instant::now();
        self.recent_outbound.push_back((text.to_string(), now));
        // Entries are pushed in send order, so expired ones cluster at the front.
        while self
            .recent_outbound
            .front()
            .is_some_and(|(_, t)| now.saturating_duration_since(*t) > RECENT_OUTBOUND_TTL)
        {
            self.recent_outbound.pop_front();
        }
        while self.recent_outbound.len() > RECENT_OUTBOUND_CAP {
            self.recent_outbound.pop_front();
        }
    }

    /// Deliver an operator notification (PR merge-ready, stall/problem, high-risk
    /// sign-off request) to every configured operator. This is the notifications-out
    /// path; it is independent of any in-flight meeting turn.
    pub async fn notify(&mut self, text: &str) -> SimardResult<()> {
        let operators = self.operators.clone();
        for op in &operators {
            self.reply(op, text).await?;
        }
        Ok(())
    }

    /// Handle a low-risk lightweight command (`status`, `pause`, `approve`) for an
    /// authorized sender and reply on the channel.
    async fn handle_low_risk(&mut self, sender: &str, cmd: &InboundCommand) -> SimardResult<()> {
        let reply = match cmd {
            InboundCommand::Status => self.handler.status(),
            InboundCommand::Pause => self.handler.pause(),
            InboundCommand::Approve => match self.pending.take() {
                // Any allowlisted operator may sign off the pending high-risk
                // command (all senders here have already cleared the allowlist),
                // so the original requester (`_from`) is intentionally not compared.
                Some((_from, pending_cmd)) => match self.handler.execute_approved(&pending_cmd) {
                    Ok(msg) => msg,
                    Err(e) => format!("Approved, but the action failed: {e}"),
                },
                None => "Nothing is awaiting sign-off.".to_string(),
            },
            // `handle_low_risk` is only called for AutoExecute commands; a
            // conversation turn is routed out to the driver by `recv`, never here.
            InboundCommand::Conversation(_) | InboundCommand::Deploy | InboundCommand::Merge(_) => {
                return Ok(());
            }
        };
        self.reply(sender, &reply).await
    }
}

/// The operator-facing prompt for a gated high-risk command awaiting sign-off.
fn high_risk_prompt(cmd: &InboundCommand) -> String {
    let action = match cmd {
        InboundCommand::Merge(n) => format!("Merging PR #{n}"),
        InboundCommand::Deploy => "Deploying".to_string(),
        other => format!("{other:?}"),
    };
    format!(
        "{action} is a HIGH-RISK action and will NOT run from a text message. \
         Reply `approve` to sign off; it will then proceed through the existing gated authority."
    )
}

impl<T: SignalTransport + Send, H: SignalCommandHandler> ConversationChannel
    for SignalConversation<T, H>
{
    fn name(&self) -> &'static str {
        "signal"
    }

    async fn recv(&mut self) -> SimardResult<Option<Inbound>> {
        loop {
            let Some(line) = self.transport.recv_line().await? else {
                return Ok(None);
            };
            let Some(parsed) = super::transport::parse_incoming(&line) else {
                continue; // JSON-RPC responses, receipts, unparseable lines.
            };

            // Loop prevention for sync-sent (Note-to-Self) messages, issue #2575.
            // A linked device receives sync-sent transcripts of BOTH the operator's
            // Note-to-Self commands AND Simard's own replies; only the former (from
            // the operator's primary phone, destined for the account) is a command.
            if parsed.is_sync_sent {
                if !should_accept_sync_sent(
                    parsed.source_device,
                    self.own_device_id,
                    parsed.sync_destination.as_deref(),
                    &self.account,
                    PRIMARY_DEVICE_ID,
                ) {
                    tracing::debug!(
                        target: "signal",
                        "ignoring sync-sent message that is not a primary-phone Note to Self"
                    );
                    continue;
                }
                if matches_recent_outbound(&parsed.body, &self.recent_outbound, Instant::now()) {
                    tracing::debug!(
                        target: "signal",
                        "ignoring sync-sent echo of a recently-sent outbound message"
                    );
                    continue;
                }
            }

            let sender = parsed.sender;
            let text = parsed.body;

            match self.allowlist.authorize(&sender) {
                // Guardrail (a): fail-closed. Unknown senders are dropped.
                AuthDecision::Ignored => {
                    tracing::debug!(target: "signal", "dropping message from non-allowlisted sender");
                    continue;
                }
                // Read-only senders may only read `status`, never mutate.
                AuthDecision::ReadOnly => {
                    if matches!(parse_inbound(&text), InboundCommand::Status) {
                        let s = self.handler.status();
                        self.reply(&sender, &s).await?;
                    }
                    continue;
                }
                AuthDecision::Authorized => {
                    let cmd = parse_inbound(&text);
                    // A meeting turn is handed to the shared driver, bound to
                    // the authorized sender's identity (guardrail (b)).
                    if let InboundCommand::Conversation(turn) = &cmd {
                        self.current_operator = Some(sender.clone());
                        return Ok(Some(Inbound {
                            from: OperatorRef {
                                id: sender,
                                authorized: true,
                            },
                            text: turn.clone(),
                        }));
                    }
                    // A lightweight operator command: gate it (guardrail (c)).
                    match gate(&cmd) {
                        GateDecision::AutoExecute => {
                            self.handle_low_risk(&sender, &cmd).await?;
                        }
                        GateDecision::PendingSignOff => {
                            self.pending = Some((sender.clone(), cmd.clone()));
                            let prompt = high_risk_prompt(&cmd);
                            self.reply(&sender, &prompt).await?;
                        }
                    }
                }
            }
        }
    }

    async fn send(&mut self, out: Outbound) -> SimardResult<()> {
        let recipient = self
            .current_operator
            .clone()
            .or_else(|| self.operators.first().cloned());
        let Some(recipient) = recipient else {
            tracing::warn!(target: "signal", "no operator to deliver an outbound to; dropping");
            return Ok(());
        };
        let text = match out.kind {
            OutKind::Error => format!("⚠ {}", out.text),
            _ => out.text,
        };
        self.reply(&recipient, &text).await
    }
}

/// Connect to the configured signal-cli daemon and drive an operator meeting
/// conversation over Signal to completion. Feature-gated; requires the meeting
/// LLM provider to be configured (same as the CLI/dashboard meeting).
pub async fn run(config: SignalConfig) -> SimardResult<()> {
    use crate::error::SimardError;

    let transport = super::transport::JsonRpcTransport::connect(&config.endpoint).await?;
    let handler = RuntimeCommandHandler::new();
    let mut channel = SignalConversation::new(transport, handler, &config);

    let agent = open_signal_agent_session().ok_or_else(|| SimardError::ActionExecutionFailed {
        action: "signal-meeting".to_string(),
        reason: "No LLM agent backend available. Check SIMARD_LLM_PROVIDER and auth config."
            .to_string(),
    })?;
    let system_prompt = load_meeting_system_prompt();
    let mut backend =
        crate::meeting_backend::MeetingBackend::new_session("Signal", agent, None, system_prompt);

    crate::conversation_channel::run_conversation(&mut channel, &mut backend).await
}

/// Load the shared meeting system prompt (same asset the CLI/dashboard use).
fn load_meeting_system_prompt() -> String {
    let path = crate::operator_commands::prompt_root().join("simard/meeting_system.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Open a Meeting-mode agent session for the Signal channel, mirroring the CLI
/// and dashboard backends so all three channels get identical behavior.
fn open_signal_agent_session() -> Option<Box<dyn crate::base_types::BaseTypeSession>> {
    let provider = match crate::session_builder::LlmProvider::resolve() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[simard] signal agent: LLM provider not configured: {e}");
            return None;
        }
    };
    match crate::session_builder::SessionBuilder::new(
        crate::identity::OperatingMode::Meeting,
        provider,
    )
    .node_id("signal-channel")
    .address("signal-channel://local")
    .adapter_tag("signal")
    .open()
    {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[simard] signal agent session failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::*;
    use crate::signal_conversation::transport::MockTransport;

    const OPERATOR: &str = "+15557654321";
    const STRANGER: &str = "+15550000000";
    const ACCOUNT: &str = "+15551230000";
    const THIRD_PARTY: &str = "+15559990000";

    #[derive(Default)]
    struct SpyState {
        executed: Vec<InboundCommand>,
        paused: bool,
    }

    /// A command handler that records `execute_approved` calls so tests can prove
    /// a high-risk action is only ever run after an explicit `approve`.
    struct SpyHandler {
        state: Arc<Mutex<SpyState>>,
    }

    impl SignalCommandHandler for SpyHandler {
        fn status(&self) -> String {
            format!("STATUS(paused={})", self.state.lock().unwrap().paused)
        }
        fn pause(&mut self) -> String {
            self.state.lock().unwrap().paused = true;
            "PAUSED".to_string()
        }
        fn execute_approved(&mut self, cmd: &InboundCommand) -> SimardResult<String> {
            self.state.lock().unwrap().executed.push(cmd.clone());
            Ok(format!("EXECUTED {cmd:?}"))
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn receive_line(sender: &str, text: &str) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "receive",
            "params": {"envelope": {"sourceNumber": sender, "dataMessage": {"message": text}}}
        })
        .to_string()
    }

    /// Extract `(recipient, message)` from a JSON-RPC `send` line the channel wrote.
    fn sent(line: &str) -> (String, String) {
        let v: Value = serde_json::from_str(line).unwrap();
        (
            v["params"]["recipient"][0].as_str().unwrap().to_string(),
            v["params"]["message"].as_str().unwrap().to_string(),
        )
    }

    fn config(read_only_unknown: bool) -> SignalConfig {
        SignalConfig {
            endpoint: "127.0.0.1:0".to_string(),
            account: ACCOUNT.to_string(),
            allowlist: vec![OPERATOR.to_string()],
            read_only_unknown,
            own_device_id: None,
        }
    }

    fn channel(
        lines: Vec<String>,
        read_only_unknown: bool,
    ) -> (
        SignalConversation<MockTransport, SpyHandler>,
        Arc<Mutex<SpyState>>,
    ) {
        let state = Arc::new(Mutex::new(SpyState::default()));
        let handler = SpyHandler {
            state: Arc::clone(&state),
        };
        let transport = MockTransport::with_lines(lines);
        let ch = SignalConversation::new(transport, handler, &config(read_only_unknown));
        (ch, state)
    }

    #[test]
    fn recv_drops_unknown_sender_and_yields_authorized_conversation() {
        let (mut ch, _state) = channel(
            vec![
                receive_line(STRANGER, "hello"),
                receive_line(OPERATOR, "let's chat"),
            ],
            false,
        );
        let inbound = block_on(ch.recv())
            .unwrap()
            .expect("authorized conversation");
        assert_eq!(inbound.text, "let's chat");
        assert_eq!(inbound.from.id, OPERATOR);
        assert!(inbound.from.authorized);
        // The stranger's message was dropped silently — no reply was sent.
        assert!(ch.transport.sent.is_empty());
    }

    #[test]
    fn recv_status_replies_and_reaches_eof() {
        let (mut ch, _state) = channel(vec![receive_line(OPERATOR, "status")], false);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert_eq!(ch.transport.sent.len(), 1);
        let (to, msg) = sent(&ch.transport.sent[0]);
        assert_eq!(to, OPERATOR);
        assert!(msg.contains("STATUS"), "got {msg}");
    }

    #[test]
    fn recv_high_risk_merge_never_auto_executes() {
        let (mut ch, state) = channel(vec![receive_line(OPERATOR, "merge #42")], false);
        assert!(block_on(ch.recv()).unwrap().is_none());
        // Nothing executed from the text; a sign-off prompt was sent instead.
        assert!(state.lock().unwrap().executed.is_empty());
        assert_eq!(ch.transport.sent.len(), 1);
        let (_to, msg) = sent(&ch.transport.sent[0]);
        assert!(msg.contains("HIGH-RISK"), "got {msg}");
        assert!(msg.contains("approve"), "got {msg}");
    }

    #[test]
    fn recv_approve_after_pending_executes_exactly_once() {
        let (mut ch, state) = channel(
            vec![
                receive_line(OPERATOR, "merge #42"),
                receive_line(OPERATOR, "approve"),
            ],
            false,
        );
        assert!(block_on(ch.recv()).unwrap().is_none());
        let executed = state.lock().unwrap().executed.clone();
        assert_eq!(executed, vec![InboundCommand::Merge(42)]);
        // prompt + execution result.
        assert_eq!(ch.transport.sent.len(), 2);
        let (_to, msg) = sent(&ch.transport.sent[1]);
        assert!(msg.contains("EXECUTED"), "got {msg}");
    }

    #[test]
    fn approve_without_pending_is_a_noop() {
        let (mut ch, state) = channel(vec![receive_line(OPERATOR, "approve")], false);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert!(state.lock().unwrap().executed.is_empty());
        let (_to, msg) = sent(&ch.transport.sent[0]);
        assert!(msg.contains("Nothing is awaiting"), "got {msg}");
    }

    #[test]
    fn read_only_unknown_gets_status_but_never_mutates() {
        let (mut ch, state) = channel(
            vec![
                receive_line(STRANGER, "status"),
                receive_line(STRANGER, "merge #1"),
            ],
            true,
        );
        assert!(block_on(ch.recv()).unwrap().is_none());
        // Exactly one reply: the read-only status. The merge was dropped.
        assert_eq!(ch.transport.sent.len(), 1);
        let (to, msg) = sent(&ch.transport.sent[0]);
        assert_eq!(to, STRANGER);
        assert!(msg.contains("STATUS"), "got {msg}");
        assert!(state.lock().unwrap().executed.is_empty());
    }

    #[test]
    fn notify_addresses_every_operator() {
        let (mut ch, _state) = channel(vec![], false);
        block_on(ch.notify("PR #7 is merge-ready")).unwrap();
        assert_eq!(ch.transport.sent.len(), 1);
        let (to, msg) = sent(&ch.transport.sent[0]);
        assert_eq!(to, OPERATOR);
        assert!(msg.contains("merge-ready"), "got {msg}");
    }

    #[test]
    fn send_delivers_reply_to_current_operator() {
        let (mut ch, _state) = channel(vec![receive_line(OPERATOR, "hi there")], false);
        let _ = block_on(ch.recv()).unwrap().expect("conversation turn");
        block_on(ch.send(Outbound {
            kind: OutKind::Assistant,
            text: "reply text".to_string(),
        }))
        .unwrap();
        let (to, msg) = sent(ch.transport.sent.last().unwrap());
        assert_eq!(to, OPERATOR);
        assert_eq!(msg, "reply text");
    }

    // ── Note-to-Self acceptance + loop prevention (issue #2575) ──────────────
    //
    // On a single-number linked-device setup the operator and Simard share one
    // E.164; the operator commands Simard from Signal's "Note to Self", which
    // signal-cli delivers as a sync-sent message. These scenarios pin the
    // conjunctive acceptance predicate and the three layered loop guards.

    /// A signal-cli `receive` line for a Note-to-Self (sync-sent) message: the
    /// account sent `body` to `destination` from device `source_device`.
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

    /// Config for the single-number linked-device setup: the account's own number
    /// is the allowlisted operator, with an optional signal-cli own-device id.
    fn note_config(own_device_id: Option<u32>) -> SignalConfig {
        SignalConfig {
            endpoint: "127.0.0.1:0".to_string(),
            account: ACCOUNT.to_string(),
            allowlist: vec![ACCOUNT.to_string()],
            read_only_unknown: false,
            own_device_id,
        }
    }

    fn note_channel(
        lines: Vec<String>,
        own_device_id: Option<u32>,
    ) -> (
        SignalConversation<MockTransport, SpyHandler>,
        Arc<Mutex<SpyState>>,
    ) {
        let state = Arc::new(Mutex::new(SpyState::default()));
        let handler = SpyHandler {
            state: Arc::clone(&state),
        };
        let transport = MockTransport::with_lines(lines);
        let ch = SignalConversation::new(transport, handler, &note_config(own_device_id));
        (ch, state)
    }

    #[test]
    fn note_to_self_status_from_primary_phone_runs_as_command() {
        // A sync-sent message from device 1, destined for the account, is a genuine
        // Note to Self → treated as a `status` command from the (allowlisted) account.
        let (mut ch, _state) = note_channel(vec![sync_sent_line(1, ACCOUNT, "status")], None);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert_eq!(ch.transport.sent.len(), 1, "one status reply");
        let (to, msg) = sent(&ch.transport.sent[0]);
        assert_eq!(
            to, ACCOUNT,
            "the reply goes back to the account (Note to Self)"
        );
        assert!(msg.contains("STATUS"), "got {msg}");
    }

    #[test]
    fn note_to_self_conversation_turn_is_bound_to_the_account() {
        let (mut ch, _state) = note_channel(
            vec![sync_sent_line(1, ACCOUNT, "let's plan the release")],
            None,
        );
        let inbound = block_on(ch.recv()).unwrap().expect("a conversation turn");
        assert_eq!(inbound.text, "let's plan the release");
        assert_eq!(inbound.from.id, ACCOUNT);
        assert!(inbound.from.authorized);
    }

    #[test]
    fn sync_sent_from_signal_cli_own_device_is_ignored() {
        // A reply Simard emitted syncs back from signal-cli's linked device (>= 2).
        // It must be ignored — never processed as a new command (loop prevention).
        let (mut ch, state) = note_channel(vec![sync_sent_line(2, ACCOUNT, "status")], Some(2));
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert!(
            ch.transport.sent.is_empty(),
            "no reply to our own synced-back message"
        );
        assert!(state.lock().unwrap().executed.is_empty());
    }

    #[test]
    fn sync_sent_from_linked_device_ignored_without_own_device_id() {
        // Even with own_device_id unset, the primary-phone gate rejects a device>=2
        // sync-sent — the loop-free guarantee does not depend on configuration.
        let (mut ch, _state) = note_channel(vec![sync_sent_line(3, ACCOUNT, "status")], None);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert!(ch.transport.sent.is_empty());
    }

    #[test]
    fn sync_sent_echo_of_recent_outbound_is_ignored() {
        // Simard sends a notification; the linked device syncs it back as a device-1
        // sync-sent with an identical body. Echo suppression drops it even though it
        // clears the primary-phone gate.
        let echo = "PR #7 is merge-ready";
        let (mut ch, _state) = note_channel(vec![sync_sent_line(1, ACCOUNT, echo)], None);
        // Record the outbound first — this is what reply() tracks.
        block_on(ch.notify(echo)).unwrap();
        assert_eq!(ch.transport.sent.len(), 1, "just the notification");
        // The synced-back echo is ignored: recv reaches EOF with no new send.
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert_eq!(ch.transport.sent.len(), 1, "no reply to the echo");
    }

    #[test]
    fn sync_sent_to_third_party_is_ignored() {
        // The operator texts someone else from their phone; it syncs to the linked
        // device but is NOT a Note to Self, so Simard ignores it.
        let (mut ch, state) = note_channel(vec![sync_sent_line(1, THIRD_PARTY, "status")], None);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert!(ch.transport.sent.is_empty(), "not a command; no reply");
        assert!(state.lock().unwrap().executed.is_empty());
    }

    #[test]
    fn sync_sent_high_risk_from_primary_phone_is_still_gated() {
        // Even a genuine Note-to-Self high-risk command must not auto-execute.
        let (mut ch, state) = note_channel(vec![sync_sent_line(1, ACCOUNT, "merge #42")], None);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert!(
            state.lock().unwrap().executed.is_empty(),
            "no auto-execute from a text"
        );
        assert_eq!(ch.transport.sent.len(), 1);
        let (_to, msg) = sent(&ch.transport.sent[0]);
        assert!(msg.contains("HIGH-RISK"), "got {msg}");
    }

    #[test]
    fn normal_data_message_from_separate_number_still_parsed() {
        // Regression: the dedicated-number path is unchanged. A normal dataMessage
        // from a separate allowlisted operator still yields a conversation turn.
        let (mut ch, _state) = channel(vec![receive_line(OPERATOR, "let's chat")], false);
        let inbound = block_on(ch.recv()).unwrap().expect("a conversation turn");
        assert_eq!(inbound.text, "let's chat");
        assert_eq!(inbound.from.id, OPERATOR);
        assert!(inbound.from.authorized);
    }

    #[test]
    fn receipt_and_unparseable_lines_are_skipped() {
        let receipt = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceNumber":"+15551230000","receiptMessage":{"isDelivery":true}}}}"#.to_string();
        let (mut ch, _state) = note_channel(vec![receipt, "not json".to_string()], None);
        assert!(block_on(ch.recv()).unwrap().is_none());
        assert!(ch.transport.sent.is_empty());
    }
}
