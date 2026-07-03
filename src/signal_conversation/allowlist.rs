//! Guardrail (a): the sender allowlist — fail-closed authorization for inbound
//! Signal senders (issue #2527).
//!
//! Because the [`crate::conversation_channel::ConversationChannel`] contract
//! forbids yielding an unauthorized inbound to the driver, an unknown sender can
//! never reach a command handler: the allowlist decision is applied inside the
//! Signal channel's `recv` before anything is dispatched.

use super::config::SignalConfig;

/// The result of checking one inbound sender against the allowlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthDecision {
    /// Sender is on the allowlist; may issue commands (still subject to
    /// high-risk gating). Maps to `OperatorRef.authorized = true`.
    Authorized,
    /// Sender is not allowlisted but `read_only_unknown` is set: may receive
    /// read-only results only, and can NEVER trigger a mutation.
    ReadOnly,
    /// Sender is not allowlisted (fail-closed): dropped, no dispatch, no reply.
    Ignored,
}

/// A fail-closed E.164 sender allowlist.
pub struct Allowlist {
    numbers: Vec<String>,
    read_only_unknown: bool,
}

impl Allowlist {
    /// Build directly from a list of allowed E.164 numbers.
    pub fn new(numbers: Vec<String>, read_only_unknown: bool) -> Self {
        Self {
            numbers,
            read_only_unknown,
        }
    }

    /// Build from the `[signal]` configuration table.
    pub fn from_config(cfg: &SignalConfig) -> Self {
        Self::new(cfg.allowlist.clone(), cfg.read_only_unknown)
    }

    /// Classify an inbound sender. Fail-closed: only an exact E.164 match in the
    /// configured list is [`AuthDecision::Authorized`]. A non-match is
    /// [`AuthDecision::ReadOnly`] when `read_only_unknown` is set, else
    /// [`AuthDecision::Ignored`]. An empty allowlist authorizes nobody.
    pub fn authorize(&self, sender: &str) -> AuthDecision {
        if self.numbers.iter().any(|n| n == sender) {
            AuthDecision::Authorized
        } else if self.read_only_unknown {
            AuthDecision::ReadOnly
        } else {
            AuthDecision::Ignored
        }
    }
}
