//! The escalate-vs-course-correct DECISION invariant for a blocked-goal
//! escalation (issue #4419).
//!
//! When the Overseer decides a goal is genuinely blocked, the agentic triage
//! recipe (`prompt_assets/simard/overseer/escalation_triage.md`) picks exactly
//! one course-correction and emits the triage schema. This module holds the
//! machine-checkable invariant that keeps that schema honest: the `escalate`
//! field is populated **iff** the decision is to ask the operator a question.
//! A self-correcting decision (rewrite an unmeasurable done-gate, or complete a
//! goal already delivered by a merged PR) must never escalate to a human; the
//! ask-operator decision must always carry a plain-English human-decision
//! reason. The Rust side owns only this thin structural check — the *choice* of
//! correction is the recipe's, not a bare integer threshold's.

use std::fmt;

/// The one course-correction the triage brain chose for a blocked goal. Mirrors
/// the `decision` field of the triage output schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CourseCorrection {
    /// Rewrite an unmeasurable done-gate into a machine-checkable finish line
    /// (self-correcting — never escalates).
    RewriteDoneGate,
    /// Mark complete a goal already delivered by a merged PR (self-correcting —
    /// never escalates).
    CompleteDeliveredGoal,
    /// Ask the operator exactly ONE plain-English question because a human
    /// decision is genuinely required (the only escalating decision).
    AskOperatorOneQuestion,
}

impl CourseCorrection {
    /// True only for [`CourseCorrection::AskOperatorOneQuestion`] — the sole
    /// decision that hands the call to a human. The self-correcting decisions
    /// unblock the goal agentically and never escalate.
    pub fn requires_human_escalation(&self) -> bool {
        matches!(self, CourseCorrection::AskOperatorOneQuestion)
    }
}

impl fmt::Display for CourseCorrection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::RewriteDoneGate => "rewrite-done-gate",
            Self::CompleteDeliveredGoal => "complete-delivered-goal",
            Self::AskOperatorOneQuestion => "ask-operator-one-question",
        };
        f.write_str(s)
    }
}

/// The triage escalation invariant was violated: either a self-correcting
/// decision carried a human-escalation reason, or an ask-operator decision was
/// missing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageInvariantError {
    /// Plain description of the violated invariant.
    pub reason: String,
}

impl fmt::Display for TriageInvariantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "triage escalation invariant violated: {}", self.reason)
    }
}

impl std::error::Error for TriageInvariantError {}

/// Assert the triage `escalate` field is consistent with the chosen decision:
///
/// - a self-correcting decision (`RewriteDoneGate` / `CompleteDeliveredGoal`)
///   must have `escalate == None`, and
/// - an `AskOperatorOneQuestion` decision must carry a non-empty plain-English
///   human-decision reason.
///
/// Returns `Err(TriageInvariantError)` on either violation so a malformed triage
/// result is rejected rather than silently escalating (or silently swallowing an
/// operator question).
pub fn validate_triage_escalation(
    decision: CourseCorrection,
    escalate: Option<&str>,
) -> Result<(), TriageInvariantError> {
    match (decision.requires_human_escalation(), escalate) {
        (true, Some(reason)) if !reason.trim().is_empty() => Ok(()),
        (true, _) => Err(TriageInvariantError {
            reason: format!(
                "decision {decision} requires a non-empty plain-English human-decision reason in `escalate`"
            ),
        }),
        (false, None) => Ok(()),
        (false, Some(_)) => Err(TriageInvariantError {
            reason: format!(
                "self-correcting decision {decision} must not escalate: `escalate` must be null"
            ),
        }),
    }
}
