//! Progress-evidence gatekeeper for goal-board progress updates.
//!
//! Implements the hallucinated-progress meta-bug fix (issue #1967): a
//! proposed progress *increase* on an active goal is accepted only when
//! verifiable evidence supports it. Without evidence, the gate
//! refuses to mutate the goal board and records a
//! `"brain hallucination detected: …"` cognitive-memory episode.
//!
//! Surface:
//! * [`ProgressEvidenceChecker`] — trait gating one decision.
//! * [`LlmReviewerProgressChecker`](super::progress_reviewer::LlmReviewerProgressChecker)
//!   — production checker (LLM-backed review).
//! * [`NoopProgressEvidenceChecker`] — kill-switch + test default.
//! * Façade [`crate::goal_curation::update_goal_progress_with_evidence`]
//!   wires this trait into the existing OODA loop.

use std::sync::OnceLock;

use chrono::{DateTime, Utc};

use super::types::ActiveGoal;

/// Outcome of a progress-evidence check.
///
/// Both variants are returned as `Ok(...)` by the gate façade — the
/// caller distinguishes `Accept` from `Reject` by pattern match, not by
/// `Result` discrimination. See `update_goal_progress_with_evidence`.
#[derive(Clone, Debug, PartialEq)]
pub enum EvidenceDecision {
    /// Evidence found — the caller may apply the progress update.
    Accept { reason: String },
    /// No evidence — the caller must keep the prior percent and emit
    /// a hallucination audit episode.
    Reject { reason: String },
}

/// Gate trait — decides whether a proposed progress increase is backed
/// by verifiable git artifacts.
///
/// `Send + Sync` so a single `Arc<dyn ProgressEvidenceChecker>` can be
/// installed on `OodaBridges` and shared across OODA actions.
pub trait ProgressEvidenceChecker: Send + Sync {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        since: DateTime<Utc>,
    ) -> EvidenceDecision;
}

// ===========================================================================
// NoopProgressEvidenceChecker — kill switch & test default
// ===========================================================================

/// Always returns `Accept { reason: "noop checker (no evidence enforced)" }`.
///
/// Used:
/// 1. By tests' default bridges constructors so existing tests don't
///    need to mock `git`/`gh`.
/// 2. As the operator escape hatch via `SIMARD_PROGRESS_EVIDENCE=off` at
///    daemon boot.
pub struct NoopProgressEvidenceChecker;

impl ProgressEvidenceChecker for NoopProgressEvidenceChecker {
    fn check(
        &self,
        _goal: &ActiveGoal,
        _old_percent: u32,
        _new_percent: u32,
        _since: DateTime<Utc>,
    ) -> EvidenceDecision {
        EvidenceDecision::Accept {
            reason: "noop checker (no evidence enforced)".to_string(),
        }
    }
}

// ===========================================================================
// Process-start fallback timestamp
// ===========================================================================

/// Returns the daemon's process-start timestamp (cached via `OnceLock`).
///
/// Last-resort `since` value when a goal has no
/// `last_progress_update_at` and no prior `"goal progress accepted: …"`
/// memory episode. Guarantees the gate is never a silent open door on a
/// fresh daemon process.
pub fn process_start() -> DateTime<Utc> {
    static START: OnceLock<DateTime<Utc>> = OnceLock::new();
    *START.get_or_init(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_checker_always_accepts() {
        let g = ActiveGoal {
            id: "x".into(),
            description: "y".into(),
            priority: 1,
            status: crate::goal_curation::GoalProgress::InProgress { percent: 10 },
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        };
        let dec = NoopProgressEvidenceChecker.check(&g, 10, 20, Utc::now());
        match dec {
            EvidenceDecision::Accept { reason } => assert!(reason.contains("noop")),
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn process_start_is_stable_across_calls() {
        let a = process_start();
        let b = process_start();
        assert_eq!(a, b);
    }
}
