//! TDD (Step 7, RED) — the OODA no-progress / re-orientation **livelock** fix
//! (issues #4497 / #4499 / #4504 / #4508 / #4509, plus the defective escalation
//! path #4474 / #4472).
//!
//! ROOT CAUSE these tests kill: the breaker dedups a still-blocked goal's
//! `ooda-stuck` tracking issue ONLY on in-memory `wip_refs`
//! ([`super::no_progress::is_breaker_tracking_ref`]). A re-orientation
//! ([`crate::goal_curation::ActiveGoal::roll_to_new_cycle`]) clears `wip_refs`,
//! so the very next threshold cycle sees "no tracked issue" and files a fresh
//! duplicate — the observed behaviour where ONE still-blocked goal spammed five
//! near-identical tracking issues (#4497/#4499/#4504/#4508/#4509) in ~6h while
//! never converging.
//!
//! TARGET contract (these tests reference API that does NOT exist yet, so the
//! crate test build FAILS to compile until the feature lands — that compile
//! failure IS the RED state of red→green→refactor):
//!
//!   1. `breaker_signature(goal_id)` — a deterministic, per-goal dedup key
//!      (reusing the proven `stewardship::dedup::failure_signature` convention)
//!      that is STABLE across re-orient and process restart, so it is the
//!      durable identity of "this goal's ooda-stuck tracking issue".
//!   2. `NoProgressIssueFiler::find_open_tracking_issue(signature)` — a
//!      read-only, fail-closed REMOTE search-before-create. The remote open-issue
//!      list (not the volatile in-memory `wip_refs`) is the dedup source of
//!      truth, so a re-orient/restart can never resurrect the duplicate-filing
//!      livelock.
//!   3. `NoProgressBreakerReport.halted` — after a goal's guided retry is spent
//!      and it is STILL blocked, the breaker escalates EXACTLY ONCE and then
//!      terminally halts re-orientation for that goal (recorded here), instead of
//!      re-firing/re-orienting every overseer tick.
//!
//! Everything is hermetic: an injected in-memory filer fake models the remote
//! issue store; no `gh`, no network.

use std::cell::RefCell;

use super::no_progress::{
    FiledIssue, NoProgressBreakerReport, NoProgressIssueFiler,
    apply_no_progress_breaker_with_threshold, breaker_signature,
};
use crate::error::SimardResult;
use crate::goal_curation::completion_gate::EvidenceSource;
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, is_no_progress_marker,
};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaState, PlannedAction};

// ─────────────────────────── fixtures ──────────────────────────────────────

/// Canned evidence source: no completion evidence at all, so a stalled goal is
/// genuinely stuck (the breaker cannot certify it done / obsolete).
struct NoEvidence;
impl EvidenceSource for NoEvidence {
    fn any_pr_merged(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
}

/// In-memory filer modelling the REMOTE `ooda-stuck` issue store. Every filed
/// issue is remembered under the `ooda-signature:<sig>` marker embedded in its
/// body, and [`find_open_tracking_issue`](NoProgressIssueFiler::find_open_tracking_issue)
/// searches that store — exactly the search-before-create the production
/// `GhIssueFiler` performs against `gh issue list`. This lets the test prove the
/// remote store (not in-memory `wip_refs`) is the dedup source of truth across a
/// re-orient / restart.
#[derive(Default)]
struct RemoteFiler {
    /// Every `file_issue` call's (title, body) — length asserts "filed once".
    files: RefCell<Vec<(String, String)>>,
    /// Every `find_open_tracking_issue` call's signature — proves search-first.
    searches: RefCell<Vec<String>>,
    /// The remote store: signature → the issue standing open for it.
    remote: RefCell<Vec<(String, FiledIssue)>>,
    /// When true every remote op returns `None` (models a `gh` outage) so the
    /// fail-closed path can be exercised.
    offline: bool,
}

impl RemoteFiler {
    fn offline() -> Self {
        Self {
            offline: true,
            ..Self::default()
        }
    }

    /// Seed the remote store with a pre-existing tracking issue for `signature`,
    /// modelling a process that ALREADY filed the issue in a prior run (restart
    /// dedup).
    fn with_existing(signature: &str, number: &str) -> Self {
        let f = Self::default();
        f.remote.borrow_mut().push((
            signature.to_string(),
            FiledIssue {
                number: number.to_string(),
                url: None,
            },
        ));
        f
    }
}

impl NoProgressIssueFiler for RemoteFiler {
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue> {
        if self.offline {
            return None;
        }
        self.files
            .borrow_mut()
            .push((title.to_string(), body.to_string()));
        // The signature marker the breaker MUST embed so a later search can find
        // this exact issue. The marker is `ooda-signature:<sig>` on its own body
        // token — the same body-marker/search convention `stewardship` uses.
        let signature = body
            .split_whitespace()
            .find_map(|t| t.strip_prefix("ooda-signature:"))
            .expect("breaker must embed an `ooda-signature:<sig>` body marker so dedup can match")
            .to_string();
        let number = format!("{}", 4500 + self.files.borrow().len());
        let filed = FiledIssue {
            number: number.clone(),
            url: None,
        };
        self.remote.borrow_mut().push((signature, filed.clone()));
        Some(filed)
    }

    fn find_open_tracking_issue(&self, signature: &str) -> Option<FiledIssue> {
        self.searches.borrow_mut().push(signature.to_string());
        if self.offline {
            return None;
        }
        self.remote
            .borrow()
            .iter()
            .find(|(sig, _)| sig == signature)
            .map(|(_, filed)| filed.clone())
    }
}

/// A stuck, self-affecting (repo=None) goal parked at 0% with one open,
/// never-merging PR so it is genuinely stuck (no derivable completion signal).
fn stuck_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "converge the red-canary deploy gate", 1);
    g.status = GoalProgress::NotStarted;
    g.wip_refs = vec![WipRef {
        kind: "pr".to_string(),
        ref_id: "7".to_string(),
        label: "PR #7".to_string(),
        url: None,
    }];
    g
}

fn state_with(goal: ActiveGoal) -> OodaState {
    let mut board = GoalBoard::new();
    board.active.push(goal);
    OodaState::new(board)
}

fn no_action_outcome(goal_id: &str) -> ActionOutcome {
    ActionOutcome {
        action: PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some(goal_id.to_string()),
            description: "advance".to_string(),
        },
        success: true,
        detail: format!("no-action: I'll verify concretely next cycle (goal '{goal_id}')"),
    }
}

/// Drive `goal_id` through exactly `threshold` no-action cycles so the breaker
/// fires once, returning the firing cycle's report.
fn drive_to_escalation(
    state: &mut OodaState,
    goal_id: &str,
    evidence: &dyn EvidenceSource,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> NoProgressBreakerReport {
    let mut last = NoProgressBreakerReport::default();
    for _ in 0..threshold {
        last = apply_no_progress_breaker_with_threshold(
            state,
            &[no_action_outcome(goal_id)],
            evidence,
            filer,
            threshold,
        );
    }
    last
}

/// Simulate the per-goal re-orientation the agentic reasoner performs
/// (`ActiveGoal::roll_to_new_cycle`): drop the goal's tracked refs so the OLD
/// in-memory-only dedup is defeated. The remote store is untouched — that is the
/// whole point of the fix.
fn reorient(state: &mut OodaState, goal_id: &str) {
    if let Some(g) = state
        .active_goals
        .active
        .iter_mut()
        .find(|g| g.id == goal_id)
    {
        g.wip_refs.clear();
        g.status = GoalProgress::NotStarted;
    }
}

// ─────────────────────────── tests ─────────────────────────────────────────

#[test]
fn breaker_signature_is_deterministic_per_goal() {
    // Same goal id ⇒ identical signature across independent calls (so a re-orient
    // or a restart recomputes the SAME dedup key and matches the standing issue).
    let a = breaker_signature("simard-identity-4d27c91a");
    let b = breaker_signature("simard-identity-4d27c91a");
    assert_eq!(a, b, "signature must be stable for a fixed goal id");
    assert!(!a.is_empty(), "signature must be non-empty");
}

#[test]
fn breaker_signature_is_distinct_across_goals() {
    // Two different goals must never collide onto one tracking issue.
    let a = breaker_signature("goal-4d27c91a");
    let b = breaker_signature("goal-7f5afcca");
    assert_ne!(a, b, "distinct goals must get distinct signatures");
}

#[test]
fn a_still_blocked_goal_files_at_most_one_issue_across_reorient() {
    // THE #4497 livelock: escalate once → re-orient (clears wip_refs) → escalate
    // again. The second escalation must find the standing remote issue via the
    // signature and NOT file a duplicate, so `file_issue` is called exactly ONCE
    // total even though the goal re-stalled after losing its in-memory link.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let filer = RemoteFiler::default();
    let evidence = NoEvidence;
    let mut state = state_with(stuck_goal("simard-identity-4d27c91a"));

    let first = drive_to_escalation(
        &mut state,
        "simard-identity-4d27c91a",
        &evidence,
        &filer,
        threshold,
    );
    assert_eq!(
        first.escalated,
        vec!["simard-identity-4d27c91a".to_string()],
        "the stuck goal escalates on the first threshold firing"
    );
    assert_eq!(
        filer.files.borrow().len(),
        1,
        "the first escalation files exactly one ooda-stuck issue"
    );
    match &state.active_goals.active[0].status {
        GoalProgress::Blocked(reason) => assert!(
            is_no_progress_marker(reason),
            "escalated goal must carry the no-progress sentinel: {reason}"
        ),
        other => panic!("expected Blocked, got {other:?}"),
    }

    // Re-orient wipes the in-memory tracking ref (the OLD dedup's only source).
    reorient(&mut state, "simard-identity-4d27c91a");

    // Re-stall to the threshold again.
    let _ = drive_to_escalation(
        &mut state,
        "simard-identity-4d27c91a",
        &evidence,
        &filer,
        threshold,
    );

    assert_eq!(
        filer.files.borrow().len(),
        1,
        "no duplicate issue: remote search-before-create dedups across re-orient (#4497)"
    );
    assert!(
        !filer.searches.borrow().is_empty(),
        "the breaker must consult find_open_tracking_issue BEFORE filing"
    );
    let expected_sig = breaker_signature("simard-identity-4d27c91a");
    assert!(
        filer.searches.borrow().contains(&expected_sig),
        "the remote search must key on the goal's stable breaker_signature"
    );
}

#[test]
fn dedup_survives_process_restart() {
    // A restart drops the in-memory tracker/state entirely. A fresh state whose
    // goal re-stalls must still match the tracking issue a PRIOR process filed
    // (seeded in the remote), proving the remote store — not in-process memory —
    // is the dedup source of truth.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let signature = breaker_signature("simard-identity-7f5afcca");
    let filer = RemoteFiler::with_existing(&signature, "4497");
    let evidence = NoEvidence;
    let mut fresh_state = state_with(stuck_goal("simard-identity-7f5afcca"));

    let _ = drive_to_escalation(
        &mut fresh_state,
        "simard-identity-7f5afcca",
        &evidence,
        &filer,
        threshold,
    );

    assert!(
        filer.files.borrow().is_empty(),
        "a goal whose issue was filed by a prior process must NOT re-file after restart"
    );
    assert!(
        filer.searches.borrow().contains(&signature),
        "restart dedup must go through the remote signature search"
    );
}

#[test]
fn escalation_halts_reorientation_after_it_fires_once() {
    // After the breaker escalates a still-blocked goal it must record the goal as
    // terminally HALTED (escalate once, stop re-orienting) rather than re-firing
    // every tick. Subsequent no-action ticks are no-ops: no new escalation, no
    // second issue.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let filer = RemoteFiler::default();
    let evidence = NoEvidence;
    let mut state = state_with(stuck_goal("simard-identity-4d27c91a"));

    let report = drive_to_escalation(
        &mut state,
        "simard-identity-4d27c91a",
        &evidence,
        &filer,
        threshold,
    );
    assert!(
        report
            .halted
            .contains(&"simard-identity-4d27c91a".to_string()),
        "a goal escalated after its guided retry must be marked halted (terminal), not re-orient forever"
    );

    // Keep ticking: a halted goal never produces a second escalation or issue.
    for _ in 0..(threshold * 2) {
        let r = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome("simard-identity-4d27c91a")],
            &evidence,
            &filer,
            threshold,
        );
        assert!(
            r.escalated.is_empty(),
            "a halted goal must not re-escalate on later ticks"
        );
    }
    assert_eq!(
        filer.files.borrow().len(),
        1,
        "a halted goal files exactly one tracking issue, ever"
    );
}

#[test]
fn filer_outage_keeps_goal_blocked_and_never_aborts_the_cycle() {
    // Repairs #4472/#4474: when the remote is unreachable (search AND create
    // return None) the breaker must fail CLOSED — the goal stays Blocked with the
    // sentinel and the cycle returns normally (no panic, no propagated error),
    // rather than the escalation path blowing up or spamming.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let filer = RemoteFiler::offline();
    let evidence = NoEvidence;
    let mut state = state_with(stuck_goal("simard-identity-4d27c91a"));

    let report = drive_to_escalation(
        &mut state,
        "simard-identity-4d27c91a",
        &evidence,
        &filer,
        threshold,
    );

    assert_eq!(
        report.escalated,
        vec!["simard-identity-4d27c91a".to_string()],
        "the goal is still escalated (blocked) even when the filer is offline"
    );
    assert!(
        filer.files.borrow().is_empty(),
        "an offline filer files nothing (it returned None), yet must not abort the cycle"
    );
    match &state.active_goals.active[0].status {
        GoalProgress::Blocked(reason) => assert!(
            is_no_progress_marker(reason),
            "goal stays Blocked with the no-progress sentinel despite the filer outage: {reason}"
        ),
        other => panic!("expected Blocked, got {other:?}"),
    }
}
