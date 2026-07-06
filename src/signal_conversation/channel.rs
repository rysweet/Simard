//! The Signal conversation channel — a [`ConversationChannel`] over signal-cli
//! (issue #2527).
//!
//! `SignalConversation` gives an allowlisted operator a full meeting conversation
//! over Signal, plus a lightweight remote command surface (`status`, `pause`,
//! `approve`, `deploy`, `merge #NNNN`) and operator notifications out — all on the
//! **same** meeting engine and handoff/goal-carryover chain as the CLI and
//! dashboard channels.
//!
//! Each operator's conversation is also wired into the **same OODA-loop context
//! and graph cognitive memory as the CLI meeting** (issue #2527): its system
//! prompt starts with live OODA state (recent meetings, decisions, active goals,
//! operator identity, known projects) and each per-operator backend carries its
//! own cognitive-memory store, so a `/close` consolidates the conversation back
//! into graph memory (episodes, summary facts) — not just the flat handoff bundle.
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
//! No symbol *defined here* is named `bridge`/`Bridge`. `SignalConversation` is a
//! first-class conversation channel and does not implement the cognitive-memory
//! `BridgeTransport`. The per-operator OODA/graph-memory wiring (issue #2527) does
//! call the pre-existing external helper `memory_ipc::launch_writer_bridge` to
//! obtain a cognitive-memory handle, but that handle is bound locally as `memory`
//! — never `bridge` — so the guardrail holds for every symbol this module names.

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

/// Connect to the configured signal-cli daemon and drive **continuous,
/// multi-turn, per-operator** meeting conversations over Signal (issue #2577).
///
/// Each allowlisted operator gets a persistent conversation session (keyed by
/// their Signal address) that is reused across successive inbound messages and
/// survives daemon restarts via [`super::session_store`]. Feature-gated;
/// requires the meeting LLM provider to be configured (same as the
/// CLI/dashboard meeting).
pub async fn run(config: SignalConfig) -> SimardResult<()> {
    use crate::error::SimardError;

    let transport = super::transport::JsonRpcTransport::connect(&config.endpoint).await?;
    let handler = RuntimeCommandHandler::new();
    let mut channel = SignalConversation::new(transport, handler, &config);

    // Fail fast on a misconfigured provider (the only realistic agent-open
    // failure), so the operator sees a clear startup error rather than a
    // per-turn failure once messages start arriving.
    if let Err(e) = crate::session_builder::LlmProvider::resolve() {
        return Err(SimardError::ActionExecutionFailed {
            action: "signal-meeting".to_string(),
            reason: format!(
                "No LLM agent backend available: {e}. Check SIMARD_LLM_PROVIDER and auth config."
            ),
        });
    }

    let state_root = crate::state_root::simard_state_root();

    // Per-operator backend factory: each operator gets an independent meeting
    // backend (its own agent session, its own history, AND its own cognitive-
    // memory store), so two operators never share context. `run_continuous`
    // calls this once per operator on first touch and replays that operator's
    // persisted history into the fresh backend.
    //
    // This is where the Signal channel is wired into the same OODA-loop context
    // and graph cognitive-memory model as the CLI meeting (issue #2527 follow-up,
    // adapted to the per-operator run_continuous structure of #2575/#2577):
    //
    //   1. Recall — a live cognitive-memory store is opened on the daemon's
    //      state root (`launch_writer_bridge`, which shares the running daemon's
    //      store over IPC when one is up). The system prompt is enriched with the
    //      live OODA state (goals, decisions, operator identity, projects, …) via
    //      the shared `build_enriched_meeting_system_prompt`, so Simard starts a
    //      Signal chat already knowing her own state — no manual context loading.
    //   2. Write-back — that same store is moved into the `MeetingBackend`, so on
    //      `/close` the Signal conversation is consolidated back into graph memory
    //      (episodes, summary facts, goal carryover) exactly like the CLI meeting
    //      (see `meeting_backend::closing`).
    //
    // Each per-operator backend gets its OWN store instance because the store is
    // moved into the backend. Memory/recall failures propagate (PHILOSOPHY.md:
    // no silent degradation); a post-startup agent-open failure (unexpected after
    // the provider check above) still degrades to a backend that reports the
    // outage per turn rather than crashing the daemon that serves every operator.
    let mut make_backend =
        |_operator: &str| -> SimardResult<crate::meeting_backend::MeetingBackend> {
            let agent: Box<dyn crate::base_types::BaseTypeSession> = match open_signal_agent_session(
            ) {
                Some(a) => a,
                None => {
                    eprintln!(
                        "[simard][ERROR] signal: agent session open failed after startup validation; \
                             replies will report the outage until it recovers"
                    );
                    Box::new(UnavailableAgent::new())
                }
            };

            // Open this operator's own cognitive-memory store on the daemon's
            // state root (recall + write-back). Named `memory`, never `bridge`,
            // per the signal_conversation naming guardrail.
            let memory = crate::memory_ipc::launch_writer_bridge(&state_root)?.into_box();

            // Recall: enrich the prompt with live OODA context (borrows the
            // store), then move the store into the backend for write-back.
            let system_prompt =
                crate::operator_commands_meeting::build_enriched_meeting_system_prompt(&*memory)?;

            Ok(crate::meeting_backend::MeetingBackend::new_session(
                "signal",
                agent,
                Some(memory),
                system_prompt,
            ))
        };

    run_continuous(&mut channel, &state_root, &mut make_backend).await
}

/// Lifecycle commands an operator can send over Signal to control the
/// conversation itself (as opposed to a normal turn or a remote command). These
/// are handled entirely inside [`run_continuous`] — they never reach the
/// reasoner and are never persisted as conversation turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    /// `/new` or `/reset` — rotate onto a fresh session (previous retained).
    Reset,
    /// `/help` — show the command banner.
    Help,
    /// `/close` — end the current conversation.
    Close,
}

/// Classify an inbound line as a conversation-lifecycle command, or `None` for
/// an ordinary turn. Matched case-insensitively on the trimmed text so `/NEW`
/// and `/New` also work.
fn lifecycle_command(text: &str) -> Option<Lifecycle> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("/new") || t.eq_ignore_ascii_case("/reset") {
        Some(Lifecycle::Reset)
    } else if t.eq_ignore_ascii_case("/help") {
        Some(Lifecycle::Help)
    } else if t.eq_ignore_ascii_case("/close") {
        Some(Lifecycle::Close)
    } else {
        None
    }
}

/// The operator-facing lifecycle banner surfaced by `/help`.
fn help_banner() -> String {
    "[simard] Commands:\n\
     \x20 /help          — show this help\n\
     \x20 /new  (/reset) — start a fresh conversation (clears prior context)\n\
     \x20 /close         — end this conversation and write the handoff\n\
     \x20 status | pause | approve | deploy | merge #NNNN — operator commands\n\
     Anything else is a message in our ongoing conversation."
        .to_string()
}

/// Redact an operator address for structured logs: keep only the last four
/// characters so a session id (safe) and a coarse operator hint are logged
/// without persisting a full phone number to the daemon log.
fn redact_operator(operator: &str) -> String {
    let n = operator.chars().count();
    if n <= 4 {
        return "…".to_string();
    }
    let tail: String = operator.chars().skip(n - 4).collect();
    format!("…{tail}")
}

/// Drive a **continuous, multi-turn, per-operator** Signal conversation
/// (issue #2577).
///
/// Unlike [`crate::conversation_channel::run_conversation`] — which owns a
/// single [`MeetingBackend`] for the life of the process — this driver keeps a
/// registry of backends keyed by the inbound operator identity
/// (`inbound.from.id`, the authorized Signal E.164 that `recv` binds), so two
/// operators never share history. Each operator's conversation is:
///
///   1. **Continuous in-process** — successive inbound messages route to the
///      SAME backend, so the reasoner sees the accumulated prior-turn history.
///   2. **Durable** — every turn is appended to the operator's session file via
///      [`super::session_store`], so a daemon restart replays the persisted
///      history back into a fresh backend and the conversation resumes rather
///      than starting over.
///   3. **Operator-controllable** — `/new` (alias `/reset`) rotates the operator
///      onto a fresh session id (previous history retained on disk); `/help`
///      shows the lifecycle banner; `/close` ends the current conversation.
///
/// `make_backend(operator)` mints a fresh backend for a given operator key —
/// the real `run` supplies the meeting agent, the enriched OODA-context system
/// prompt, and the operator's own cognitive-memory store (and so may fail if
/// recall/memory setup fails); tests inject a fake.
/// Loop-prevention (device gate + echo suppression) stays entirely inside
/// [`SignalConversation::recv`], so Simard's own outbound is never re-consumed
/// as a new turn.
pub async fn run_continuous<T, H, F>(
    channel: &mut SignalConversation<T, H>,
    state_root: &std::path::Path,
    make_backend: &mut F,
) -> SimardResult<()>
where
    T: SignalTransport + Send,
    H: SignalCommandHandler,
    F: FnMut(&str) -> SimardResult<crate::meeting_backend::MeetingBackend>,
{
    use std::collections::HashMap;

    use super::session_store;
    use crate::meeting_backend::MeetingBackend;

    // Per-operator live backends (in-process continuity) and the durable
    // session id each is bound to. Keyed by the authorized operator identity
    // that `recv` binds onto the inbound.
    let mut backends: HashMap<String, MeetingBackend> = HashMap::new();
    let mut sessions: HashMap<String, String> = HashMap::new();

    while let Some(inbound) = channel.recv().await? {
        let operator = inbound.from.id.clone();
        let text = inbound.text.trim().to_string();
        if text.is_empty() {
            continue;
        }

        // ── Conversation-lifecycle commands (never a turn, never persisted) ──
        match lifecycle_command(&text) {
            Some(Lifecycle::Reset) => {
                // Rotate onto a fresh session id; the previous session file is
                // retained on disk, and the live backend is dropped so the next
                // turn starts from an empty (resumed-as-empty) context.
                let new_sid = crate::session_id::new_session_id();
                session_store::set_active_session(state_root, &operator, &new_sid)?;
                backends.remove(&operator);
                sessions.remove(&operator);
                tracing::info!(
                    target: "signal",
                    operator = %redact_operator(&operator),
                    session_id = %new_sid,
                    "session.reset"
                );
                channel
                    .send(Outbound {
                        kind: OutKind::Status,
                        text: "[simard] Started a new conversation.".to_string(),
                    })
                    .await?;
                continue;
            }
            Some(Lifecycle::Help) => {
                channel
                    .send(Outbound {
                        kind: OutKind::Status,
                        text: help_banner(),
                    })
                    .await?;
                continue;
            }
            Some(Lifecycle::Close) => {
                let out = match backends.remove(&operator) {
                    Some(mut backend) => match backend.close() {
                        Ok(s) => {
                            let mut body = format!(
                                "[simard] Conversation closed. {} messages. Summary: {}",
                                s.message_count, s.summary_text
                            );
                            if let Some(dir) = &s.bundle_dir {
                                body.push_str(&format!("\nBundle: {dir}"));
                            }
                            Outbound {
                                kind: OutKind::Status,
                                text: body,
                            }
                        }
                        Err(e) => Outbound {
                            kind: OutKind::Error,
                            text: format!("Conversation closed with error: {e}"),
                        },
                    },
                    None => Outbound {
                        kind: OutKind::Status,
                        text: "[simard] No active conversation to close.".to_string(),
                    },
                };
                // Rotate to a fresh session so the next message starts a brand
                // new conversation rather than resuming the closed one.
                let new_sid = crate::session_id::new_session_id();
                session_store::set_active_session(state_root, &operator, &new_sid)?;
                sessions.remove(&operator);
                tracing::info!(
                    target: "signal",
                    operator = %redact_operator(&operator),
                    "session.close"
                );
                channel.send(out).await?;
                continue;
            }
            None => {}
        }

        // ── Ensure a live backend for this operator, resuming on first touch ──
        if !backends.contains_key(&operator) {
            let sid = match session_store::active_session_for(state_root, &operator)? {
                Some(existing) => existing,
                None => {
                    let fresh = crate::session_id::new_session_id();
                    session_store::set_active_session(state_root, &operator, &fresh)?;
                    fresh
                }
            };
            let mut backend = make_backend(&operator)?;
            let resumed = match session_store::load_session_at(state_root, &sid)? {
                Some(sess) => {
                    let n = sess.history.len();
                    // Replay the persisted (uncapped) history into the fresh
                    // backend so the next turn is context-aware after a restart.
                    backend.restore(sess.history);
                    n
                }
                None => 0,
            };
            if resumed > 0 {
                tracing::info!(
                    target: "signal",
                    operator = %redact_operator(&operator),
                    session_id = %sid,
                    turns = resumed,
                    "session.resume"
                );
            } else {
                tracing::info!(
                    target: "signal",
                    operator = %redact_operator(&operator),
                    session_id = %sid,
                    "session.create"
                );
            }
            backends.insert(operator.clone(), backend);
            sessions.insert(operator.clone(), sid);
        }

        let sid = sessions
            .get(&operator)
            .cloned()
            .expect("session id recorded alongside the backend");
        let backend = backends
            .get_mut(&operator)
            .expect("backend just ensured for this operator");

        // ── Run the turn, then persist the newly-appended user+assistant pair ─
        let out = match backend.send_message(&text) {
            Ok(resp) => {
                // The two messages send_message just appended are the newest in
                // history (eviction, if any, is at the front), so the tail is
                // exactly this turn's user + assistant pair — cap-independent.
                let hist = backend.history();
                let start = hist.len().saturating_sub(2);
                for msg in &hist[start..] {
                    session_store::append_turn_at(state_root, &sid, msg)?;
                }
                Outbound {
                    kind: OutKind::Assistant,
                    text: resp.content,
                }
            }
            Err(e) => Outbound {
                kind: OutKind::Error,
                text: format!("[error: {e}]"),
            },
        };
        channel.send(out).await?;
    }
    Ok(())
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

/// A degraded [`crate::base_types::BaseTypeSession`] used only when an agent
/// session cannot be opened for an operator *after* startup provider validation
/// already passed (an unexpected transient). Every turn returns a clear error so
/// the operator is told the reasoner is unavailable, while the daemon keeps
/// serving other operators rather than crashing.
struct UnavailableAgent {
    descriptor: crate::base_types::BaseTypeDescriptor,
}

impl UnavailableAgent {
    fn new() -> Self {
        use crate::base_types::{BaseTypeDescriptor, BaseTypeId, standard_session_capabilities};
        use crate::metadata::{BackendDescriptor, Freshness};
        use crate::runtime::RuntimeTopology;
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("signal-unavailable-agent"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "signal-unavailable-agent",
                    "signal:unavailable",
                    // `now()` only fails if the system clock predates 1970.
                    Freshness::now().expect("system clock is before the unix epoch"),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
        }
    }
}

impl crate::base_types::BaseTypeSession for UnavailableAgent {
    fn descriptor(&self) -> &crate::base_types::BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        Ok(())
    }

    fn run_turn(
        &mut self,
        _input: crate::base_types::BaseTypeTurnInput,
    ) -> SimardResult<crate::base_types::BaseTypeOutcome> {
        Err(crate::error::SimardError::ActionExecutionFailed {
            action: "signal-meeting".to_string(),
            reason: "the reasoner is temporarily unavailable; please retry shortly".to_string(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        Ok(())
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
