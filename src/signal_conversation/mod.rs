//! Signal channel — the feature-gated Signal implementation of
//! [`crate::conversation_channel::ConversationChannel`] (issue #2527).
//!
//! `SignalConversation` lets an allowlisted operator command Simard and receive
//! her notifications over Signal, using the **same** meeting engine and
//! handoff/goal-carryover chain as the CLI and dashboard channels. Simard does
//! not embed the Signal protocol; she talks to a locally-run `signal-cli`
//! JSON-RPC daemon.
//!
//! This module is compiled only under the `signal` Cargo feature (default off),
//! so the default build has no Signal code and needs no signal-cli installed.
//!
//! # Naming
//!
//! Nothing here is named `adapter`/`Adapter`. `SignalConversation` is a first-class
//! conversation channel; it does not implement, extend, or route through the
//! pre-existing cognitive-memory `ServerTransport`.
//!
//! This step delivers the config type, the two remote-command-surface guardrails,
//! the signal-cli JSON-RPC transport, and the `SignalConversation` channel:
//! - `config` — the `[signal]` table and its env-first loader,
//! - `allowlist` — guardrail (a), the fail-closed sender allowlist,
//! - `gating` — guardrail (c), high-risk classification,
//! - `transport` — the newline-delimited JSON-RPC client (tokio TCP),
//! - `channel` — `SignalConversation`, the [`ConversationChannel`](crate::conversation_channel::ConversationChannel)
//!   over signal-cli, with allowlist + identity binding (guardrail (b)) + gating,
//!   notifications-out, and the `run` entrypoint.

pub mod allowlist;
pub mod channel;
pub mod config;
pub mod gating;
pub mod transport;

#[cfg(test)]
mod tests;

pub use allowlist::{Allowlist, AuthDecision};
pub use channel::{RuntimeCommandHandler, SignalCommandHandler, SignalConversation, run};
pub use config::SignalConfig;
pub use gating::{GateDecision, InboundCommand, RiskClass, classify, gate, parse_inbound};
pub use transport::{JsonRpcTransport, SignalTransport};
