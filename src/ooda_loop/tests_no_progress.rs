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
    fn file_issue(&self, title: &str, body: &str) -> Option<super::no_progress::FiledIssue> {
        let mut calls = self.calls.borrow_mut();
        calls.push((title.to_string(), body.to_string()));
        // Fabricate a deterministic, distinct issue number per filing so the
        // escalation path can link it back to the goal (mirrors `gh` returning
        // the created issue's number).
        Some(super::no_progress::FiledIssue {
            number: format!("{}", 9000 + calls.len()),
            url: None,
        })
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

// --- perpetual/standing-goal exemption (issue #2589) ------------------------

/// A STANDING/PERPETUAL goal (issues #2580/#2589): its description carries the
/// durable standing marker, so it is recognised by the *same* `is_perpetual()`
/// flag the non-completability path keys on. Modeled on the live
/// `continuously-research-and-improve-your-own-cogn-*` research goal, at 0%.
fn perpetual_goal(id: &str) -> ActiveGoal {
    let g = ActiveGoal::new(
        id,
        "STANDING PERPETUAL goal — never mark complete; continuously research \
         and improve your own cognition",
        5,
    );
    assert!(
        g.is_perpetual(),
        "fixture must be recognised as standing/perpetual by the shared #2580/#2589 flag"
    );
    g
}

#[test]
fn perpetual_goal_idles_instead_of_blocking_past_threshold() {
    // The production defect: a STANDING/PERPETUAL goal is inherently bursty — it
    // ships durable improvements periodically and idles between. Idle cycles are
    // NORMAL, not a livelock, so the no-progress SAFEGUARD must NEVER hard-block
    // it. Driven N+1 consecutive no-action cycles (one past where a normal goal
    // is parked), it must: stay ACTIVE (never Blocked), file NO tracking issue,
    // set NO [OODA-SAFEGUARD] sentinel, never be escalated, be reported as a
    // perpetual idle, and have its no-action counter reset every cycle.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve";
    let mut goal = perpetual_goal(id);
    goal.wip_refs = vec![pr_ref("7")]; // an open, unmerged PR
    let mut state = state_with(goal);
    // No completion evidence and no obsolescence signal: a NORMAL goal in this
    // exact situation would be escalated + Blocked at the threshold.
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    for cycle in 1..=(threshold + 1) {
        let report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome(id)],
            &evidence,
            &filer,
            threshold,
        );

        assert!(
            !report.fired(),
            "cycle {cycle}: a perpetual goal idling must not fire the breaker"
        );
        assert!(
            report.escalated.is_empty(),
            "cycle {cycle}: a perpetual goal must never be escalated"
        );
        assert_eq!(
            report.perpetual_idled,
            vec![id.to_string()],
            "cycle {cycle}: the idle must be recorded as a perpetual idle (normal, not a fault)"
        );
        assert!(
            !matches!(
                state.active_goals.active[0].status,
                GoalProgress::Blocked(_)
            ),
            "cycle {cycle}: a perpetual goal must never be Blocked by the no-progress breaker"
        );
        assert!(
            !state.active_goals.active.iter().any(|g| matches!(
                &g.status,
                GoalProgress::Blocked(r) if is_no_progress_marker(r)
            )),
            "cycle {cycle}: no [OODA-SAFEGUARD] sentinel may ever be set on a perpetual goal"
        );
        assert_eq!(
            state.no_progress_tracker.consecutive(id),
            0,
            "cycle {cycle}: the perpetual goal's no-action counter must reset each idle cycle"
        );
    }

    // The goal is still on the board, available for the next cycle, and nothing
    // was ever escalated to a human.
    assert_eq!(
        state.active_goals.active.len(),
        1,
        "the perpetual goal must remain active and re-selectable"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "a perpetual goal must never file an [OODA-SAFEGUARD] tracking issue"
    );
}

#[test]
fn non_perpetual_goal_still_escalates_and_is_never_reported_idled() {
    // Regression guard: a NORMAL (non-standing) goal at the same N+1 no-action
    // cycles keeps the existing safeguard behaviour EXACTLY — escalated, Blocked
    // with the sentinel, one issue filed — and is never mistaken for a perpetual
    // idle. This proves the exemption keys on the shared perpetual flag only.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "normal-livelocked-goal";
    let mut goal = stuck_goal(id);
    assert!(
        !goal.is_perpetual(),
        "control fixture must NOT be standing/perpetual"
    );
    goal.wip_refs = vec![pr_ref("9")];
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    // Below threshold: nothing fires, and no perpetual-idle is ever reported.
    for cycle in 1..threshold {
        let report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome(id)],
            &evidence,
            &filer,
            threshold,
        );
        assert!(!report.fired(), "cycle {cycle} must not fire");
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: a normal goal is never a perpetual idle"
        );
    }

    // Threshold cycle: escalate + Block with the sentinel, exactly as before.
    let report = apply_no_progress_breaker_with_threshold(
        &mut state,
        &[no_action_outcome(id)],
        &evidence,
        &filer,
        threshold,
    );
    assert_eq!(report.escalated, vec![id.to_string()]);
    assert!(
        report.perpetual_idled.is_empty(),
        "a normal goal must never be reported as a perpetual idle"
    );
    assert_eq!(filer.calls.borrow().len(), 1, "exactly one issue filed");
    match &state.active_goals.active[0].status {
        GoalProgress::Blocked(reason) => assert!(
            is_no_progress_marker(reason),
            "the normal goal must still carry the no-progress sentinel: {reason}"
        ),
        other => panic!("expected Blocked, got {other:?}"),
    }
}
