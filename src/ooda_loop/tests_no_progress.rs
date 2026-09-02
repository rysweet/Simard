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
use std::collections::HashSet;

use super::cycle::sweep_stale_assignments_with_sessions;
use super::no_progress::{
    NoProgressBreakerReport, NoProgressIssueFiler, ResearchIdleFault, StandingIdle,
    apply_no_progress_breaker_with_threshold, classify_standing_idle, prune_merged_pr_refs,
};
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

/// A `session` wip_ref keyed on the engineer tmux session name — a LIVE kind
/// (`has_live_in_flight_ref`) until the stale-assignment sweep drops it.
fn session_ref(session: &str) -> WipRef {
    WipRef {
        kind: "session".to_string(),
        ref_id: session.to_string(),
        label: format!("engineer session {session}"),
        url: None,
    }
}

/// A `branch` wip_ref for the dead engineer's working branch — a LIVE kind that
/// must be dropped alongside the session when its tmux session dies.
fn branch_ref(name: &str) -> WipRef {
    WipRef {
        kind: "branch".to_string(),
        ref_id: name.to_string(),
        label: format!("branch {name}"),
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

/// A STANDING/PERPETUAL but **non-research** goal (issues #2580/#2589): a
/// CI-stewardship charter whose bursty idling is genuinely benign. It carries
/// the durable standing marker (so `is_perpetual()` holds) but NO cognition /
/// research marker, so `is_standing_research_goal()` is false. This is the goal
/// class that KEEPS the benign perpetual-idle exemption (issue #4399): for it an
/// idle cycle is normal, not a fault. Deliberately kept distinct from
/// [`standing_research_goal`] so the #4399 fixture split can never drift the two
/// exemption paths back together.
fn perpetual_goal(id: &str) -> ActiveGoal {
    let g = ActiveGoal::new(id, "Steward CI health. STANDING PERPETUAL goal.", 5);
    assert!(
        g.is_perpetual(),
        "fixture must be recognised as standing/perpetual by the shared #2580/#2589 flag"
    );
    assert!(
        !g.is_standing_research_goal(),
        "the benign-exemption fixture must NOT read as a standing research goal (#4399)"
    );
    g
}

/// A STANDING/PERPETUAL **research** goal (issue #4399): standing/perpetual AND
/// marked cognition-research, i.e. `is_standing_research_goal()` holds. Modeled
/// on the live `continuously-research-and-improve-your-own-cogn-70ab8541` goal.
/// For this class an idle cycle is a FAULT — the breaker records it in
/// `research_idle_faults` (never `perpetual_idled`) as a SIGNAL; re-orienting the
/// goal is the agentic per-goal reasoner's job (#4453), not the breaker's.
fn standing_research_goal(id: &str) -> ActiveGoal {
    let g = ActiveGoal::new(
        id,
        "Continuously research and improve your own cognition: graph memory, \
         recall quality, and reasoner reliability. STANDING PERPETUAL goal.",
        5,
    );
    assert!(
        g.is_standing_research_goal(),
        "fixture must read as a standing research goal (#4399)"
    );
    g
}

#[test]
fn perpetual_goal_idles_instead_of_blocking_past_threshold() {
    // A STANDING/PERPETUAL but NON-research goal (CI stewardship) is inherently
    // bursty — it ships durable improvements periodically and idles between. For
    // this class idle cycles are NORMAL, not a livelock, so the no-progress
    // SAFEGUARD must NEVER hard-block it. Driven N+1 consecutive no-action cycles
    // (one past where a normal goal is parked), it must: stay ACTIVE (never
    // Blocked), file NO tracking issue, set NO [OODA-SAFEGUARD] sentinel, never be
    // escalated, be reported as a (benign) perpetual idle, and have its no-action
    // counter reset every cycle. (Research goals take the fault path instead —
    // see `research_goal_idle_is_a_fault_not_a_benign_perpetual_idle`.)
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "steward-ci-health";
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

// --- research goal never-idle: idle is a FAULT, not exempt (issue #4399) -----

#[test]
fn research_goal_idle_is_a_fault_not_a_benign_perpetual_idle() {
    // The #4399 defect: the standing RESEARCH goal was silently swept into the
    // benign perpetual-idle exemption ("standing/perpetual goal idled this cycle
    // — normal, not a fault"), so over many cycles it produced nothing new. Under
    // the never-idle rail an idle research cycle is a FAULT: it is recorded in
    // `research_idle_faults` — NEVER `perpetual_idled` — while STILL staying
    // fail-closed (never blocked, never escalated, never a firing). Driven N+1
    // consecutive no-action cycles (past where a normal goal is parked).
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    // Genuinely idle: NO live in-flight artifact (empty wip_refs). This is the
    // case that MUST fault — a research goal holding an open PR is progress
    // (ResearchInFlight), not an idle fault; see
    // `research_goal_with_live_pr_is_in_flight_progress_not_a_fault`.
    goal.wip_refs.clear();
    assert!(!goal.has_live_in_flight_ref());
    let mut state = state_with(goal);
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
        assert_eq!(
            report.research_idle_faults,
            vec![id.to_string()],
            "cycle {cycle}: a research-goal idle must be recorded as a FAULT"
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: a research goal must NOT get the benign perpetual-idle exemption"
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: an idle fault is a fail-closed re-orient, not a breaker firing"
        );
        assert!(
            report.escalated.is_empty(),
            "cycle {cycle}: a research idle must never escalate to a human"
        );
    }
    assert!(
        filer.calls.borrow().is_empty(),
        "a research-goal idle must never file an [OODA-SAFEGUARD] tracking issue"
    );
}

#[test]
fn research_goal_with_live_pr_is_in_flight_progress_not_a_fault() {
    // Crusty finding 1 (HIGH): a research goal that opened a durable PR (a genuine
    // novel action) and then produces a no-action cycle while that PR is still
    // open/unmerged is NOT meaningfully idle — it holds a live in-flight artifact.
    // The never-idle rail must treat it as PROGRESS (ResearchInFlight): the goal
    // must NOT be counted as a research_idle_fault and must NOT be re-oriented,
    // because roll_to_new_cycle would wipe the load-bearing wip_refs the Overseer
    // dedup set, engineer-admission control, and completion gate depend on — which
    // would let the next cycle spawn an overlapping engineer on the same seam and
    // lose merge tracking of the open PR. Its wip_refs, assignment, and status
    // must be PRESERVED; the counter still resets and it stays active.
    // (This replaces the pre-fix assertion that the PR ref was DROPPED — that
    // asserted the buggy behavior.)
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.assigned_to = Some("engineer-42".to_string());
    goal.wip_refs = vec![pr_ref("7")]; // an open, unmerged PR — live in-flight work
    assert!(goal.has_live_in_flight_ref());
    let mut state = state_with(goal);
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
            report.research_idle_faults.is_empty(),
            "cycle {cycle}: a research goal holding a live PR is in-flight progress, not an idle fault"
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: in-flight progress is neither a fault nor a benign perpetual idle"
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: in-flight progress must never fire the breaker"
        );

        let goal = &state.active_goals.active[0];
        assert_eq!(
            goal.wip_refs,
            vec![pr_ref("7")],
            "cycle {cycle}: the open PR ref must be PRESERVED (dedup/admission/merge-tracking depend on it)"
        );
        assert_eq!(
            goal.assigned_to.as_deref(),
            Some("engineer-42"),
            "cycle {cycle}: assignment must be preserved for the in-flight goal"
        );
        assert!(
            matches!(goal.status, GoalProgress::InProgress { percent: 40 }),
            "cycle {cycle}: an in-flight research goal must NOT be reset to NotStarted, got {:?}",
            goal.status
        );
        assert!(
            !matches!(goal.status, GoalProgress::Blocked(_)),
            "cycle {cycle}: an in-flight research goal must never be Blocked"
        );
        assert_eq!(
            state.no_progress_tracker.consecutive(id),
            0,
            "cycle {cycle}: the no-action counter must still reset each cycle (stays active)"
        );
    }
    assert_eq!(
        state.active_goals.active.len(),
        1,
        "the research goal must remain active and re-selectable"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "in-flight progress must never file an [OODA-SAFEGUARD] tracking issue"
    );
}

#[test]
fn research_goal_idle_with_no_live_ref_faults_but_does_not_reorient() {
    // Issue #4453: the imperative no-progress breaker records a GENUINELY idle
    // research goal (empty wip_refs — no live in-flight artifact) as a FAULT
    // SIGNAL, but it must NOT itself re-orient the goal. The re-orient decision
    // (and the destructive `roll_to_new_cycle`) is owned exclusively by the
    // agentic per-goal-per-cycle reasoner (`drive_per_goal_cycle`). If this
    // imperative path also rolled the goal it would double-drive it — resetting
    // it to `NotStarted` and dropping WIP even when the reasoner decided to
    // `wait`/`continue` — which was the 70ab8541 idle→reset fault-loop. So the
    // goal's status and WIP must survive the breaker unchanged, while it stays
    // fail-closed: never blocked/parked, its no-action counter reset each cycle,
    // and staying active for the reasoner to decide on.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.wip_refs.clear(); // genuinely idle: no live in-flight artifact
    assert!(!goal.has_live_in_flight_ref());
    let mut state = state_with(goal);
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
        assert_eq!(
            report.research_idle_faults,
            vec![id.to_string()],
            "cycle {cycle}: a genuinely idle research goal must be recorded as a FAULT signal"
        );
        let goal = &state.active_goals.active[0];
        assert!(
            matches!(goal.status, GoalProgress::InProgress { percent: 40 }),
            "cycle {cycle}: the imperative breaker must NOT re-orient the goal \
             (re-orient is the reasoner's job, #4453) — status must be unchanged, got {:?}",
            goal.status
        );
        assert!(
            !matches!(goal.status, GoalProgress::Blocked(_)),
            "cycle {cycle}: a research goal must never be Blocked by the no-progress breaker"
        );
        assert!(
            !state.active_goals.active.iter().any(|g| matches!(
                &g.status,
                GoalProgress::Blocked(r) if is_no_progress_marker(r)
            )),
            "cycle {cycle}: no [OODA-SAFEGUARD] sentinel may ever be set on a research goal"
        );
        assert_eq!(
            state.no_progress_tracker.consecutive(id),
            0,
            "cycle {cycle}: the research goal's no-action counter must reset each idle cycle"
        );
    }
    assert_eq!(
        state.active_goals.active.len(),
        1,
        "the research goal must remain active and re-selectable"
    );
}

#[test]
fn research_idle_fault_is_not_a_firing_and_is_logged() {
    // Contract on the report itself: `research_idle_faults` is a fail-closed
    // re-orient signal, NOT a terminal breaker action, so it must never count as a
    // `fired()`. It must still be surfaced in the one-line cycle log so an idle
    // research fault is visible (never silent, unlike the old benign exemption).
    let mut report = NoProgressBreakerReport::default();
    report
        .research_idle_faults
        .push("continuously-research-and-improve-your-own-cogn-70ab8541".to_string());
    assert!(
        !report.fired(),
        "a research idle fault is a re-orient, not a terminal breaker firing"
    );
    let line = report.log_line();
    assert!(
        line.contains("research_faults=1"),
        "the cycle log must surface the research idle fault count: {line}"
    );
}

#[test]
fn research_idle_fault_alone_makes_the_report_noteworthy_so_the_count_is_surfaced() {
    // Crusty finding 2 (observability): on a PURE research-idle cycle the report
    // does NOT fire, auto-clear, or error — so before the fix the root-cause
    // breaker's aggregate log gate (`fired() || auto_cleared || errors`) stayed
    // false and `research_faults=N` never reached the cycle log. `is_noteworthy()`
    // is the single source of truth for that gate and MUST be true when a
    // research-idle fault occurred, so the count is consistently surfaced.
    let empty = NoProgressBreakerReport::default();
    assert!(
        !empty.is_noteworthy(),
        "a truly empty pass has nothing to surface"
    );

    let mut report = NoProgressBreakerReport::default();
    report
        .research_idle_faults
        .push("continuously-research-and-improve-your-own-cogn-70ab8541".to_string());
    assert!(
        !report.fired(),
        "the fault path must not be a firing (guards the gate is not merely fired())"
    );
    assert!(
        report.is_noteworthy(),
        "a pure research-idle cycle must be noteworthy so the aggregate log surfaces research_faults=N"
    );
}

// --- classify_standing_idle: the single, pure decision point (issue #4399) ---

#[test]
fn classify_standing_idle_flags_a_research_goal_as_a_fault() {
    // A standing RESEARCH goal (perpetual AND cognition-research) with NO live
    // in-flight artifact idling is a FAULT — the classifier returns ResearchFault
    // with a fixed-vocabulary category, never the benign exemption.
    let g = standing_research_goal("continuously-research-and-improve-your-own-cogn-70ab8541");
    assert!(
        !g.has_live_in_flight_ref(),
        "fixture must be genuinely idle"
    );
    assert_eq!(
        classify_standing_idle(&g),
        Some(StandingIdle::ResearchFault {
            fault: ResearchIdleFault::NoNovelActionProduced
        }),
        "a genuinely idle standing research goal must classify as a research fault"
    );
}

#[test]
fn classify_standing_idle_research_with_live_ref_is_in_flight_not_a_fault() {
    // Crusty finding 1: a standing research goal holding a LIVE in-flight artifact
    // (open PR / branch / session) is making progress — the classifier must return
    // ResearchInFlight, NOT ResearchFault, so the breaker preserves its wip_refs and
    // does not re-orient it.
    let mut g = standing_research_goal("continuously-research-and-improve-your-own-cogn-70ab8541");
    g.wip_refs = vec![pr_ref("7")];
    assert!(g.has_live_in_flight_ref());
    assert_eq!(
        classify_standing_idle(&g),
        Some(StandingIdle::ResearchInFlight),
        "a research goal holding a live PR must classify as in-flight progress, not a fault"
    );
}

#[test]
fn classify_standing_idle_keeps_non_research_standing_goal_benign() {
    // A standing NON-research goal (CI stewardship) idling is BENIGN — the
    // classifier returns BenignExempt, preserving the #2589 exemption.
    let g = perpetual_goal("steward-ci-health");
    assert_eq!(
        classify_standing_idle(&g),
        Some(StandingIdle::BenignExempt),
        "a non-research standing goal's idle must stay a benign exemption"
    );
}

#[test]
fn classify_standing_idle_is_none_for_an_ordinary_goal() {
    // A bounded, non-standing goal is never classified here — it falls through to
    // the normal escalation ladder.
    let g = ActiveGoal::new("ship-feature-x", "Ship feature X and close the epic.", 5);
    assert!(
        !g.is_perpetual(),
        "fixture must be an ordinary bounded goal"
    );
    assert_eq!(
        classify_standing_idle(&g),
        None,
        "an ordinary goal must not be classified as a standing idle"
    );
}

#[test]
fn classify_standing_idle_is_total_over_pathological_descriptions() {
    // Totality / panic-freedom (enforced under clippy -D warnings): the classifier
    // is pure and must never panic on empty, very-long, Unicode, or control-char
    // descriptions — only the structured predicates decide the branch.
    for desc in [
        String::new(),
        "🧠".repeat(4096),
        "STANDING PERPETUAL\u{0000}\u{202e}research cognition".to_string(),
        "x".repeat(100_000),
    ] {
        let g = ActiveGoal::new("pathological-id", &desc, 5);
        let _ = classify_standing_idle(&g); // must not panic
    }
}

// ===========================================================================
// NEW-1 (PR #4428): the ResearchInFlight exemption keys on ref KIND, not ref
// LIVENESS. Nothing pruned stale/merged/dead refs before the breaker ran, so a
// research goal holding a MERGED PR ref or a DEAD engineer session read as
// `has_live_in_flight_ref` forever and silently idled — reintroducing the #4399
// loophole behind a narrower gate. `wip_refs` must reflect only LIVE artifacts
// BEFORE the breaker classifies, via two liveness-pruning prongs:
//   Prong 1 — the stale-assignment sweep also drops a dead session's
//             session/branch/engineer refs (cycle.rs).
//   Prong 2 — a per-cycle, IO-free `prune_merged_pr_refs` reconcile drops `pr`
//             refs whose number is not in the open-PR set (no_progress.rs).
// The three tests below are the authoritative NEW-1 contract (fail before the
// fix, pass after). They deliberately reuse the round-1 breaker harness so the
// preserved-refs finding #1 (test c) and the never-idle guarantee (tests a/b)
// are exercised through the exact same shared classifier.
// ===========================================================================

/// (a) A standing research goal whose ONLY `wip_ref` is a MERGED/CLOSED PR.
///
/// Before the fix the kind-based guard reads that dead `pr` ref as live, so the
/// goal is classified `ResearchInFlight` on every subsequent NO-ACTION cycle:
/// never faulted, never re-oriented, never logged — it silently idles forever.
/// After Prong 2 prunes the ref (PR #7 is not in the open set), the now-refless
/// goal is correctly a `ResearchFault`: counted in `research_idle_faults`,
/// re-oriented (`NotStarted`), stays active, and is never `Blocked`.
#[test]
fn research_goal_whose_only_ref_is_a_merged_pr_faults_after_liveness_reconcile() {
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.assigned_to = Some("engineer-42".to_string());
    goal.wip_refs = vec![pr_ref("7")]; // its ONLY ref — a PR that has since merged/closed
    assert!(
        goal.has_live_in_flight_ref(),
        "precondition: the kind-based guard reads the stale PR ref as live (the NEW-1 loophole)"
    );
    let mut state = state_with(goal);

    // Prong 2: PR #7 is NOT in the open-PR set (it merged/closed), so the pure,
    // IO-free reconcile prunes it — leaving the goal with no live in-flight ref.
    let open_prs: HashSet<u32> = HashSet::new();
    prune_merged_pr_refs(&mut state.active_goals, &open_prs);
    assert!(
        !state.active_goals.active[0].has_live_in_flight_ref(),
        "a merged/closed PR ref must be pruned so the goal no longer reads as in-flight"
    );
    assert!(
        state.active_goals.active[0].wip_refs.is_empty(),
        "the goal's only (stale) ref was the merged PR — after prune wip_refs is empty"
    );

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
        assert_eq!(
            report.research_idle_faults,
            vec![id.to_string()],
            "cycle {cycle}: after the merged PR ref is pruned, an idle research goal is a FAULT (never ResearchInFlight)"
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: an idle research goal is a fault, not a benign perpetual idle"
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: a research idle fault is fail-closed — never a firing"
        );

        let goal = &state.active_goals.active[0];
        assert!(
            matches!(goal.status, GoalProgress::InProgress { percent: 40 }),
            "cycle {cycle}: the imperative breaker records the fault signal but must NOT \
             re-orient the goal (that is the reasoner's job, #4453) — status unchanged, got {:?}",
            goal.status
        );
        assert!(
            !matches!(goal.status, GoalProgress::Blocked(_)),
            "cycle {cycle}: a research goal must never be Blocked by the no-progress breaker"
        );
        assert_eq!(
            state.no_progress_tracker.consecutive(id),
            0,
            "cycle {cycle}: the no-action counter must reset each idle cycle (stays active)"
        );
    }
    assert_eq!(
        state.active_goals.active.len(),
        1,
        "the research goal must remain active and re-selectable"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "a research idle fault must never file an [OODA-SAFEGUARD] tracking issue"
    );
}

/// (b) A standing research goal with a `session`/`branch` `wip_ref` whose tmux
/// session is DEAD.
///
/// Before the fix `sweep_stale_assignments_with_sessions` cleared the dead
/// engineer's assignment but LEFT its session/branch refs, so the goal kept
/// reading as `has_live_in_flight_ref` and idled forever as `ResearchInFlight`.
/// After Prong 1 the sweep also drops the dead session's session/branch/engineer
/// refs, so the next NO-ACTION cycle correctly faults (the re-orient itself is the
/// agentic per-goal reasoner's job, #4453 — the breaker only records the signal).
#[test]
fn research_goal_with_dead_session_ref_is_swept_then_faults_as_signal() {
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.assigned_to = Some("dead-session".to_string());
    goal.wip_refs = vec![session_ref("dead-session"), branch_ref("feat/x")];
    assert!(
        goal.has_live_in_flight_ref(),
        "precondition: a session/branch ref reads as live until the sweep drops it"
    );
    let mut state = state_with(goal);

    // Prong 1: the engineer's tmux session is DEAD (absent from the live set). The
    // sweep clears the assignment AND drops that dead session's session/branch refs.
    let live: HashSet<String> = ["other-live-session".to_string()].into_iter().collect();
    sweep_stale_assignments_with_sessions(&mut state.active_goals, &live);
    let swept = &state.active_goals.active[0];
    assert!(
        swept.assigned_to.is_none(),
        "the dead-session assignment must be cleared by the sweep"
    );
    assert!(
        !swept.has_live_in_flight_ref(),
        "the dead session's session/branch refs must be dropped so the goal no longer reads as in-flight"
    );

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
        assert_eq!(
            report.research_idle_faults,
            vec![id.to_string()],
            "cycle {cycle}: with the dead-session refs swept, an idle research goal must FAULT"
        );
        let goal = &state.active_goals.active[0];
        assert!(
            matches!(goal.status, GoalProgress::NotStarted),
            "cycle {cycle}: status is NotStarted from the dead-assignment SWEEP (which \
             makes the goal re-dispatchable), not from the breaker — the breaker only \
             records the fault signal and never re-orients (#4453); got {:?}",
            goal.status
        );
        assert!(
            !matches!(goal.status, GoalProgress::Blocked(_)),
            "cycle {cycle}: a research goal must never be Blocked by the no-progress breaker"
        );
        assert_eq!(
            state.no_progress_tracker.consecutive(id),
            0,
            "cycle {cycle}: the no-action counter must reset each idle cycle (stays active)"
        );
    }
    assert_eq!(
        state.active_goals.active.len(),
        1,
        "the research goal must remain active and re-selectable"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "a research idle fault must never file an [OODA-SAFEGUARD] tracking issue"
    );
}

/// (c) Regression guard (round-1 finding #1 must stay intact): a standing
/// research goal whose PR ref is GENUINELY OPEN (its number is in the open-PR
/// set) must SURVIVE the per-cycle liveness reconcile untouched, so it is still
/// classified `ResearchInFlight`: `wip_refs` + `assigned_to` preserved, NOT
/// faulted, NOT re-oriented (`roll_to_new_cycle` would wipe the load-bearing
/// refs the Overseer dedup set, engineer-admission control, and completion gate
/// all depend on).
#[test]
fn genuinely_open_pr_ref_survives_liveness_reconcile_and_stays_in_flight() {
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.assigned_to = Some("engineer-42".to_string());
    goal.wip_refs = vec![pr_ref("7")]; // an OPEN, unmerged PR — live in-flight work
    let mut state = state_with(goal);

    // Prong 2: PR #7 IS in the open set → the reconcile must NOT prune it.
    let open_prs: HashSet<u32> = [7u32].into_iter().collect();
    prune_merged_pr_refs(&mut state.active_goals, &open_prs);
    assert_eq!(
        state.active_goals.active[0].wip_refs,
        vec![pr_ref("7")],
        "an OPEN PR ref must survive the liveness reconcile (round-1 finding #1 intact)"
    );
    assert!(
        state.active_goals.active[0].has_live_in_flight_ref(),
        "the surviving open PR ref keeps the goal reading as in-flight"
    );

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
            report.research_idle_faults.is_empty(),
            "cycle {cycle}: a goal holding a genuinely-open PR is in-flight progress, not an idle fault"
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: in-flight progress is neither a fault nor a benign perpetual idle"
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: in-flight progress must never fire the breaker"
        );

        let goal = &state.active_goals.active[0];
        assert_eq!(
            goal.wip_refs,
            vec![pr_ref("7")],
            "cycle {cycle}: the open PR ref must be PRESERVED (dedup/admission/merge-tracking depend on it)"
        );
        assert_eq!(
            goal.assigned_to.as_deref(),
            Some("engineer-42"),
            "cycle {cycle}: assignment must be preserved for the in-flight goal"
        );
        assert!(
            matches!(goal.status, GoalProgress::InProgress { percent: 40 }),
            "cycle {cycle}: an in-flight research goal must NOT be reset to NotStarted, got {:?}",
            goal.status
        );
        assert_eq!(
            state.no_progress_tracker.consecutive(id),
            0,
            "cycle {cycle}: the no-action counter must still reset each cycle (stays active)"
        );
    }
    assert!(
        filer.calls.borrow().is_empty(),
        "in-flight progress must never file an [OODA-SAFEGUARD] tracking issue"
    );
}

// ---------------------------------------------------------------------------
// NEW-1 Prong 2 fail-open unit tests (PR #4428): the pure `prune_merged_pr_refs`
// must only ever remove a provably-dead `pr` ref — never a possibly-live one —
// so every ambiguous case errs toward KEEPING the ref.
// ---------------------------------------------------------------------------

/// An unparseable `pr` `ref_id` is KEPT (not pruned) — a malformed-but-possibly
/// -live ref must never be dropped on a guess (would risk the round-1 F1 bug).
#[test]
fn prune_keeps_unparseable_pr_ref_and_reports_nothing() {
    let mut goal = standing_research_goal("g-unparseable");
    let bad = WipRef {
        kind: "pr".to_string(),
        ref_id: "not-a-number".to_string(),
        label: "PR ???".to_string(),
        url: None,
    };
    goal.wip_refs = vec![bad.clone()];
    let mut state = state_with(goal);

    let open: HashSet<u32> = HashSet::new();
    let pruned = prune_merged_pr_refs(&mut state.active_goals, &open);

    assert!(
        pruned.is_empty(),
        "an unparseable pr ref must not be reported as pruned"
    );
    assert_eq!(
        state.active_goals.active[0].wip_refs,
        vec![bad],
        "an unparseable pr ref_id must be KEPT (fail-open), even against an empty open set"
    );
}

/// An `Ok([])` open set (genuinely no open PRs) prunes EVERY `pr` ref — each is
/// necessarily stale.
#[test]
fn prune_drops_all_pr_refs_when_open_set_is_empty() {
    let mut goal = standing_research_goal("g-allmerged");
    goal.wip_refs = vec![pr_ref("11"), pr_ref("#22")];
    let mut state = state_with(goal);

    let open: HashSet<u32> = HashSet::new();
    let pruned = prune_merged_pr_refs(&mut state.active_goals, &open);

    assert!(
        state.active_goals.active[0].wip_refs.is_empty(),
        "with no open PRs every pr ref is stale and must be pruned"
    );
    assert_eq!(
        pruned.len(),
        2,
        "both pruned pr refs must be reported (including the '#'-prefixed one)"
    );
}

/// A non-`pr` (`issue`) ref is NEVER touched by Prong 2 — it is a durable record,
/// and is already deny-by-default in `has_live_in_flight_ref`.
#[test]
fn prune_never_touches_non_pr_refs() {
    let mut goal = standing_research_goal("g-issue");
    let issue = WipRef {
        kind: "issue".to_string(),
        ref_id: "100".to_string(),
        label: "issue #100".to_string(),
        url: None,
    };
    let branch = branch_ref("feat/z");
    goal.wip_refs = vec![issue.clone(), branch.clone()];
    let mut state = state_with(goal);

    // An empty open set would prune every `pr` ref — but there are none here.
    let open: HashSet<u32> = HashSet::new();
    let pruned = prune_merged_pr_refs(&mut state.active_goals, &open);

    assert!(pruned.is_empty(), "no pr refs present → nothing pruned");
    assert_eq!(
        state.active_goals.active[0].wip_refs,
        vec![issue, branch],
        "issue/branch refs must pass through the PR-liveness reconcile untouched"
    );
}

// ===========================================================================
// #4927 end-to-end: a self-healed standing hygiene goal is breaker-exempt
//
// Reproduction + fix of the recurring-goal-reblock incident. The live
// `articulate-repo-hygiene-backlog` goal sat on the cognitive-memory board with
// an UNMARKED description, so the driver's `!is_perpetual()` exemption never
// applied: it was re-parked and issue-filed every OODA cycle (#4927/#4930/#4934).
// Once the standing seed declares it and `reconcile_standing_markers` self-heals
// the persisted goal to perpetual, driving the breaker N+1 consecutive
// no-action cycles must NEVER block it, escalate it, or file a tracking issue —
// it is a benign perpetual idle. A companion test proves the SAME goal, left
// unmarked (pre-fix), still escalates — so the fix is exactly the standing tag.
// ===========================================================================

fn hygiene_goal_unmarked() -> ActiveGoal {
    let title = "Articulate repo-hygiene backlog";
    let id = crate::goals::goal_slug(title);
    let mut g = ActiveGoal::new(
        id,
        "Turn observations into prioritized repo-hygiene goals.",
        2,
    );
    g.status = GoalProgress::NotStarted;
    assert!(
        !g.is_perpetual(),
        "the pre-fix live goal must be unmarked (the #4927 defect)"
    );
    g
}

#[test]
fn reconciled_standing_hygiene_goal_is_exempt_from_the_no_progress_breaker() {
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let goal = hygiene_goal_unmarked();
    let id = goal.id.clone();

    // Self-heal the persisted goal via the standing seed (the #4927 fix).
    let mut board = GoalBoard::new();
    board.active.push(goal);
    let standing = crate::identity::SeedGoal::new(
        2,
        "Articulate repo-hygiene backlog",
        "Turn observations into prioritized repo-hygiene goals.",
        None,
    )
    .standing();
    let healed = crate::goal_curation::reconcile_standing_markers(&mut board, &[standing]);
    assert_eq!(
        healed.added, 1,
        "reconcile must self-heal the one matching live goal"
    );
    assert!(
        board.active[0].is_perpetual(),
        "post-reconcile the hygiene goal must read as perpetual (#4927)"
    );

    let mut state = OodaState::new(board);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    // N+1 consecutive no-action cycles — one past where a normal goal is parked.
    for cycle in 1..=(threshold + 1) {
        let report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome(&id)],
            &evidence,
            &filer,
            threshold,
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: a reconciled standing goal must not fire the breaker (#4927)"
        );
        assert!(
            report.escalated.is_empty(),
            "cycle {cycle}: a reconciled standing goal must never be escalated"
        );
        assert_eq!(
            report.perpetual_idled,
            vec![id.clone()],
            "cycle {cycle}: the idle must be recorded as a benign perpetual idle"
        );
        assert!(
            !matches!(
                state.active_goals.active[0].status,
                GoalProgress::Blocked(_)
            ),
            "cycle {cycle}: a reconciled standing goal must never be Blocked"
        );
    }

    assert!(
        filer.calls.borrow().is_empty(),
        "a reconciled standing goal must never file an [OODA-SAFEGUARD] tracking issue (#4927)"
    );
}

#[test]
fn unmarked_hygiene_goal_still_escalates_proving_the_tag_is_the_fix() {
    // Control: the identical hygiene goal, left UNMARKED (no standing seed /
    // reconcile), reproduces the pre-fix #4927 behaviour — the breaker fires,
    // the goal is escalated with the sentinel, and exactly one issue is filed.
    // This proves the exemption keys precisely on the standing tag.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let goal = hygiene_goal_unmarked();
    let id = goal.id.clone();
    let mut state = state_with(goal);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    let mut fired = false;
    for _ in 1..=(threshold + 1) {
        let report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome(&id)],
            &evidence,
            &filer,
            threshold,
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "an unmarked goal must never be treated as a perpetual idle"
        );
        if report.fired() {
            fired = true;
        }
    }
    assert!(
        fired,
        "an unmarked hygiene goal must still trip the no-progress breaker"
    );
    assert!(
        !filer.calls.borrow().is_empty(),
        "the pre-fix unmarked goal must still file exactly the escalation issue"
    );
}

#[test]
fn reverted_standing_seed_goal_re_enters_the_no_progress_breaker() {
    // #4927 rework: a standing declaration is conservatively reversible. A
    // source:seed goal marked standing, then reverted by an explicit
    // `standing = false` seed, must lose its breaker exemption and escalate
    // again exactly like an ordinary stuck goal — proving the reversal actually
    // re-arms the safety breaker.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let title = "Articulate repo-hygiene backlog";
    let id = crate::goals::goal_slug(title);
    let mut goal = ActiveGoal::new(
        id.clone(),
        "Turn observations into prioritized repo-hygiene goals.",
        2,
    )
    .with_label(crate::goal_curation::labels::SOURCE_SEED);
    goal.status = GoalProgress::NotStarted;

    let mut board = GoalBoard::new();
    board.active.push(goal);

    // 1) Declare standing -> exempt.
    let standing = crate::identity::SeedGoal::new(
        2,
        title,
        "Turn observations into prioritized repo-hygiene goals.",
        None,
    )
    .standing();
    assert_eq!(
        crate::goal_curation::reconcile_standing_markers(&mut board, &[standing]).added,
        1
    );
    assert!(board.active[0].is_perpetual());

    // 2) Revert with an explicit standing=false seed of the SAME slug -> the
    //    reconciler strips the marker it added; the goal converges again. The
    //    reversal MUST be explicit (`.non_standing()`), never a merely-omitted
    //    seed, which stays inert (#4927 three-state semantics).
    let reverted = crate::identity::SeedGoal::new(
        2,
        title,
        "Turn observations into prioritized repo-hygiene goals.",
        None,
    )
    .non_standing();
    assert!(
        reverted.authorizes_standing_reversal(),
        "sanity: an explicit standing=false seed authorizes reversal"
    );
    assert_eq!(
        crate::goal_curation::reconcile_standing_markers(&mut board, &[reverted]).removed,
        1
    );
    assert!(
        !board.active[0].is_perpetual(),
        "after reversal the goal is no longer breaker-exempt"
    );

    // 3) Drive the breaker: the reverted goal must escalate and file an issue.
    let mut state = OodaState::new(board);
    let evidence = FakeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
    };
    let filer = RecordingFiler::default();

    let mut fired = false;
    for _ in 1..=(threshold + 1) {
        let report = apply_no_progress_breaker_with_threshold(
            &mut state,
            &[no_action_outcome(&id)],
            &evidence,
            &filer,
            threshold,
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "a reverted goal must never be treated as a perpetual idle"
        );
        if report.fired() {
            fired = true;
        }
    }
    assert!(
        fired,
        "a reverted standing goal must trip the no-progress breaker again"
    );
    assert!(
        !filer.calls.borrow().is_empty(),
        "the reverted goal must file the escalation issue like any ordinary stuck goal"
    );
}
