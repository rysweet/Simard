//! Guardrail (c): high-risk gating (issue #2527).
//!
//! Every inbound command is classified. Low-risk commands run immediately for an
//! allowlisted sender; high-risk / mutating actions are NEVER auto-executed from
//! a text — they create a pending operator decision and Simard asks for explicit
//! sign-off, routing the eventual mutation through the EXISTING operator gate
//! from the operational autonomy model (`git_guardrails`, `identity_auth`,
//! `stewardship::merge_authority`). This module owns only the classification; the
//! enforcement it routes to is unchanged.

/// Coarse risk class of an inbound command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskClass {
    /// Runs immediately for an allowlisted sender (read-only or a benign,
    /// sign-off-recording action).
    LowRisk,
    /// Mutating / high-risk: never auto-executed; requires explicit sign-off.
    HighRisk,
}

/// A lightweight inbound command parsed from Signal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboundCommand {
    /// Report daemon health, active goals, in-flight engineers. Low-risk.
    Status,
    /// Pause autonomous dispatch. Low-risk.
    Pause,
    /// Record operator sign-off for a pending high-risk request. Low-risk: it
    /// only records the sign-off; it does not itself perform the mutation.
    Approve,
    /// Request a deploy. High-risk → pending sign-off.
    Deploy,
    /// Merge PR #NNNN via the gated merge authority. High-risk → pending sign-off.
    Merge(u64),
    /// Any other text — an ordinary meeting turn, answered conversationally.
    Conversation(String),
}

/// What the channel should do with a (already-authorized) inbound command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateDecision {
    /// Low-risk: execute immediately.
    AutoExecute,
    /// High-risk: create a pending operator decision and ask for sign-off. The
    /// mutation is NEVER performed directly from the text.
    PendingSignOff,
}

/// Parse one line of inbound Signal text into an [`InboundCommand`].
///
/// The lightweight command vocabulary (`status`, `pause`, `approve`, `deploy`,
/// `merge #NNNN`) is matched case-insensitively; anything else is an ordinary
/// meeting turn, carried verbatim (original case/whitespace) as
/// [`InboundCommand::Conversation`].
pub fn parse_inbound(text: &str) -> InboundCommand {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "status" => InboundCommand::Status,
        "pause" => InboundCommand::Pause,
        "approve" => InboundCommand::Approve,
        "deploy" => InboundCommand::Deploy,
        _ => {
            if let Some(rest) = lower.strip_prefix("merge") {
                let rest = rest.trim();
                let digits = rest.strip_prefix('#').unwrap_or(rest).trim();
                // An empty or non-numeric remainder parses to `Err`, so this
                // naturally falls through to `Conversation` for bare `merge`.
                if let Ok(n) = digits.parse::<u64>() {
                    return InboundCommand::Merge(n);
                }
            }
            InboundCommand::Conversation(trimmed.to_string())
        }
    }
}

/// Classify an inbound command's risk. `status`/`pause`/`approve` and ordinary
/// conversation turns are low-risk; `deploy`/`merge` are high-risk mutating
/// actions.
pub fn classify(cmd: &InboundCommand) -> RiskClass {
    match cmd {
        InboundCommand::Status
        | InboundCommand::Pause
        | InboundCommand::Approve
        | InboundCommand::Conversation(_) => RiskClass::LowRisk,
        InboundCommand::Deploy | InboundCommand::Merge(_) => RiskClass::HighRisk,
    }
}

/// Decide whether an authorized inbound command may auto-execute or must first
/// obtain explicit operator sign-off. High-risk commands MUST return
/// [`GateDecision::PendingSignOff`] — a mutating action is never performed
/// directly from a text message.
pub fn gate(cmd: &InboundCommand) -> GateDecision {
    match classify(cmd) {
        RiskClass::LowRisk => GateDecision::AutoExecute,
        RiskClass::HighRisk => GateDecision::PendingSignOff,
    }
}
