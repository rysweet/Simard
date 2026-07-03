//! Fix 3 wiring integration tests: the OODA-curate no-progress breaker
//! (`super::no_progress`).
//!
//! The pure ladder is unit-tested in
//! `crate::goal_curation::tests_no_progress_breaker`; these tests exercise the
//! *wiring* — that a stuck goal driven through
//! [`apply_no_progress_breaker_with_threshold`] over N cycles actually mutates
//! the board (mark done / drop / block) and files exactly one tracking issue,
//! and that real progress resets the counter so the breaker never fires on a
//! goal that is being worked.

use std::cell::RefCell;

use super::no_progress::{NoProgressIssueFiler, apply_no_progress_breaker_with_threshold};
use crate::error::SimardResult;
use crate::goal_curation::completion_gate::EvidenceSource;
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, is_no_progress_marker,
};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaState, PlannedAction};

// --- fixtures ---------------------------------------------------------------

/// Canned evidence source: every query returns a fixed boolean.
struct FakeEvidence {
    pr_merged: bool,
    issue_closed: bool,
    deployed: bool,
}

impl EvidenceSource for FakeEvidence {
    fn any_pr_merged(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.pr_merged)
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.issue_closed)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.deployed)
    }
}

/// Records escalation issue filings so tests assert the count without `gh`.
#[derive(Default)]
struct RecordingFiler {
    calls: RefCell<Vec<(String, String)>>,
}

impl NoProgressIssueFiler for RecordingFiler {
    fn file_issue(&self, title: &str, body: &str) {
        self.calls
            .borrow_mut()
            .push((title.to_string(), body.to_string()));
    }
}

fn issue_ref(num: &str, label: &str) -> WipRef {
    WipRef {
        kind: "issue".to_string(),
        ref_id: num.to_string(),
        label: label.to_string(),
        url: None,
    }
}

fn pr_ref(num: &str) -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: num.to_string(),
        label: format!("PR #{num}"),
        url: None,
    }
}

/// A stuck, self-affecting (repo=None) goal at 0%.
fn stuck_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "harden the supply chain", 1);
    g.status = GoalProgress::NotStarted;
    g
}

fn state_with(goal: ActiveGoal) -> OodaState {
    let mut board = GoalBoard::new();
    board.active.push(goal);
    OodaState::new(board)
}

/// A no-shippable-progress ("NO ACTION") outcome for `goal_id`, in the exact
/// shape `assess_only_outcome` authors and `outcome_made_no_progress` keys on.
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

/// A concrete-progress outcome (engineer spawned) for `goal_id`.
fn progress_outcome(goal_id: &str) -> ActionOutcome {
    ActionOutcome {
        action: PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some(goal_id.to_string()),
            description: "advance".to_string(),
        },
        success: true,
        detail: format!("spawn_engineer (from prose) for goal '{goal_id}': do the work"),
    }
}

// --- tests ------------------------------------------------------------------

#[test]
fn breaker_threshold_is_small() {
    assert!(
        (2..=3).contains(&NO_PROGRESS_BREAKER_THRESHOLD),
        "breaker must be a small 2-3, got {NO_PROGRESS_BREAKER_THRESHOLD}"
    );
}

#[test]
fn n_consecutive_no_action_cycles_escalate_and_block_the_goal() {
    // The exact livelock: a goal that yields NO ACTION every cycle, with no
    // completion evidence and no obsolescence signal, must be BLOCKED with the
    // sentinel and escalated to exactly one tracking issue after the threshold.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("ladybug-supply-chain");
    goal.wip_refs = vec![pr_ref("7")]; // an open, unmerged PR
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: true,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    // Below threshold: nothing fires, the goal stays not-started.
    for cycle in 1..threshold {
        let report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome("ladybug-supply-chain")],
            &evidence,
            &filer,
            threshold,
        );
        assert!(!report.fired(), "cycle {cycle} must not fire");
        assert!(filer.calls.borrow().is_empty());
        assert!(matches!(
            state.active_goals.active[0].status,
            GoalProgress::NotStarted
        ));
    }

    // Threshold cycle: escalate.
    let report = apply_no_progress_breaker_with_threshold(
        &mut state,
        &[no_action_outcome("ladybug-supply-chain")],
        &evidence,
        &filer,
        threshold,
    );
    assert_eq!(report.escalated, vec!["ladybug-supply-chain".to_string()]);
    assert_eq!(filer.calls.borrow().len(), 1, "exactly one issue filed");
    match &state.active_goals.active[0].status {
        GoalProgress::Blocked(reason) => assert!(
            is_no_progress_marker(reason),
            "blocked reason must carry the no-progress sentinel: {reason}"
        ),
        other => panic!("expected Blocked, got {other:?}"),
    }

    // The counter cleared: another no-action cycle does NOT immediately re-fire
    // (no (N+1)th escalation / duplicate issue this cycle).
    let report = apply_no_progress_breaker_with_threshold(
        &mut state,
        &[no_action_outcome("ladybug-supply-chain")],
        &evidence,
        &filer,
        threshold,
    );
    assert!(!report.fired(), "counter must reset after firing");
    assert_eq!(filer.calls.borrow().len(), 1, "no duplicate issue");
}

#[test]
fn merged_pr_goal_is_marked_done_for_archival() {
    // A goal whose PR merged, issue closed, and change deployed → the done-gate
    // certifies it Complete → the breaker sets it Completed so the evidence-aware
    // archive removes it.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("ladybug-rust");
    goal.wip_refs = vec![pr_ref("1")];
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: true,
        issue_closed: true,
        deployed: true,
    };
    let filer = RecordingFiler::default();

    let mut report = Default::default();
    for _ in 0..threshold {
        report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome("ladybug-rust")],
            &evidence,
            &filer,
            threshold,
        );
    }
    assert_eq!(report.marked_done, vec!["ladybug-rust".to_string()]);
    assert!(matches!(
        state.active_goals.active[0].status,
        GoalProgress::Completed
    ));
    assert!(filer.calls.borrow().is_empty(), "MarkDone files no issue");
}

#[test]
fn out_of_scope_goal_is_dropped_from_the_board() {
    // A goal whose linked issue is an explicit out-of-scope handoff → Drop.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("lbug-patched");
    goal.wip_refs = vec![issue_ref(
        "1",
        "out-of-scope for this daemon; filed upstream",
    )];
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    let mut report = Default::default();
    for _ in 0..threshold {
        report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome("lbug-patched")],
            &evidence,
            &filer,
            threshold,
        );
    }
    assert_eq!(report.dropped, vec!["lbug-patched".to_string()]);
    assert!(
        state.active_goals.active.is_empty(),
        "dropped goal must leave the active board"
    );
    assert!(filer.calls.borrow().is_empty(), "Drop files no issue");
}

#[test]
fn concrete_progress_resets_the_no_action_counter() {
    // no-action, no-action, PROGRESS, no-action → only 1 consecutive at the end,
    // so the breaker never fires even though total no-action cycles >= threshold.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: true,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    for _ in 0..(threshold - 1) {
        let r = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome("g")],
            &evidence,
            &filer,
            threshold,
        );
        assert!(!r.fired());
    }
    // Concrete progress resets.
    let r = apply_no_progress_breaker_with_threshold(
        &mut state,
        &[progress_outcome("g")],
        &evidence,
        &filer,
        threshold,
    );
    assert!(!r.fired());

    // From here, threshold-1 more no-action cycles still must not fire.
    for _ in 0..(threshold - 1) {
        let r = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome("g")],
            &evidence,
            &filer,
            threshold,
        );
        assert!(
            !r.fired(),
            "reset must prevent firing before a fresh threshold"
        );
    }
    assert!(filer.calls.borrow().is_empty());
    assert!(matches!(
        state.active_goals.active[0].status,
        GoalProgress::NotStarted
    ));
}

#[test]
fn stale_counter_is_pruned_when_goal_leaves_the_board() {
    // A goal that accumulates a no-action count, then leaves the board, must not
    // leak a counter entry.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: true,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    apply_no_progress_breaker_with_threshold(
        &mut state,
        &[no_action_outcome("g")],
        &evidence,
        &filer,
        threshold,
    );
    assert_eq!(state.no_progress_tracker.consecutive("g"), 1);

    // Goal removed from the board (e.g. operator dropped it).
    state.active_goals.active.clear();
    apply_no_progress_breaker_with_threshold(&mut state, &[], &evidence, &filer, threshold);
    assert_eq!(
        state.no_progress_tracker.consecutive("g"),
        0,
        "counter for a departed goal must be pruned"
    );
}
