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
///
/// The vocabulary is short and ASCII, so recognition uses case-insensitive ASCII
/// comparisons rather than allocating a fully lowercased copy of the message.
/// This matters on the per-message inbound path: an ordinary conversation turn —
/// which can be a long paste — is recognized as free text without ever being
/// case-folded (the `eq_ignore_ascii_case` checks short-circuit on length).
pub fn parse_inbound(text: &str) -> InboundCommand {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("status") {
        InboundCommand::Status
    } else if trimmed.eq_ignore_ascii_case("pause") {
        InboundCommand::Pause
    } else if trimmed.eq_ignore_ascii_case("approve") {
        InboundCommand::Approve
    } else if trimmed.eq_ignore_ascii_case("deploy") {
        InboundCommand::Deploy
    } else if let Some(n) = parse_merge(trimmed) {
        InboundCommand::Merge(n)
    } else {
        InboundCommand::Conversation(trimmed.to_string())
    }
}

/// Parse a `merge #NNNN` command (case-insensitive `merge` prefix, optional `#`),
/// returning the PR number. A bare `merge`, a non-numeric remainder, or any text
/// that merely starts with "merge" yields `None`, so it falls through to
/// [`InboundCommand::Conversation`] — matching the original lowercase behavior.
fn parse_merge(trimmed: &str) -> Option<u64> {
    // ASCII-case-insensitively strip the 5-byte "merge" prefix. `get(..5)` is
    // `None` if `trimmed` is shorter than the prefix or byte 5 is not a char
    // boundary; either way this is not a `merge` command.
    let rest = trimmed.get(..5)?;
    if !rest.eq_ignore_ascii_case("merge") {
        return None;
    }
    let rest = trimmed[5..].trim();
    let digits = rest.strip_prefix('#').unwrap_or(rest).trim();
    digits.parse::<u64>().ok()
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
