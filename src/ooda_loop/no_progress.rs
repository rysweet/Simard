//! Fix 3 wiring: apply the no-progress breaker in the OODA curate phase.
//!
//! The pure policy lives in
//! [`crate::goal_curation::no_progress_breaker`]; this module is the thin,
//! side-effecting adapter that the OODA cycle calls each round. It turns the
//! breaker's [`NoProgressResolution`] into concrete board mutations and a
//! `gh`-filed tracking issue, mirroring the brain-*failure* safeguard in
//! [`crate::ooda_actions::advance_goal`] but for the *no-action* livelock.
//!
//! Kept in `ooda_loop` (the goal-selection / curate path) rather than the
//! reasoners, per the incident's coordination constraint (the
//! `ooda_brain`/reasoner/bridge files are owned by the naming-cleanup rename).
//!
//! See `docs/concepts/steerable-ooda-daemon.md` ("The no-progress breaker
//! (Fix 3)").

use std::collections::HashSet;

use crate::goal_curation::GoalProgress;
use crate::goal_curation::completion_gate::{CompletionEvidenceGate, EvidenceSource};
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, NoProgressResolution, verify_stuck_goal,
};
use crate::ooda_actions::outcome_made_no_progress;
use crate::ooda_loop::{ActionOutcome, OodaState};

/// Files a tracking issue for a goal the breaker escalated. Injected so tests
/// exercise the escalation path without shelling out to `gh`.
pub(crate) trait NoProgressIssueFiler {
    /// File (or attempt to file) a tracking issue. Failures must be logged, not
    /// propagated: the goal is already Blocked with the sentinel, and a missing
    /// issue must never abort the cycle.
    fn file_issue(&self, title: &str, body: &str);
}

/// Production filer: `gh issue create --label ooda-stuck`, mirroring the
/// brain-failure safeguard in `ooda_actions::advance_goal::spawn`.
pub(crate) struct GhIssueFiler;

impl NoProgressIssueFiler for GhIssueFiler {
    fn file_issue(&self, title: &str, body: &str) {
        match std::process::Command::new("gh")
            .args([
                "issue",
                "create",
                "--title",
                title,
                "--body",
                body,
                "--label",
                "ooda-stuck",
            ])
            .output()
        {
            Ok(out) if out.status.success() => {
                tracing::warn!(
                    target: "simard::ooda",
                    title = %title,
                    "no-progress breaker: tracking issue filed for stuck goal",
                );
            }
            Ok(out) => {
                tracing::error!(
                    target: "simard::ooda",
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "no-progress breaker: gh issue create failed (goal still Blocked)",
                );
            }
            Err(e) => {
                tracing::error!(
                    target: "simard::ooda",
                    error = %e,
                    "no-progress breaker: gh spawn failed (goal still Blocked)",
                );
            }
        }
    }
}

/// What the breaker did this cycle — returned for logging and asserted by tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct NoProgressBreakerReport {
    /// Goals set `Completed` for the evidence-aware archive to pick up.
    pub marked_done: Vec<String>,
    /// Goals removed from the board as obsolete.
    pub dropped: Vec<String>,
    /// Goals set `Blocked` with the sentinel and escalated to a tracking issue.
    pub escalated: Vec<String>,
    /// Standing/perpetual goals (issue #2589) that produced a no-action ("idle")
    /// cycle. Such a goal is inherently bursty — it ships a durable improvement
    /// periodically and idles between — so it is **exempt** from the breaker:
    /// its consecutive-no-action counter is reset and it stays active rather than
    /// being blocked/escalated. Recorded for the cycle log because an idling
    /// standing goal is normal, not a fault. Never contributes to
    /// [`fired`](Self::fired).
    pub perpetual_idled: Vec<String>,
}

impl NoProgressBreakerReport {
    /// True when the breaker fired for at least one goal this cycle. A
    /// standing/perpetual idle is deliberately **not** a firing — it is the
    /// exemption working as intended — so `perpetual_idled` is excluded.
    pub fn fired(&self) -> bool {
        !self.marked_done.is_empty() || !self.dropped.is_empty() || !self.escalated.is_empty()
    }

    /// Compact one-line summary for the cycle log.
    pub fn log_line(&self) -> String {
        format!(
            "done={} dropped={} escalated={} perpetual_idled={}",
            self.marked_done.len(),
            self.dropped.len(),
            self.escalated.len(),
            self.perpetual_idled.len(),
        )
    }
}

/// Apply the Fix-3 no-progress breaker to this cycle's `outcomes` using the
/// default [`NO_PROGRESS_BREAKER_THRESHOLD`].
pub(crate) fn apply_no_progress_breaker(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    filer: &dyn NoProgressIssueFiler,
) -> NoProgressBreakerReport {
    apply_no_progress_breaker_with_threshold(
        state,
        outcomes,
        evidence,
        filer,
        NO_PROGRESS_BREAKER_THRESHOLD,
    )
}

/// Threshold-parameterised core (tests inject a small threshold rather than
/// coupling to the shipped constant).
///
/// For each outcome carrying a goal id:
/// * a no-shippable-progress no-op ([`outcome_made_no_progress`]) bumps the
///   goal's consecutive-no-action counter; at `threshold` the done-gate runs
///   **once** and the goal is resolved via the ladder (mark done / drop /
///   escalate);
/// * any other successful goal outcome (engineer spawned, progress accepted)
///   resets the counter.
///
/// Marked-done goals are set [`GoalProgress::Completed`] so the subsequent
/// `archive_completed_evidence_aware` archives them with the same evidence;
/// dropped goals are removed from the board; escalated goals are set
/// [`GoalProgress::Blocked`] with the no-progress sentinel and a tracking issue
/// is filed. Stale counters are pruned to the live board.
pub(crate) fn apply_no_progress_breaker_with_threshold(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> NoProgressBreakerReport {
    let mut report = NoProgressBreakerReport::default();

    // Detach the tracker from `state` so the disposition closure can borrow the
    // board immutably while the tracker mutates. They are disjoint, but the
    // borrow checker cannot prove it through a method call across the closure.
    let mut tracker = std::mem::take(&mut state.no_progress_tracker);

    for outcome in outcomes {
        let Some(goal_id) = outcome.action.goal_id.as_deref() else {
            continue;
        };

        if !outcome_made_no_progress(outcome) {
            // Real progress (engineer spawned, or reviewer-accepted advance)
            // resets the consecutive-no-action count. Failures (success=false)
            // are owned by `goal_failure_counts` and left untouched here.
            if outcome.success {
                tracker.record_progress(goal_id);
            }
            continue;
        }

        // Standing/perpetual exemption (issue #2589). A standing/perpetual goal
        // is inherently bursty — it ships a durable improvement periodically and
        // idles between while there is nothing new to ship. An idle no-action
        // cycle is NORMAL, not the livelock the breaker guards against, so such a
        // goal must NEVER be hard-blocked / parked "needs human review": that is
        // the production defect this fixes. Reset its counter and keep it active
        // for the next cycle. Detection reuses the *same* `is_perpetual()` flag
        // (issue #2580) the non-completability path keys on — there is exactly one
        // notion of "standing/perpetual", never a second one.
        if state
            .active_goals
            .active
            .iter()
            .any(|g| g.id == goal_id && g.is_perpetual())
        {
            tracker.record_progress(goal_id);
            report.perpetual_idled.push(goal_id.to_string());
            tracing::info!(
                target: "simard::ooda",
                goal = %goal_id,
                "no-progress breaker: standing/perpetual goal idled this cycle \
                 (normal, not a fault) — counter reset, goal stays active",
            );
            continue;
        }

        // Compute the resolution in an inner scope so the immutable borrow of
        // the board (via `goal`) ends before the match mutates the board.
        let resolution = {
            let Some(goal) = state.active_goals.active.iter().find(|g| g.id == goal_id) else {
                // The goal already left the board this cycle (e.g. archived);
                // clear any stale counter and skip.
                tracker.record_progress(goal_id);
                continue;
            };
            let gate = CompletionEvidenceGate::new(evidence);
            tracker.record_and_resolve(goal_id, threshold, || verify_stuck_goal(goal, &gate))
        };

        match resolution {
            NoProgressResolution::Continue => {}
            NoProgressResolution::MarkDone => {
                if let Some(g) = state
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.status = GoalProgress::Completed;
                }
                report.marked_done.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    "no-progress breaker: evidence present — marking goal DONE for archival",
                );
            }
            NoProgressResolution::Drop { reason } => {
                state.active_goals.active.retain(|g| g.id != goal_id);
                state.active_goals.backlog.retain(|b| b.id != goal_id);
                report.dropped.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    reason = %reason,
                    "no-progress breaker: goal obsolete — DROPPING from the board",
                );
            }
            NoProgressResolution::Escalate {
                blocked_reason,
                issue_title,
                issue_body,
            } => {
                if let Some(g) = state
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.status = GoalProgress::Blocked(blocked_reason);
                }
                filer.file_issue(&issue_title, &issue_body);
                report.escalated.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    "no-progress breaker: unresolved after threshold — BLOCKED + tracking issue filed",
                );
            }
        }
    }

    // Prune counters for goals no longer on the active board.
    let live: HashSet<String> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.clone())
        .collect();
    tracker.retain_goals(&live);

    state.no_progress_tracker = tracker;
    report
}
