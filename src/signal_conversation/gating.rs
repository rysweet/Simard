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

// ───────────── operator-notification anti-self-ingest marker (#2631) ─────────
//
// Every outbound Overseer→operator Signal notification is wrapped in a reserved
// sentinel so the INBOUND processor can DETERMINISTICALLY recognize and SKIP
// Simard's own notifications synced back to a linked device — independent of the
// fragile exact-body/time-window echo suppression (`matches_recent_outbound`).
// See docs/reference/overseer-operator-notifications.md (Part A).

/// Reserved sentinel prefixing every Overseer→operator Signal notification. Its
/// bell glyph + `▶` + the literal `OPERATOR:` never occur in a real operator
/// command, so detection is a substring test that leaves normal messages
/// untouched. Operators MUST NOT send any message *containing* this sentinel.
pub const OPERATOR_NOTIFY_MARKER: &str = "🔔 SIMARD▶OPERATOR:";

/// Human-readable footer appended to a wrapped notification so the message reads
/// pleasantly on the operator's phone. Display only — it is NEVER used for
/// detection.
pub const OPERATOR_NOTIFY_FOOTER: &str = "\n\n— Simard automated notice · do not reply";

/// Wrap an operator-notification `body` in the reserved marker (+ footer) so the
/// inbound processor can deterministically skip it. The result STARTS WITH
/// [`OPERATOR_NOTIFY_MARKER`] and ENDS WITH [`OPERATOR_NOTIFY_FOOTER`].
pub fn wrap_operator_notification(body: &str) -> String {
    format!("{OPERATOR_NOTIFY_MARKER} {body}{OPERATOR_NOTIFY_FOOTER}")
}

/// True iff `text` carries the reserved [`OPERATOR_NOTIFY_MARKER`] anywhere in
/// the message — a SUBSTRING test, NOT `starts_with`, so a synced echo with an
/// added transcript prefix (e.g. `"You sent: …"`) or a quoted reply is still
/// recognized. The marker only causes an inbound message to be IGNORED; it never
/// authorizes anything.
pub fn is_operator_notification(text: &str) -> bool {
    text.contains(OPERATOR_NOTIFY_MARKER)
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    #[test]
    fn wrap_starts_with_marker_and_ends_with_footer() {
        let body = "The Overseer autonomously performed a goal-blocked in rysweet/Simard.\n";
        let wrapped = wrap_operator_notification(body);
        assert!(
            wrapped.starts_with(OPERATOR_NOTIFY_MARKER),
            "wrapped notice must lead with the marker: {wrapped:?}"
        );
        assert!(
            wrapped.contains(body),
            "the operator-readable body must be preserved verbatim: {wrapped:?}"
        );
        assert!(
            wrapped.ends_with(OPERATOR_NOTIFY_FOOTER),
            "the human footer must trail the notice: {wrapped:?}"
        );
    }

    #[test]
    fn is_operator_notification_detects_a_wrapped_body() {
        let wrapped = wrap_operator_notification("goal g-1 needs human review");
        assert!(
            is_operator_notification(&wrapped),
            "a freshly wrapped notification must be recognized"
        );
    }

    #[test]
    fn is_operator_notification_is_substring_not_prefix() {
        // A synced-back echo can arrive with a leading transcript prefix or as a
        // quote; substring detection must still recognize the marker so the gate
        // does not depend on the exact-body echo window.
        let with_prefix = format!("You sent: {OPERATOR_NOTIFY_MARKER} merge #42 …");
        assert!(
            is_operator_notification(&with_prefix),
            "must detect the marker mid-string: {with_prefix:?}"
        );
        let quoted = format!("> {OPERATOR_NOTIFY_MARKER} deploy");
        assert!(
            is_operator_notification(&quoted),
            "must detect a quoted marker"
        );
    }

    #[test]
    fn normal_operator_messages_are_never_flagged() {
        // The vocabulary of real operator commands and ordinary turns must never
        // be mistaken for a self-notification (no false positives).
        for msg in [
            "status",
            "pause",
            "approve",
            "deploy",
            "merge #42",
            "let's plan the release",
            "SIMARD is great",
            "",
        ] {
            assert!(
                !is_operator_notification(msg),
                "false positive on a normal message: {msg:?}"
            );
        }
    }

    #[test]
    fn wrap_then_detect_round_trips() {
        let wrapped =
            wrap_operator_notification("Goal `g-4821` is blocked and needs human review.");
        assert!(
            is_operator_notification(&wrapped),
            "wrap → detect must round-trip so the inbound gate always drops our own notices"
        );
    }
}
