//! Operator-facing message safety: a fail-closed denylist that keeps raw
//! machine markers out of every plain-English Signal update (issue #4419).
//!
//! The escalation-triage brain restates a blocked goal in plain English and
//! sends the operator one short update per reasoning step. Those updates must
//! NEVER carry an internal diagnostic token — `OODA-SAFEGUARD`,
//! `UNCLEAR-CRITERIA`, `GENUINELY-STUCK`, `why=…`, `evidence=[…]`, the `🔒`
//! lock, or the `health-review:blocked-goal` reason marker — nor be multi-line
//! (Signal updates are single-line). This module is the presentation-only guard
//! that enforces that: it is a check, never a state authority, and it fails
//! CLOSED — an ambiguous or marker-bearing message is rejected before it can be
//! sent, rather than silently scrubbed.

use std::fmt;

/// Every raw machine marker an operator must never see. A message containing any
/// of these (case-insensitively) is rejected before it can be sent. Sourced from
/// the triage translation table in
/// `prompt_assets/simard/overseer/escalation_triage.md`.
pub const OPERATOR_FORBIDDEN_MARKERS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "why=",
    "evidence=[",
    "\u{1F512}", // the 🔒 lock token
    "health-review:blocked-goal",
];

/// Why an operator-facing message was rejected. Carries the specific reason so
/// the caller can log the *fact* of a rejected leak (never the leaked payload)
/// under structured `tracing`, and fix the message rather than send it raw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorMessageRejected {
    /// Plain description of the violated rule (e.g. which marker leaked, or that
    /// the message was multi-line / carried control characters).
    pub reason: String,
}

impl fmt::Display for OperatorMessageRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operator message rejected: {}", self.reason)
    }
}

impl std::error::Error for OperatorMessageRejected {}

/// Assert a message is safe to show an operator: single-line, control-char-free,
/// plain English with no raw machine marker. Fails CLOSED — returns `Err` (never
/// a scrubbed string) so the caller must fix the message before sending it.
pub fn ensure_operator_safe(message: &str) -> Result<(), OperatorMessageRejected> {
    // Signal operator updates are single-line: reject any newline or carriage
    // return outright so a marker can't be smuggled past a per-line check.
    if message.contains('\n') || message.contains('\r') {
        return Err(OperatorMessageRejected {
            reason: "message must be a single line (no newline / carriage return)".to_string(),
        });
    }
    // No other control characters either (log-injection / terminal-escape guard).
    if message.chars().any(char::is_control) {
        return Err(OperatorMessageRejected {
            reason: "message must not contain control characters".to_string(),
        });
    }
    // Fail-closed marker denylist, matched case-insensitively so a lower-cased
    // leak is caught just as a raw upper-cased token would be.
    let haystack = message.to_ascii_lowercase();
    for marker in OPERATOR_FORBIDDEN_MARKERS {
        if haystack.contains(&marker.to_ascii_lowercase()) {
            return Err(OperatorMessageRejected {
                reason: format!(
                    "message carries an internal marker that must be translated: {marker}"
                ),
            });
        }
    }
    Ok(())
}
