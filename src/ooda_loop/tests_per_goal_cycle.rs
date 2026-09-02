//! TDD regression tests for the per-goal-per-cycle driver loop (issue #4453).
//!
//! These tests define the contract of the NEW driver that runs EXACTLY ONE
//! agentic decision per active goal per cycle and routes the outcome through a
//! THIN deterministic rail. They currently fail to compile — the driver
//! (`cycle::drive_per_goal_cycle`) and its observable outcome
//! (`cycle::PerGoalDecisionOutcome`) do not exist yet.
//!
//! Guard rails encoded here (design A6/A7, brief acceptance):
//!   * T1 anti-loop  — a 70ab8541-style standing research goal, driven over N
//!     cycles by a brain that only ever returns continue/spawn/investigate, is
//!     NEVER self-reset: its wip_refs survive and it never rolls to NotStarted.
//!   * one-decision  — the driver calls the brain EXACTLY once per active goal
//!     and records a judgment (with a non-empty reason) for every goal; none is
//!     left idle without both an action and a reason.
//!   * input-not-decider — an alarming demoted signal (stale claim / standing
//!     idle) fed into ctx does NOT itself reclaim/roll; only the brain's
//!     reasoned action can, proving the imperative deciders were demoted.
//!   * T2 investigate-first — a stale-worker scenario reaches a destructive
//!     roll ONLY after an `investigate` verdict was recorded first.

use std::collections::VecDeque;
use std::sync::Mutex;

use crate::error::SimardResult;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_brain::{
    BrainPhase, EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain, PerGoalAction,
    PerGoalCycleCtx, take_brain_judgments, with_brain_judgment_scope,
};
use crate::ooda_loop::OodaState;
use crate::ooda_loop::cycle::drive_per_goal_cycle;

// ---------------------------------------------------------------------------
// Scripted per-goal brain test double (design A7 — no live recipe subprocess)
// ---------------------------------------------------------------------------

/// Pops one scripted `PerGoalAction` per `decide_per_goal_cycle` call and
/// records every `PerGoalCycleCtx` it was handed, so tests can assert both the
/// driver's routing AND that the gather step wired durable state + the three
/// demoted signals into the ctx.
struct ScriptedPerGoalBrain {
    script: Mutex<VecDeque<PerGoalAction>>,
    seen: Mutex<Vec<PerGoalCycleCtx>>,
}

impl ScriptedPerGoalBrain {
    fn new(actions: impl IntoIterator<Item = PerGoalAction>) -> Self {
        Self {
            script: Mutex::new(actions.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<PerGoalCycleCtx> {
        self.seen.lock().unwrap().clone()
    }
}

impl OodaBrain for ScriptedPerGoalBrain {
    fn decide_engineer_lifecycle(
        &self,
        _ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        Ok(EngineerLifecycleDecision::ContinueSkipping {
            rationale: "not under test".into(),
        })
    }

    fn decide_per_goal_cycle(&self, ctx: &PerGoalCycleCtx) -> SimardResult<PerGoalAction> {
        self.seen.lock().unwrap().push(ctx.clone());
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted brain: more decisions requested than scripted"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_pr_ref() -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: "4453".to_string(),
        label: "open PR in review".to_string(),
        url: None,
    }
}

/// The live 70ab8541 goal: a STANDING PERPETUAL cognition-research goal that
/// currently holds an open, unmerged PR (live in-flight progress).
fn standing_research_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(
        id,
        "Continuously research and improve your own cognition: graph memory, \
         recall quality, distillation fact-yield, and reasoner reliability. \
         STANDING PERPETUAL goal — durable improvements only",
        1,
    );
    g.assigned_to = Some("engineer-r".to_string());
    g.status = GoalProgress::InProgress { percent: 30 };
    g.wip_refs = vec![live_pr_ref()];
    g
}

fn state_with(goals: Vec<ActiveGoal>) -> OodaState {
    let mut board = GoalBoard::new();
    board.active = goals;
    OodaState::new(board)
}

fn cont(reason: &str) -> PerGoalAction {
    PerGoalAction::Continue {
        reason: reason.into(),
    }
}

// ---------------------------------------------------------------------------
// T1 — anti-loop: a standing research goal is NEVER self-reset (70ab8541)
// ---------------------------------------------------------------------------

#[test]
fn t1_standing_research_goal_never_idle_resets_over_many_cycles() {
    let goal_id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    // Brain only ever chooses non-destructive actions across N cycles.
    let script = [
        cont("engineer healthy, PR in review"),
        PerGoalAction::Spawn {
            reason: "PR merged; dispatch the next source".into(),
            task_hint: "survey new graph-memory papers".into(),
        },
        PerGoalAction::Investigate {
            reason: "engineer quiet; look at logs".into(),
        },
        cont("still making progress"),
        PerGoalAction::Wait {
            reason: "next PR awaiting CI".into(),
        },
    ];
    let n = script.len();
    let brain = ScriptedPerGoalBrain::new(script);
    let mut state = state_with(vec![standing_research_goal(goal_id)]);

    for cycle in 0..n {
        let outcomes = with_brain_judgment_scope(|| {
            let outcomes = drive_per_goal_cycle(&mut state, &brain).expect("driver ok");
            let judgments = take_brain_judgments();
            // Every active goal must have produced a recorded judgment with a
            // non-empty reason — none left idle without an action + reason.
            assert_eq!(
                judgments.len(),
                1,
                "cycle {cycle}: exactly one per-goal judgment expected"
            );
            let j = &judgments[0];
            assert_eq!(j.phase, BrainPhase::PerGoalCycle);
            assert!(
                !j.rationale.trim().is_empty(),
                "cycle {cycle}: every decision must record a non-empty reason"
            );
            outcomes
        });

        assert_eq!(outcomes.len(), 1, "cycle {cycle}: one outcome per goal");
        let goal = &state.active_goals.active[0];
        // The load-bearing invariant: continue/spawn/investigate/wait NEVER
        // wipe the live PR ref nor roll the goal back to NotStarted.
        assert_eq!(
            goal.wip_refs.len(),
            1,
            "cycle {cycle}: the live PR ref must survive (no idle→reset loop)"
        );
        assert!(
            !matches!(goal.status, GoalProgress::NotStarted),
            "cycle {cycle}: a healthy standing research goal must never be rolled to NotStarted"
        );
        assert!(
            !outcomes[0].touched_refs,
            "cycle {cycle}: no destructive ref mutation for a continue/spawn/investigate/wait verdict"
        );
    }
}

// ---------------------------------------------------------------------------
// one decision + recorded reason per active goal per cycle
// ---------------------------------------------------------------------------

#[test]
fn driver_makes_exactly_one_decision_per_active_goal() {
    let goals = vec![
        standing_research_goal("g-research-70ab8541"),
        {
            let mut g = ActiveGoal::new("g-ci", "Steward CI health. STANDING PERPETUAL goal.", 1);
            g.assigned_to = Some("engineer-ci".to_string());
            g
        },
        ActiveGoal::new("g-feature", "Ship the export feature", 2),
    ];
    let brain = ScriptedPerGoalBrain::new([
        cont("research healthy"),
        cont("ci quiet is normal"),
        PerGoalAction::Spawn {
            reason: "start the feature".into(),
            task_hint: String::new(),
        },
    ]);
    let mut state = state_with(goals);

    let (outcomes, judgments) = with_brain_judgment_scope(|| {
        let outcomes = drive_per_goal_cycle(&mut state, &brain).expect("driver ok");
        (outcomes, take_brain_judgments())
    });

    // Exactly one brain call, one outcome, and one judgment per active goal.
    assert_eq!(brain.seen().len(), 3, "one brain call per active goal");
    assert_eq!(outcomes.len(), 3, "one outcome per active goal");
    assert_eq!(judgments.len(), 3, "one recorded judgment per active goal");

    // Every active goal id appears exactly once — none skipped, none doubled.
    let mut ids: Vec<String> = outcomes.iter().map(|o| o.goal_id.clone()).collect();
    ids.sort();
    assert_eq!(ids, ["g-ci", "g-feature", "g-research-70ab8541"]);
    for o in &outcomes {
        assert!(
            !o.reason.trim().is_empty(),
            "goal {} must carry a recorded reason",
            o.goal_id
        );
    }
}

// ---------------------------------------------------------------------------
// input-not-decider: a demoted signal never autonomously reclaims/rolls
// ---------------------------------------------------------------------------

#[test]
fn alarming_demoted_signal_does_not_autonomously_reclaim_or_roll() {
    // A standing research goal with NO live ref (empty wip_refs) is exactly the
    // shape the old classify_standing_idle would have faulted + rolled, and a
    // quiet worker is what the old reaper would have reclaimed. With the
    // deciders demoted, a brain that says Continue must leave the goal intact.
    let mut goal = standing_research_goal("g-idle-70ab8541");
    goal.wip_refs.clear(); // looks "idle"
    let assigned_before = goal.assigned_to.clone();
    let brain = ScriptedPerGoalBrain::new([cont("bursty standing goal — idle is normal")]);
    let mut state = state_with(vec![goal]);

    let outcomes =
        with_brain_judgment_scope(|| drive_per_goal_cycle(&mut state, &brain).expect("driver ok"));

    let g = &state.active_goals.active[0];
    assert!(
        !matches!(g.status, GoalProgress::NotStarted),
        "a Continue verdict must NOT roll the goal, even though it looks idle \
         (threshold/idle signal is an INPUT, not the decision)"
    );
    assert_eq!(
        g.assigned_to, assigned_before,
        "no autonomous reclaim: the engineer assignment must be untouched"
    );
    assert!(
        !outcomes[0].touched_refs,
        "a Continue verdict performs no destructive ref mutation"
    );

    // And the demoted signal WAS surfaced to the brain as an input.
    let ctx = &brain.seen()[0];
    assert_eq!(
        ctx.goal_id, "g-idle-70ab8541",
        "gather must wire the goal id"
    );
    assert!(
        ctx.standing_idle_signal,
        "the demoted standing-idle classifier must surface as a read-only ctx input"
    );
}

// ---------------------------------------------------------------------------
// T2 — investigate BEFORE any destructive action for a quiet/stale worker
// ---------------------------------------------------------------------------

#[test]
fn t2_stale_worker_is_investigated_before_any_destructive_roll() {
    let goal_id = "g-stale-worker-70ab8541";
    // A stale-claim signal is present. The brain's FIRST verdict must be
    // Investigate (look at logs/tools); only a LATER cycle may reorient.
    let brain = ScriptedPerGoalBrain::new([
        PerGoalAction::Investigate {
            reason: "engineer heartbeat went quiet; inspect logs before reclaiming".into(),
        },
        PerGoalAction::Reorient {
            reason: "logs confirm the engineer died; reclaim and redirect".into(),
        },
    ]);
    let mut goal = standing_research_goal(goal_id);
    goal.wip_refs = vec![WipRef {
        kind: "engineer".to_string(),
        ref_id: "engineer-r".to_string(),
        label: "engineer session".to_string(),
        url: None,
    }];
    let mut state = state_with(vec![goal]);

    // Cycle 1: Investigate — MUST NOT touch refs (no reclaim yet).
    let c1 = with_brain_judgment_scope(|| {
        let outcomes = drive_per_goal_cycle(&mut state, &brain).expect("driver ok");
        let judgments = take_brain_judgments();
        (outcomes, judgments)
    });
    assert_eq!(c1.0[0].action_label, "investigate");
    assert!(
        !c1.0[0].touched_refs,
        "investigate must precede — and never itself perform — a destructive reclaim/roll"
    );
    assert_eq!(
        state.active_goals.active[0].wip_refs.len(),
        1,
        "the worker ref must survive the investigate cycle"
    );
    assert_eq!(c1.1[0].decision, "investigate");

    // Cycle 2: Reorient — the destructive step, reached ONLY after investigate.
    let c2 = with_brain_judgment_scope(|| {
        let outcomes = drive_per_goal_cycle(&mut state, &brain).expect("driver ok");
        let judgments = take_brain_judgments();
        (outcomes, judgments)
    });
    assert_eq!(c2.0[0].action_label, "reorient");
    assert!(
        c2.0[0].touched_refs,
        "reorient is the destructive step and clears the refs"
    );
    assert!(
        state.active_goals.active[0].wip_refs.is_empty(),
        "reorient reclaims the stale worker's refs — but only after investigate"
    );

    // The stale-claim threshold was fed as an INPUT, never used as the decider.
    let ctx = &brain.seen()[0];
    assert!(
        ctx.stale_claim_secs.is_some(),
        "the demoted reaper threshold must surface as a read-only ctx input"
    );
}

// ---------------------------------------------------------------------------
// judgment fidelity: the recorded decision label matches the applied action
// ---------------------------------------------------------------------------

#[test]
fn recorded_judgment_label_matches_applied_action() {
    let brain = ScriptedPerGoalBrain::new([PerGoalAction::Complete {
        reason: "success criteria observed live".into(),
    }]);
    let mut state = state_with(vec![standing_research_goal("g-done")]);

    let (outcomes, judgments) = with_brain_judgment_scope(|| {
        let o = drive_per_goal_cycle(&mut state, &brain).expect("driver ok");
        (o, take_brain_judgments())
    });

    assert_eq!(outcomes[0].action_label, "complete");
    assert_eq!(judgments[0].decision, "complete");
    assert!(matches!(
        state.active_goals.active[0].status,
        GoalProgress::Completed
    ));
}
