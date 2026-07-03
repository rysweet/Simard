//! One clearly-named operator↔Simard conversation abstraction (issue #2527).
//!
//! A **meeting is a conversation.** Every operator↔Simard meeting — in a
//! terminal, in the dashboard chat pane, or over Signal — is the same
//! bidirectional session: the operator sends an inbound line, Simard delivers
//! outbound replies and notices, the session opens and closes, and the
//! structured-capture commands (`/goal`, `/decision`, `/action`, `/question`, …)
//! feed the meeting handoff and goal-carryover chain.
//!
//! [`ConversationChannel`] is the single abstraction for that session. The
//! record/status dispatch that was duplicated across the CLI REPL and the
//! dashboard chat loop is extracted once into [`dispatch::apply_record`], which
//! **both** loops now call for the `/goal`, `/decision`, `/action`, `/question`,
//! `/theme`, `/owner`, `/risk`, and `/disagree` captures. [`driver::run_conversation`]
//! is the unified driver over the trait; it is realized by the new
//! [`crate::signal_conversation::SignalConversation`] channel and by
//! [`MockConversationChannel`], and is available for any future channel.
//!
//! # Naming
//!
//! Nothing here is named `adapter`/`Adapter`. This is a first-class chat
//! abstraction and is unrelated to the pre-existing cognitive-memory
//! `ServerTransport`. The Signal implementation ([`crate::signal_conversation`])
//! does **not** route through that trait.

use std::future::Future;

use crate::error::SimardResult;
use crate::meeting_backend::MeetingBackend;

pub mod dispatch;
pub mod driver;
pub mod mock;

#[cfg(test)]
mod tests;

pub use dispatch::{Recorded, apply_record};
pub use driver::run_conversation;
pub use mock::MockConversationChannel;

/// Who sent an inbound message. Used for the sender allowlist + identity binding.
#[derive(Clone, Debug)]
pub struct OperatorRef {
    /// Channel-native id: terminal user, dashboard session id, or Signal E.164.
    pub id: String,
    /// True once this ref has cleared the channel's allowlist / identity /
    /// dashboard-auth check. The driver assumes every inbound it receives is
    /// authorized; a channel that gates senders filters before returning.
    pub authorized: bool,
}

/// One operator-originated message line, already trimmed of surrounding
/// whitespace by the channel.
#[derive(Clone, Debug)]
pub struct Inbound {
    pub from: OperatorRef,
    pub text: String,
}

/// A Simard-originated message to deliver on the channel. Render-agnostic: the
/// channel's [`ConversationChannel::send`] maps [`OutKind`] to its own
/// presentation (ANSI color, JSON role, or a Signal send).
#[derive(Clone, Debug)]
pub struct Outbound {
    pub kind: OutKind,
    pub text: String,
}

/// The kind of a Simard-originated message, so each channel can render it in its
/// own way without the shared driver knowing anything about presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutKind {
    /// Simard's conversational LLM reply.
    Assistant,
    /// A structured-capture acknowledgement ("Decision recorded: …", etc.).
    Recorded,
    /// Read-only session output (`/status`, `/state`, `/recap`, `/preview`, `/help`).
    Status,
    /// A notification-out: PR merge-ready, stall/problem, high-risk sign-off request.
    Notice,
    /// An error message.
    Error,
}

/// A bidirectional operator↔Simard conversation channel. The meeting IS the
/// conversation; each implementation is one channel over the shared
/// [`MeetingBackend`]. Native async via return-position `impl Future` (no
/// `async-trait` dependency). Used with static dispatch — never as `dyn`.
pub trait ConversationChannel {
    /// Stable id for logs/metrics: `"cli"`, `"dashboard"`, `"signal"`, `"mock"`.
    fn name(&self) -> &'static str;

    /// Await the next authorized operator line.
    ///
    /// `Ok(None)` ends the session (EOF, socket closed, or operator quit).
    /// Implementations MUST NOT yield an inbound whose `from.authorized` is
    /// `false`; unauthorized input is dropped inside the channel before it
    /// reaches the driver.
    fn recv(&mut self) -> impl Future<Output = SimardResult<Option<Inbound>>> + Send;

    /// Deliver one Simard message OUT on this channel. `send` is total: a
    /// channel renders every [`OutKind`] and must not drop any.
    fn send(&mut self, out: Outbound) -> impl Future<Output = SimardResult<()>> + Send;

    /// Per-channel hook fired after a structured capture is applied (i.e. after an
    /// [`OutKind::Recorded`] outbound). The default is a no-op; a channel with
    /// post-record effects overrides it (for example the mock counts its calls).
    /// The CLI REPL keeps its `checkpoint_wip` + live capture tally inline in its
    /// own loop, so its behavior is preserved exactly; the dashboard and Signal
    /// channels keep the default no-op.
    fn on_recorded(
        &mut self,
        _backend: &MeetingBackend,
    ) -> impl Future<Output = SimardResult<()>> + Send {
        async { Ok(()) }
    }
}
