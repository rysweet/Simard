//! TEST-FIRST (Step 7 TDD) for the *side-effecting* wiring of the agentic
//! root-cause no-progress breaker (issue #16).
//!
//! The pure policy is specified in
//! `crate::goal_curation::tests_no_progress_why`; these tests exercise the
//! investigated adapter [`apply_no_progress_breaker_investigated`] — that a
//! stuck goal driven through it over N cycles is routed down the self-resolving
//! ladder and that a human block is only ever authored **with the concrete WHY +
//! evidence attached**, never a bare "needs human review".
//!
//! The full ladder under test:
//!
//! ```text
//! threshold no-action cycles on goal G
//!         │  run the injected root-cause reasoner ONCE
//!         ▼
//!   ALREADY-COMPLETE     -> mark Completed (attach artifact evidence); NO block
//!   MISSING-PRECONDITION -> heal (e.g. clone the missing repo) + retry; NO block
//!   UPSTREAM-DEPENDENCY  -> Paused (defer) + record blocker; auto-clears; NO block
//!   UNCLEAR / STUCK      -> spawn an engineer with the WHY (bounded to once)…
//!                           …only if that is spent -> Blocked WITH why+evidence
//!   reasoner error       -> fail CLOSED: no terminal action, counter preserved
//! ```
//!
//! Every dependency (evidence, reasoner, precondition healer, engineer
//! dispatcher, issue filer) is injected as a hermetic fake — no `gh`, no clone,
//! no subprocess, no recipe run.
//!
//! RED until the investigated adapter + its seams exist.

use std::cell::RefCell;

use super::no_progress::{
    NoProgressEngineerDispatcher, NoProgressIssueFiler, PreconditionHealer,
    apply_no_progress_breaker_investigated,
};
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, is_no_progress_marker,
};
use crate::goal_curation::no_progress_why::{
    Evidence, NoProgressClass, NoProgressWhy, NoProgressWhyReasoner,
};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_loop::{ActionKind, ActionOutcome, OodaState, PlannedAction};

// --- fakes ------------------------------------------------------------------

/// Canned evidence source. `dependency` is interior-mutable so a test can model
/// an upstream that resolves between cycles (drives the auto-clear path).
struct FakeEvidence {
    pr_merged: bool,
    issue_closed: bool,
    deployed: bool,
    repo_present: bool,
    dependency: std::sync::RwLock<DependencyState>,
}

impl FakeEvidence {
    fn stuck() -> Self {
        Self {
            pr_merged: false,
            issue_closed: false,
            deployed: false,
            repo_present: true,
            dependency: std::sync::RwLock::new(DependencyState::None),
        }
    }
    fn set_dependency(&self, state: DependencyState) {
        *self.dependency.write().expect("dependency lock poisoned") = state;
    }
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
    fn repo_present(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.repo_present)
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        Ok(self
            .dependency
            .read()
            .expect("dependency lock poisoned")
            .clone())
    }
}

/// A reasoner that always returns the same canned finding (or a canned error).
struct FakeReasoner {
    result: Result<NoProgressWhy, String>,
}

impl FakeReasoner {
    fn classifying(class: NoProgressClass, evidence: Vec<Evidence>) -> Self {
        Self {
            result: Ok(NoProgressWhy::new(class, evidence)),
        }
    }
    fn failing() -> Self {
        Self {
            result: Err("root-cause recipe transport failed".to_string()),
        }
    }
}

impl NoProgressWhyReasoner for FakeReasoner {
    fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        self.result
            .clone()
            .map_err(|reason| SimardError::VerificationFailed { reason })
    }
}

/// Records precondition-heal attempts; returns a fixed outcome.
struct RecordingHealer {
    calls: RefCell<Vec<String>>,
    result: Result<(), String>,
}

impl RecordingHealer {
    fn ok() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            result: Ok(()),
        }
    }
}

impl PreconditionHealer for RecordingHealer {
    fn heal(&self, goal: &ActiveGoal, _why: &NoProgressWhy) -> Result<(), String> {
        self.calls.borrow_mut().push(goal.id.clone());
        self.result.clone()
    }
}

/// Records engineer-spawn dispatches; returns a fixed success flag.
struct RecordingDispatcher {
    calls: RefCell<Vec<(String, String)>>,
    result: bool,
}

impl RecordingDispatcher {
    fn ok() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            result: true,
        }
    }
}

impl NoProgressEngineerDispatcher for RecordingDispatcher {
    fn spawn_engineer(&self, goal_id: &str, task: &str) -> bool {
        self.calls
            .borrow_mut()
            .push((goal_id.to_string(), task.to_string()));
        self.result
    }
}

/// Records escalation issue filings (mirrors the existing suite's filer).
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

// --- fixtures ---------------------------------------------------------------

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

/// Drive the investigated adapter for one cycle.
#[allow(clippy::too_many_arguments)]
fn drive(
    state: &mut OodaState,
    goal_id: &str,
    evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> super::no_progress::NoProgressBreakerReport {
    apply_no_progress_breaker_investigated(
        state,
        &[no_action_outcome(goal_id)],
        evidence,
        reasoner,
        healer,
        dispatcher,
        filer,
        threshold,
    )
}

// === (a) ALREADY-COMPLETE: auto-complete, never block =======================

#[test]
fn already_complete_goal_is_auto_completed_with_evidence_not_blocked() {
    // The exact kgpacks-rs incident: referenced issue CLOSED + PR MERGED, but the
    // goal was never marked done, so the brain kept returning no-action. The
    // breaker must read the live artifacts, auto-complete the goal, and NEVER
    // block it "needs human review".
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("kgpacks-issue-16");
    goal.wip_refs = vec![issue_ref("16", "close E2BIG"), pr_ref("33")];
    let mut state = state_with(goal);

    let evidence = FakeEvidence {
        pr_merged: true,
        issue_closed: true,
        deployed: true,
        repo_present: true,
        dependency: std::sync::RwLock::new(DependencyState::None),
    };
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::AlreadyComplete,
        vec![
            Evidence::new("issue", "#16", "CLOSED"),
            Evidence::new("pr", "#33", "MERGED"),
        ],
    );
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    let mut report = super::no_progress::NoProgressBreakerReport::default();
    for _ in 0..threshold {
        report = drive(
            &mut state,
            "kgpacks-issue-16",
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
    }

    assert_eq!(report.marked_done, vec!["kgpacks-issue-16".to_string()]);
    assert!(
        matches!(state.active_goals.active[0].status, GoalProgress::Completed),
        "already-done goal must be set Completed for archival"
    );
    assert!(
        report.escalated.is_empty() && filer.calls.borrow().is_empty(),
        "an already-complete goal must NEVER be escalated to a human"
    );
    assert!(
        dispatcher.calls.borrow().is_empty() && healer.calls.borrow().is_empty(),
        "no engineer / heal for an already-complete goal"
    );
}

// === (b) MISSING-PRECONDITION: heal + retry, never block ====================

#[test]
fn missing_repo_triggers_the_precondition_heal_path_not_a_block() {
    // A goal targeting a repo that was never cloned must trigger the
    // clone/precondition path (self-resolve) and be retried next cycle — not
    // parked for a human.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut goal = stuck_goal("mirror-kgpacks-rs");
    goal.repo = Some("kgpacks-rs".to_string());
    let mut state = state_with(goal);

    let mut evidence = FakeEvidence::stuck();
    evidence.repo_present = false; // the governed repo is not on disk
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::MissingPrecondition,
        vec![Evidence::new("repo", "kgpacks-rs", "absent")],
    );
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    let mut report = super::no_progress::NoProgressBreakerReport::default();
    for _ in 0..threshold {
        report = drive(
            &mut state,
            "mirror-kgpacks-rs",
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
    }

    assert_eq!(
        healer.calls.borrow().as_slice(),
        &["mirror-kgpacks-rs".to_string()],
        "the missing precondition must be healed (repo cloned)"
    );
    assert_eq!(report.healed, vec!["mirror-kgpacks-rs".to_string()]);
    assert!(
        matches!(
            state.active_goals.active[0].status,
            GoalProgress::NotStarted
        ),
        "a healed goal must stay active for retry, not be blocked"
    );
    assert!(
        report.escalated.is_empty() && filer.calls.borrow().is_empty(),
        "healing a precondition must NOT escalate to a human"
    );
    assert_eq!(
        state.no_progress_tracker.consecutive("mirror-kgpacks-rs"),
        0,
        "a healed goal's no-action counter resets so it gets a fresh retry window"
    );
}

// === (c) GENUINELY-STUCK: spawn engineer, then block WITH why ===============

#[test]
fn genuinely_stuck_goal_spawns_engineer_then_blocks_with_concrete_why() {
    // A goal with no completion evidence, no missing precondition, and no
    // upstream dependency is GENUINELY-STUCK. The breaker must FIRST spawn an
    // engineer (with the WHY as guidance) rather than escalate; only after that
    // guided retry is spent and the goal is STILL stuck may it block — and then
    // the block reason must carry the classification + evidence, never a bare
    // "needs human review".
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "genuinely-stuck-goal";
    let mut goal = stuck_goal(id);
    goal.wip_refs = vec![pr_ref("7")]; // an open, unmerged PR
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    // First threshold firing: an engineer is spawned; the goal is NOT blocked.
    let mut first_fired = None;
    for cycle in 1..=threshold {
        let report = drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
        if !report.engineer_spawned.is_empty() {
            first_fired = Some(cycle);
            assert_eq!(report.engineer_spawned, vec![id.to_string()]);
        }
    }
    assert_eq!(
        first_fired,
        Some(threshold),
        "the engineer must be spawned exactly at the threshold"
    );
    assert_eq!(
        dispatcher.calls.borrow().len(),
        1,
        "exactly one guided engineer retry"
    );
    let (spawn_goal, spawn_task) = dispatcher.calls.borrow()[0].clone();
    assert_eq!(spawn_goal, id);
    assert!(
        spawn_task.contains("#7") || spawn_task.to_ascii_lowercase().contains("stuck"),
        "the engineer task must embed the WHY/evidence as guidance: {spawn_task:?}"
    );
    assert!(
        matches!(
            state.active_goals.active[0].status,
            GoalProgress::NotStarted
        ),
        "after a guided retry the goal is still active, NOT blocked"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "no human issue on first stall"
    );

    // Second threshold firing (still stuck, guided retry already spent): NOW the
    // goal is blocked, WITH the concrete why + evidence, and exactly one issue is
    // filed.
    let mut escalated_cycle = None;
    for cycle in 1..=threshold {
        let report = drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
        if !report.escalated.is_empty() {
            escalated_cycle = Some(cycle);
            assert_eq!(report.escalated, vec![id.to_string()]);
        }
    }
    assert_eq!(
        escalated_cycle,
        Some(threshold),
        "escalation must happen at the next threshold after the guided retry"
    );
    assert_eq!(
        dispatcher.calls.borrow().len(),
        1,
        "the guided retry is BOUNDED to one — no second engineer spawn"
    );
    assert_eq!(
        filer.calls.borrow().len(),
        1,
        "exactly one tracking issue filed"
    );

    match &state.active_goals.active[0].status {
        GoalProgress::Blocked(reason) => {
            assert!(
                is_no_progress_marker(reason),
                "block must carry the [OODA-SAFEGUARD] sentinel: {reason}"
            );
            assert!(
                reason.contains("GENUINELY-STUCK"),
                "block reason MUST name the classification WHY: {reason}"
            );
            assert!(
                reason.contains("#7"),
                "block reason MUST attach the evidence link: {reason}"
            );
            assert!(
                reason.contains("why="),
                "block reason must be a WHY-bearing sentinel, never a bare one: {reason}"
            );
        }
        other => panic!("expected Blocked-with-why, got {other:?}"),
    }
}

// === (d) UPSTREAM-DEPENDENCY: defer (Paused), then auto-clear ===============

#[test]
fn upstream_dependency_goal_is_paused_then_auto_cleared_when_upstream_resolves() {
    // A goal gated on another goal/PR must be DEFERRED (Paused with the specific
    // blocking ref recorded) — never "needs human review" — and must auto-clear
    // back to active once the upstream resolves.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "downstream-goal";
    let goal = stuck_goal(id);
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    evidence.set_dependency(DependencyState::Pending {
        blocking_ref: "upstream-goal (PR #33)".to_string(),
    });
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::UpstreamDependency,
        vec![Evidence::new("dependency-goal", "upstream-goal", "OPEN")],
    );
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    let mut report = super::no_progress::NoProgressBreakerReport::default();
    for _ in 0..threshold {
        report = drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
    }

    assert_eq!(report.deferred, vec![id.to_string()]);
    assert!(
        matches!(state.active_goals.active[0].status, GoalProgress::Paused),
        "an upstream-gated goal must be Paused (deferred), not Blocked"
    );
    assert!(
        state.active_goals.active[0].wip_refs.iter().any(|w| {
            w.kind.eq_ignore_ascii_case("dependency")
                && (w.label.contains("upstream-goal") || w.ref_id.contains("upstream-goal"))
        }),
        "the specific blocking upstream must be recorded on the goal as the WHY, \
         got wip_refs = {:?}",
        state.active_goals.active[0].wip_refs
    );
    assert!(
        report.escalated.is_empty() && filer.calls.borrow().is_empty(),
        "deferring on a dependency must NEVER escalate to a human"
    );

    // Upstream resolves; a subsequent pass (even with no fresh no-action outcome)
    // must auto-clear the defer back to active — no manual unblock.
    evidence.set_dependency(DependencyState::Resolved {
        blocking_ref: "upstream-goal (PR #33)".to_string(),
    });
    let cleared = apply_no_progress_breaker_investigated(
        &mut state,
        &[],
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
        threshold,
    );
    assert_eq!(
        cleared.auto_cleared,
        vec![id.to_string()],
        "a resolved upstream must auto-clear the deferred goal"
    );
    assert!(
        matches!(
            state.active_goals.active[0].status,
            GoalProgress::NotStarted
        ),
        "auto-clear must return the goal to active for re-selection"
    );
}

// === (e) reasoner error: fail CLOSED (no block, no complete) =================

#[test]
fn reasoner_error_takes_no_terminal_action_and_preserves_the_counter() {
    // Fail-closed: if the investigation itself errors, the breaker must take NO
    // terminal action — neither block nor complete — surface the error, and
    // leave the counter so the goal is retried next cycle. It must never silently
    // block or silently complete on an unknown root cause.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "investigation-errors";
    let mut goal = stuck_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::failing();
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    let mut report = super::no_progress::NoProgressBreakerReport::default();
    for _ in 0..threshold {
        report = drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
    }

    // The failure is surfaced, but no terminal action is taken.
    assert_eq!(
        report.investigation_errors,
        vec![id.to_string()],
        "a reasoner error must be surfaced, not swallowed"
    );
    assert!(
        report.marked_done.is_empty()
            && report.dropped.is_empty()
            && report.escalated.is_empty()
            && report.healed.is_empty()
            && report.deferred.is_empty()
            && report.engineer_spawned.is_empty(),
        "a reasoner error must take NO terminal action (fail closed)"
    );
    assert!(
        matches!(
            state.active_goals.active[0].status,
            GoalProgress::NotStarted
        ),
        "the goal must be neither blocked nor completed on a reasoner error"
    );
    assert!(
        filer.calls.borrow().is_empty()
            && dispatcher.calls.borrow().is_empty()
            && healer.calls.borrow().is_empty(),
        "no side effects on a fail-closed investigation error"
    );
    assert!(
        state.no_progress_tracker.consecutive(id) >= threshold,
        "the counter must be preserved (not cleared) so the goal retries next cycle"
    );
}

// === perpetual exemption is preserved by the investigated adapter ============

/// A STANDING/PERPETUAL goal (issues #2580/#2589) — recognised by the shared
/// `is_perpetual()` flag.
fn perpetual_goal(id: &str) -> ActiveGoal {
    let g = ActiveGoal::new(
        id,
        "STANDING PERPETUAL goal — never mark complete; continuously research \
         and improve your own cognition",
        5,
    );
    assert!(g.is_perpetual(), "fixture must read as standing/perpetual");
    g
}

#[test]
fn perpetual_goal_is_exempt_and_never_investigated_or_blocked() {
    // A standing/perpetual goal idling is NORMAL, not the livelock the breaker
    // guards. The exemption must run BEFORE investigation, so the reasoner is not
    // even consulted and the goal is never blocked. A panicking reasoner proves
    // it is never invoked for a perpetual idle.
    struct PanicReasoner;
    impl NoProgressWhyReasoner for PanicReasoner {
        fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
            panic!("the reasoner must NOT run for an exempt perpetual goal")
        }
    }

    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve";
    let mut goal = perpetual_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = PanicReasoner;
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    for cycle in 1..=(threshold + 1) {
        let report = drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: a perpetual idle must never fire the breaker"
        );
        assert_eq!(
            report.perpetual_idled,
            vec![id.to_string()],
            "cycle {cycle}: the idle must be recorded as a perpetual idle"
        );
        assert!(
            !matches!(
                state.active_goals.active[0].status,
                GoalProgress::Blocked(_)
            ),
            "cycle {cycle}: a perpetual goal must never be blocked"
        );
    }
    assert!(
        filer.calls.borrow().is_empty() && dispatcher.calls.borrow().is_empty(),
        "a perpetual goal must never file an issue or spawn an engineer"
    );
}

// === (f) the terminal dead-end: NEVER park a goal with evidence=[(none)] =====

#[test]
fn genuinely_stuck_with_no_evidence_surfaces_investigation_error_never_parks_none() {
    // THE production defect (verified on the live daemon 2026-07-15): 12–13 goals
    // — the six `simard-identity-*`, the coverage/coin/parity goals — never
    // produced a tracked issue/PR, so their `wip_refs` are empty and the
    // deterministic reasoner's `stuck_evidence(goal)` is `[]`. Past the guided
    // retry the terminal ELSE case then authored
    //   `[OODA-SAFEGUARD] … why=GENUINELY-STUCK evidence=[(none)]`
    // — a generic, evidence-free stamp. That dead-end is exactly what this change
    // replaces: an independent root-cause investigation must produce either an
    // evidence-backed WHY (see `..._blocks_with_concrete_why`) OR, when it
    // genuinely cannot produce ANY evidence, a SURFACED failure
    // (`investigation_errors`) — never a silent `evidence=[(none)]` block.
    //
    // Here the injected investigation returns GENUINELY-STUCK with EMPTY
    // evidence (the "investigation produced nothing" outcome). The breaker must:
    //   * NOT set `Blocked` with an `evidence=[(none)]` reason,
    //   * NOT count the goal as `escalated`,
    //   * surface the goal in `investigation_errors` (fail-visible),
    //   * leave the goal un-blocked and retriable (fail closed).
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "simard-identity-coherence";
    // A goal with NO tracked issue/PR — the exact shape whose `stuck_evidence`
    // is empty on the live daemon.
    let goal = stuck_goal(id);
    assert!(
        goal.wip_refs.is_empty(),
        "fixture must model a goal with no tracked artifacts (empty evidence)"
    );
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    // The independent investigation classifies GENUINELY-STUCK but can attach NO
    // evidence — the case that used to leak `evidence=[(none)]`.
    let reasoner = FakeReasoner::classifying(NoProgressClass::GenuinelyStuck, vec![]);
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    // First stall spends the one guided-engineer retry (the independent agentic
    // investigation dispatch); the goal is NOT blocked yet.
    for _ in 1..=threshold {
        drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
    }
    assert_eq!(
        dispatcher.calls.borrow().len(),
        1,
        "the first stall must dispatch exactly one guided investigation"
    );
    assert!(
        matches!(
            state.active_goals.active[0].status,
            GoalProgress::NotStarted
        ),
        "after the guided retry the goal is still active, not blocked"
    );

    // Second stall: the guided retry is spent and the investigation STILL yields
    // no evidence. The old code escalated here with `evidence=[(none)]`; the new
    // terminal path must instead SURFACE the failure and take no bare action.
    let mut last = super::no_progress::NoProgressBreakerReport::default();
    for _ in 1..=threshold {
        last = drive(
            &mut state,
            id,
            &evidence,
            &reasoner,
            &healer,
            &dispatcher,
            &filer,
            threshold,
        );
    }

    assert!(
        last.investigation_errors.contains(&id.to_string()),
        "an evidence-less terminal outcome must be SURFACED as an investigation \
         error, not stamped as a bare block: {last:?}"
    );
    assert!(
        !last.escalated.contains(&id.to_string()),
        "a goal must NEVER be escalated/blocked with empty evidence: {last:?}"
    );
    assert!(
        dispatcher.calls.borrow().len() == 1,
        "the guided retry stays bounded to one — no second engineer spawn"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "no tracking issue may be filed for an evidence-free terminal outcome"
    );

    // The headline invariant: the goal is never parked with `evidence=[(none)]`.
    match &state.active_goals.active[0].status {
        GoalProgress::Blocked(reason) => {
            assert!(
                !reason.contains("(none)"),
                "a goal must NEVER be parked with an evidence=[(none)] block: {reason}"
            );
        }
        // NotStarted (fail-closed, retriable) is the expected end state.
        GoalProgress::NotStarted => {}
        other => panic!("expected a fail-closed retriable state, got {other:?}"),
    }
}

// === DeterministicNoProgressReasoner: the production classifier itself ========
//
// The suite above injects a `FakeReasoner`; these tests pin the *real* reasoner
// (`crate::ooda_loop::no_progress::DeterministicNoProgressReasoner`), which is
// what actually classifies a live stall. The defect they close (issue #16):
// its terminal rung returned `GENUINELY-STUCK` even for a goal with **no**
// checkable artifacts, yielding an incoherent `evidence=[(none)]` WHY that
// violates the class contract and mis-routes an unclear-criteria goal.

use super::no_progress::DeterministicNoProgressReasoner;

/// An evidence source that is "stuck" on every certifying probe but errors on a
/// chosen investigation probe, to drive the reasoner's probe-error downgrade.
struct ErroringEvidence {
    fail_repo_present: bool,
    fail_dependency: bool,
}

impl EvidenceSource for ErroringEvidence {
    fn any_pr_merged(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn repo_present(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        if self.fail_repo_present {
            Err(SimardError::VerificationFailed {
                reason: "gh repo view timed out".to_string(),
            })
        } else {
            Ok(true)
        }
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        if self.fail_dependency {
            Err(SimardError::VerificationFailed {
                reason: "goal-board read failed".to_string(),
            })
        } else {
            Ok(DependencyState::None)
        }
    }
}

/// A broadly-scoped/exploratory goal with NO tracked PR/issue — nothing the
/// done-gate can ever check. This is the exact shape of the identity-cartographer
/// goal that stalled: its true root cause is UNCLEAR-CRITERIA, not a mysterious
/// GENUINELY-STUCK, and it must never be classified with empty evidence.
#[test]
fn deterministic_reasoner_no_checkable_artifacts_is_unclear_criteria_with_evidence() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let goal = stuck_goal("identity-cartographer-data-storytelling");
    assert!(
        goal.wip_refs.is_empty(),
        "fixture must have no checkable artifacts"
    );

    let why = reasoner.investigate(&goal).expect("reasoner returns Ok");

    assert_eq!(
        why.class,
        NoProgressClass::UnclearCriteria,
        "a stall with nothing the done-gate can check is UNCLEAR-CRITERIA, not GENUINELY-STUCK",
    );
    assert!(
        !why.evidence.is_empty(),
        "the WHY must carry evidence naming the absent criteria",
    );
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "the reasoner must NEVER emit evidence=[(none)] — the live-daemon defect (issue #16)",
    );
}

/// A stall that still has open, checkable artifacts (an open PR/issue) IS
/// genuinely stuck — a human can act on those live artifacts, which become the
/// WHY's evidence.
#[test]
fn deterministic_reasoner_open_artifacts_is_genuinely_stuck_with_those_artifacts() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let mut goal = stuck_goal("stuck-with-open-pr");
    goal.wip_refs = vec![pr_ref("42"), issue_ref("7", "tracking")];

    let why = reasoner.investigate(&goal).expect("reasoner returns Ok");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(
        !why.evidence.is_empty(),
        "open artifacts must be carried as evidence"
    );
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("pr #42 (OPEN)") && rendered.contains("issue #7 (OPEN)"),
        "the open artifacts must be the evidence: {rendered}",
    );
}

/// When an investigation probe itself errors, the reasoner keeps GENUINELY-STUCK
/// (a real, if transient, machine cause) but must still attach the failing probe
/// as evidence — never an empty `evidence=[(none)]`.
#[test]
fn deterministic_reasoner_probe_error_is_stuck_with_probe_evidence_never_empty() {
    for (fail_repo_present, fail_dependency, probe) in [
        (true, false, "repo_present"),
        (false, true, "dependency_goal_state"),
    ] {
        let evidence = ErroringEvidence {
            fail_repo_present,
            fail_dependency,
        };
        let reasoner = DeterministicNoProgressReasoner::new(&evidence);
        // No artifacts, so the ONLY thing that keeps evidence non-empty is the
        // probe-error record itself.
        let goal = stuck_goal("probe-error-goal");

        let why = reasoner
            .investigate(&goal)
            .expect("probe error downgrades, not Err");

        assert_eq!(
            why.class,
            NoProgressClass::GenuinelyStuck,
            "a probe error is a genuine machine cause, not unclear criteria",
        );
        let rendered = why.render_evidence();
        assert_ne!(
            rendered, "(none)",
            "probe-error WHY must never be evidence-less"
        );
        assert!(
            rendered.contains(probe) && rendered.contains("errored"),
            "the failing probe must be recorded as evidence: {rendered}",
        );
    }
}

/// The headline invariant across every terminal-classification shape: the
/// production reasoner must NEVER return a stall class (GENUINELY-STUCK /
/// UNCLEAR-CRITERIA) with empty evidence.
#[test]
fn deterministic_reasoner_never_emits_an_empty_evidence_stall_class() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    for wip in [
        vec![],
        vec![pr_ref("1")],
        vec![issue_ref("2", "t")],
        vec![pr_ref("3"), issue_ref("4", "t")],
    ] {
        let mut goal = stuck_goal("invariant-goal");
        goal.wip_refs = wip;
        let why = reasoner.investigate(&goal).expect("reasoner returns Ok");
        if matches!(
            why.class,
            NoProgressClass::GenuinelyStuck | NoProgressClass::UnclearCriteria
        ) {
            assert!(
                !why.evidence.is_empty(),
                "stall class {} must carry evidence, got (none) for wip_refs {:?}",
                why.class.token(),
                goal.wip_refs,
            );
        }
    }
}
