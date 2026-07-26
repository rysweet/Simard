//! TDD failing tests: the fail-CLOSED rail contract for the typed decision
//! seam (WS-4, issues #1711 / #2573 / #2658).
//!
//! `RecipeBrain::decide_per_goal_cycle` will read the reasoner's verdict from a
//! typed record via `read_verified`. EVERY failure mode of that read (absent /
//! malformed / wrong-schema / out-of-enum / empty-reason / goal-mismatch /
//! cycle-mismatch) surfaces as an `Err` from the brain. These tests pin the
//! rail-side half of that contract using a hermetic brain double that returns
//! the SAME `Err` a fail-CLOSED `read_verified` produces — no recipe subprocess,
//! no filesystem:
//!
//!   * a brain `Err` ⇒ `drive_per_goal_cycle` returns `Err` and performs a
//!     **safe no-op**: the goal's load-bearing `wip_refs`, assignment, and
//!     status are UNCHANGED (no ref mutation, no roll, no reap).
//!   * `PerGoalAction::mutates_refs()` still reports exactly `Reorient` /
//!     `Complete` — the A6 invariant the whole seam protects.
//!
//! These reference the existing `drive_per_goal_cycle` driver, which already
//! propagates a brain `Err` via `?` BEFORE applying any action; the seam change
//! only alters WHERE the `Err` originates, so this contract must hold unchanged.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_brain::{
    EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain, PerGoalAction, PerGoalCycleCtx,
};
use crate::ooda_loop::OodaState;
use crate::ooda_loop::cycle::drive_per_goal_cycle;

// ---------------------------------------------------------------------------
// Brain double: always fails, exactly as a fail-CLOSED `read_verified` does.
// ---------------------------------------------------------------------------

/// Returns the same `AdapterInvocationFailed` `Err` that a malformed / absent /
/// mismatched decision record yields — so the rail sees precisely the
/// fail-CLOSED signal the real `RecipeBrain` will emit.
struct FailClosedBrain;

impl OodaBrain for FailClosedBrain {
    fn decide_engineer_lifecycle(
        &self,
        _ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        Ok(EngineerLifecycleDecision::ContinueSkipping {
            rationale: "not under test".into(),
        })
    }

    fn decide_per_goal_cycle(&self, _ctx: &PerGoalCycleCtx) -> SimardResult<PerGoalAction> {
        Err(SimardError::AdapterInvocationFailed {
            base_type: "recipe-per-goal-cycle-brain".to_string(),
            reason: "per-goal decision record failed verification (fail-CLOSED)".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_pr_ref() -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: "4720".to_string(),
        label: "open PR in review".to_string(),
        url: None,
    }
}

/// One active goal holding a LIVE in-flight PR ref + an assigned engineer, so
/// the no-op assertion can prove NONE of it was touched on a fail-CLOSED cycle.
fn state_with_live_goal(goal_id: &str) -> OodaState {
    let mut goal = ActiveGoal::new(goal_id, "ship the feature", 1);
    goal.assigned_to = Some("engineer-a".to_string());
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.wip_refs = vec![live_pr_ref()];
    let mut board = GoalBoard::new();
    board.active.push(goal);
    OodaState::new(board)
}

// ---------------------------------------------------------------------------
// Fail-CLOSED rail contract
// ---------------------------------------------------------------------------

#[test]
fn brain_err_makes_the_driver_fail_and_mutates_nothing() {
    let goal_id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut state = state_with_live_goal(goal_id);
    let brain = FailClosedBrain;

    let result = drive_per_goal_cycle(&mut state, &brain);
    assert!(
        result.is_err(),
        "a fail-CLOSED brain Err MUST surface as a driver Err (no silent fallback, #1711)"
    );

    let g = &state.active_goals.active[0];
    assert_eq!(
        g.wip_refs.len(),
        1,
        "fail-CLOSED cycle MUST NOT clear the load-bearing wip_refs (safe no-op)"
    );
    assert_eq!(
        g.assigned_to.as_deref(),
        Some("engineer-a"),
        "fail-CLOSED cycle MUST NOT release the engineer assignment (no reap)"
    );
    assert!(
        !matches!(g.status, GoalProgress::NotStarted),
        "fail-CLOSED cycle MUST NOT roll the goal back to NotStarted"
    );
    assert!(
        !matches!(g.status, GoalProgress::Completed),
        "fail-CLOSED cycle MUST NOT complete the goal"
    );
}

#[test]
fn fail_closed_never_produces_a_default_action_outcome() {
    // The driver returns Err with no outcomes — there is NO default `Continue`
    // (or any other action) synthesized on the failure path (#1711).
    let mut state = state_with_live_goal("g1");
    let brain = FailClosedBrain;
    assert!(
        drive_per_goal_cycle(&mut state, &brain).is_err(),
        "no default action may be produced on a fail-CLOSED read"
    );
}

// ---------------------------------------------------------------------------
// A6 invariant guard — mutates_refs is exactly {Reorient, Complete}. The seam
// reads a validated enum; the rail's destructive-action set MUST NOT drift.
// ---------------------------------------------------------------------------

#[test]
fn only_reorient_and_complete_mutate_refs() {
    let non_mutating = [
        PerGoalAction::Continue { reason: "r".into() },
        PerGoalAction::Spawn {
            reason: "r".into(),
            task_hint: String::new(),
        },
        PerGoalAction::Investigate { reason: "r".into() },
        PerGoalAction::Wait { reason: "r".into() },
    ];
    for action in non_mutating {
        assert!(
            !action.mutates_refs(),
            "{action:?} MUST NOT be a ref-mutating action (A6 anti-loop invariant)"
        );
    }

    for action in [
        PerGoalAction::Reorient { reason: "r".into() },
        PerGoalAction::Complete { reason: "r".into() },
    ] {
        assert!(
            action.mutates_refs(),
            "{action:?} MUST remain a ref-mutating action"
        );
    }
}
