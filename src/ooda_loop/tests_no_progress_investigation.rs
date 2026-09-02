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
    NO_PROGRESS_BLOCKED_PREFIX, NO_PROGRESS_BREAKER_THRESHOLD,
    SURFACED_INVESTIGATION_FAILURE_LIMIT, is_no_progress_marker, is_quarantined,
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

    fn failing(err: &str) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            result: Err(err.to_string()),
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
    fn file_issue(&self, title: &str, body: &str) -> Option<super::no_progress::FiledIssue> {
        let mut calls = self.calls.borrow_mut();
        calls.push((title.to_string(), body.to_string()));
        Some(super::no_progress::FiledIssue {
            number: format!("{}", 9000 + calls.len()),
            url: None,
        })
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

fn status_of<'a>(state: &'a OodaState, id: &str) -> &'a GoalProgress {
    &state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == id)
        .unwrap_or_else(|| panic!("goal {id} left the board"))
        .status
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

/// A STANDING/PERPETUAL but **non-research** goal (issues #2580/#2589) — a
/// CI-stewardship charter recognised by the shared `is_perpetual()` flag but NOT
/// by `is_standing_research_goal()`. This is the class that KEEPS the benign
/// perpetual-idle exemption under #4399 (idling is normal, not a fault). Kept
/// distinct from [`standing_research_goal`] so the fixture split never drifts the
/// two exemption paths together.
fn perpetual_goal(id: &str) -> ActiveGoal {
    let g = ActiveGoal::new(id, "Steward CI health. STANDING PERPETUAL goal.", 5);
    assert!(g.is_perpetual(), "fixture must read as standing/perpetual");
    assert!(
        !g.is_standing_research_goal(),
        "the benign-exemption fixture must NOT read as a standing research goal (#4399)"
    );
    g
}

/// A STANDING/PERPETUAL **research** goal (issue #4399) — standing/perpetual AND
/// marked cognition-research (`is_standing_research_goal()` holds). For this class
/// an idle cycle is a FAULT: the investigated adapter records it in
/// `research_idle_faults` (never `perpetual_idled`) as a SIGNAL, BEFORE any
/// root-cause investigation runs; re-orienting the goal is the agentic per-goal
/// reasoner's job (#4453), not the breaker's.
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
fn perpetual_goal_is_exempt_and_never_investigated_or_blocked() {
    // A standing/perpetual NON-research goal idling is NORMAL, not the livelock the
    // breaker guards. The exemption must run BEFORE investigation, so the reasoner
    // is not even consulted and the goal is never blocked. A panicking reasoner
    // proves it is never invoked for a benign perpetual idle. (Research goals take
    // the fault + re-orient path instead — see
    // `research_goal_idle_is_a_fault_via_investigated_adapter`.)
    struct PanicReasoner;
    impl NoProgressWhyReasoner for PanicReasoner {
        fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
            panic!("the reasoner must NOT run for an exempt perpetual goal")
        }
    }

    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "steward-ci-health";
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

#[test]
fn research_goal_idle_is_a_fault_signal_without_reorient_via_investigated_adapter() {
    // The #4399 rail, enforced on the investigated adapter (site L610) too, so the
    // two breaker sites cannot drift: a standing RESEARCH goal that idles is a
    // FAULT, not the benign exemption. The classifier runs BEFORE investigation
    // (a panicking reasoner proves it is never consulted), records the idle in
    // `research_idle_faults` (NEVER `perpetual_idled`), and stays fail-closed
    // (never fired, never blocked, never escalated, never a spawned engineer).
    // Issue #4453: this imperative path records the fault SIGNAL but must NOT
    // re-orient the goal — the destructive `roll_to_new_cycle` is owned solely by
    // the agentic per-goal reasoner (`drive_per_goal_cycle`). Rolling here as well
    // would double-drive the goal (the 70ab8541 idle→reset loop), so the goal's
    // status and WIP must survive the breaker unchanged.
    struct PanicReasoner;
    impl NoProgressWhyReasoner for PanicReasoner {
        fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
            panic!("the reasoner must NOT run for a research-goal idle fault")
        }
    }

    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.wip_refs.clear(); // genuinely idle: no live in-flight artifact -> a fault
    assert!(!goal.has_live_in_flight_ref());
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
        assert_eq!(
            report.research_idle_faults,
            vec![id.to_string()],
            "cycle {cycle}: a research idle must be recorded as a FAULT signal"
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: a research goal must NOT get the benign perpetual-idle exemption"
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: an idle fault is a fail-closed re-orient, not a firing"
        );
        assert!(
            report.escalated.is_empty(),
            "cycle {cycle}: a research idle must never escalate to a human"
        );
        let goal = &state.active_goals.active[0];
        assert!(
            matches!(goal.status, GoalProgress::InProgress { percent: 40 }),
            "cycle {cycle}: the imperative breaker must NOT re-orient the goal \
             (re-orient is the reasoner's job, #4453) — status must be unchanged, got {:?}",
            goal.status
        );
    }
    assert!(
        filer.calls.borrow().is_empty() && dispatcher.calls.borrow().is_empty(),
        "a research idle must never file an issue or spawn an engineer"
    );
}

#[test]
fn research_goal_with_live_pr_is_in_flight_via_investigated_adapter() {
    // Crusty finding 1 (HIGH), enforced on the investigated adapter (site L610)
    // too so the two breaker sites cannot drift: a standing RESEARCH goal holding
    // a LIVE in-flight artifact (open, unmerged PR) that produces a no-action
    // cycle is PROGRESS (ResearchInFlight), NOT an idle fault. It must NOT be
    // recorded in `research_idle_faults`, must NOT be re-oriented (wip_refs /
    // status preserved so dedup/admission/merge-tracking survive), and — like the
    // fault path — must run BEFORE investigation (a panicking reasoner proves it
    // is never consulted) and stay fail-closed (never fired/blocked/escalated).
    struct PanicReasoner;
    impl NoProgressWhyReasoner for PanicReasoner {
        fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
            panic!("the reasoner must NOT run for an in-flight research goal")
        }
    }

    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "continuously-research-and-improve-your-own-cogn-70ab8541";
    let mut goal = standing_research_goal(id);
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.assigned_to = Some("engineer-42".to_string());
    goal.wip_refs = vec![pr_ref("7")]; // an open, unmerged PR — live in-flight work
    assert!(goal.has_live_in_flight_ref());
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
            report.research_idle_faults.is_empty(),
            "cycle {cycle}: a research goal holding a live PR is in-flight progress, not a fault"
        );
        assert!(
            report.perpetual_idled.is_empty(),
            "cycle {cycle}: in-flight progress is neither a fault nor a benign perpetual idle"
        );
        assert!(
            !report.fired(),
            "cycle {cycle}: in-flight progress must never fire the breaker"
        );
        assert!(
            report.escalated.is_empty(),
            "cycle {cycle}: in-flight progress must never escalate to a human"
        );
        let goal = &state.active_goals.active[0];
        assert_eq!(
            goal.wip_refs,
            vec![pr_ref("7")],
            "cycle {cycle}: the open PR ref must be PRESERVED (dedup/admission/merge-tracking depend on it)"
        );
        assert!(
            matches!(goal.status, GoalProgress::InProgress { percent: 40 }),
            "cycle {cycle}: an in-flight research goal must NOT be reset to NotStarted, got {:?}",
            goal.status
        );
    }
    assert!(
        filer.calls.borrow().is_empty() && dispatcher.calls.borrow().is_empty(),
        "in-flight progress must never file an issue or spawn an engineer"
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

// === (h) evidence-less re-investigation is BOUNDED, then TERMINALLY QUARANTINED =
//
// Issue #16 (#4096) fixed the live defect of parking a goal with a bare
// `evidence=[(none)]` block by making the evidence-less terminal rung
// *non-terminal* — it surfaces the failure and lets the goal re-investigate.
// But an *unbounded* re-investigation is its OWN livelock: a goal whose
// done-criteria are permanently unclear (the six `simard-identity-*` codename
// goals) surfaces → resets → forever, making no shippable progress and NEVER
// reaching a human — and, worse, re-blocking + re-filing a near-identical
// `ooda-stuck` issue each cycle (the process_health churn).
//
// The fix (process_health, HIGH) bounds it with a TERMINAL QUARANTINE: once a
// goal accrues `SURFACED_INVESTIGATION_FAILURE_LIMIT` consecutive surfaced
// failures it is quarantined — Blocked WITH the re-investigation count as
// concrete evidence (never `(none)`), one deduplicated triage issue filed, and a
// durable quarantine marker written so the re-investigation pass never
// re-schedules or re-files it again. The routing consults the PRE-bump surfaced
// count, so quarantine trips one episode AFTER the counter first reaches the
// bound (each below-bound episode still surfaces + bumps the counter).

/// Drive one full stall episode (`threshold` no-action cycles) and return the
/// last cycle's report — one episode yields exactly one terminal-rung decision.
#[allow(clippy::too_many_arguments)]
fn drive_episode(
    state: &mut OodaState,
    id: &str,
    evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> super::no_progress::NoProgressBreakerReport {
    let mut last = super::no_progress::NoProgressBreakerReport::default();
    for _ in 1..=threshold {
        last = drive(
            state, id, evidence, reasoner, healer, dispatcher, filer, threshold,
        );
    }
    last
}

#[test]
fn evidenceless_reinvestigation_is_bounded_then_terminally_quarantined() {
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let limit = SURFACED_INVESTIGATION_FAILURE_LIMIT;
    let id = "simard-identity-concierge-hospitality-design";
    // A goal with NO tracked issue/PR — the exact empty-evidence shape.
    let goal = stuck_goal(id);
    let mut state = state_with(goal);
    // Spend the one guided-engineer retry up front so every stall episode goes
    // straight to the evidence-less terminal rung (isolates the surfaced-failure
    // bound from the one-shot engineer spawn).
    state.no_progress_tracker.mark_guided_retry(id);

    let evidence = FakeEvidence::stuck();
    // The independent investigation classifies UNCLEAR-CRITERIA but can attach NO
    // evidence, every single cycle — the permanently-unclear codename goal.
    let reasoner = FakeReasoner::classifying(NoProgressClass::UnclearCriteria, vec![]);
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    // Episodes up to AND INCLUDING the one that lifts the counter to the bound
    // SURFACE the failure (fail-visible, retriable) and NEVER quarantine — but
    // each bumps the persisted surfaced-failure counter. Because routing reads
    // the PRE-bump count, the counter reaches `limit` without tripping quarantine.
    for episode in 1..=limit {
        let report = drive_episode(
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
            report.investigation_errors.contains(&id.to_string()),
            "episode {episode} (below/at the counter bound) must SURFACE the failure: {report:?}"
        );
        assert!(
            report.quarantined.is_empty(),
            "episode {episode} must NOT quarantine yet (pre-bump count still below the bound): {report:?}"
        );
        assert_eq!(
            state.no_progress_tracker.surfaced_failures(id),
            episode,
            "each surfaced failure must bump the persisted consecutive counter"
        );
        assert!(
            filer.calls.borrow().is_empty(),
            "no human issue may be filed before quarantine: {:?}",
            filer.calls.borrow()
        );
        assert!(
            matches!(status_of(&state, id), GoalProgress::NotStarted),
            "a surfaced-but-not-quarantined goal stays retriable (NotStarted)"
        );
        assert!(
            !is_quarantined(
                state
                    .active_goals
                    .active
                    .iter()
                    .find(|g| g.id == id)
                    .expect("goal on board")
            ),
            "episode {episode} must not have written the quarantine marker yet"
        );
    }

    // The NEXT episode observes a PRE-bump surfaced count == limit and TERMINALLY
    // QUARANTINES the goal (the unbounded re-investigation + re-file churn stops).
    let report = drive_episode(
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
        report.quarantined.contains(&id.to_string()),
        "once the surfaced count reaches SURFACED_INVESTIGATION_FAILURE_LIMIT ({limit}) the \
         next episode must TERMINALLY QUARANTINE the goal — ending the churn: {report:?}"
    );
    assert!(
        !report.investigation_errors.contains(&id.to_string()),
        "the quarantining episode is terminal, it does not merely surface: {report:?}"
    );
    assert!(
        report.fired(),
        "a terminal quarantine is a breaker firing: {report:?}"
    );

    // The goal now carries the durable quarantine marker.
    assert!(
        is_quarantined(
            state
                .active_goals
                .active
                .iter()
                .find(|g| g.id == id)
                .expect("goal on board")
        ),
        "the quarantined goal must carry the durable quarantine marker"
    );

    // The block reason carries the re-investigation COUNT as concrete evidence —
    // never a bare `evidence=[(none)]` — and keeps the safeguard marker.
    match status_of(&state, id) {
        GoalProgress::Blocked(reason) => {
            assert!(
                !reason.contains("(none)"),
                "the quarantine must NEVER read as an evidence=[(none)] block: {reason}"
            );
            assert!(
                is_no_progress_marker(reason) && reason.starts_with(NO_PROGRESS_BLOCKED_PREFIX),
                "the quarantine must keep the [OODA-SAFEGUARD] marker: {reason}"
            );
            assert!(
                reason.contains(NoProgressClass::UnclearCriteria.token()),
                "the quarantine must name the accurate root cause class: {reason}"
            );
            assert!(
                reason.contains(&format!("{limit} consecutive evidence-less investigations")),
                "the quarantine evidence must be the re-investigation count: {reason}"
            );
        }
        other => panic!("expected a WHY-bearing Blocked quarantine, got {other:?}"),
    }

    // Exactly one human triage issue is filed, and it asks for MEASURABLE
    // done-criteria (the objective's mandate).
    let calls = filer.calls.borrow();
    assert_eq!(
        calls.len(),
        1,
        "exactly one human triage issue must be filed"
    );
    let (title, body) = &calls[0];
    assert!(
        title.contains(id) && title.contains(NoProgressClass::UnclearCriteria.token()),
        "the issue title must name the goal and its root cause: {title}"
    );
    assert!(
        body.contains("measurable") && body.contains("machine-verifiable"),
        "the issue body must ask a human to make the done-criteria measurable: {body}"
    );
    assert!(
        body.contains("CLOSED") && body.contains("MERGED"),
        "the ask must name concrete machine-checkable shapes (issue CLOSED / PR MERGED): {body}"
    );

    // No engineer was ever spawned in this test (the retry was pre-spent), and the
    // surfaced-failure counter is cleared on quarantine so the terminal state is
    // clean (the goal is henceforth skipped by the re-investigation pass anyway).
    assert!(
        dispatcher.calls.borrow().is_empty(),
        "no guided engineer is spawned once the retry is spent"
    );
    assert_eq!(
        state.no_progress_tracker.surfaced_failures(id),
        0,
        "the surfaced-failure counter is cleared once the goal is quarantined"
    );
}

#[test]
fn real_progress_resets_the_surfaced_failure_counter() {
    // A goal that surfaces an evidence-less failure but then makes REAL progress
    // must start a fresh surfaced-failure window — a transient investigation
    // hiccup must never accumulate toward a spurious escalation.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "simard-identity-coherence";
    let goal = stuck_goal(id);
    let mut state = state_with(goal);
    state.no_progress_tracker.mark_guided_retry(id);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(NoProgressClass::GenuinelyStuck, vec![]);
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    // One surfaced failure accrues.
    drive_episode(
        &mut state,
        id,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
        threshold,
    );
    assert_eq!(
        state.no_progress_tracker.surfaced_failures(id),
        1,
        "one surfaced failure must accrue"
    );

    // Real progress on the goal (a reviewer-accepted advance) resets the counter.
    state.no_progress_tracker.record_progress(id);
    assert_eq!(
        state.no_progress_tracker.surfaced_failures(id),
        0,
        "real progress must reset the surfaced-failure window"
    );
}

// === (i) UNCLEAR-CRITERIA escalation LINKS the tracking issue so the =========
//         done-criteria become measurable (the WHY this change closes)
//
// The synthetic `simard-identity-*` codename goals stalled as UNCLEAR-CRITERIA
// with the WHY `done-criteria <id> (unmeasurable: no tracked PR/issue the
// done-gate can verify)`. The breaker filed a tracking issue but ORPHANED it —
// never linked it back to the goal — so the goal's `wip_refs` stayed empty,
// `has_derivable_signal` stayed `false`, and the done-gate could never verify
// completion. This asserts the fix: escalation links the filed tracking issue
// as an `issue` wip_ref, giving the goal a derivable signal the done-gate can
// finally check — the done-criteria are now measurable.

/// Return the goal's breaker-authored tracking-issue refs (kind `issue`, label
/// prefixed `[no-progress-tracking] `).
fn tracking_issue_refs<'a>(state: &'a OodaState, id: &str) -> Vec<&'a WipRef> {
    state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == id)
        .map(|g| {
            g.wip_refs
                .iter()
                .filter(|w| {
                    w.kind.eq_ignore_ascii_case("issue")
                        && w.label.starts_with("[no-progress-tracking] ")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// True when the goal carries a tracked `pr`/`issue` wip_ref — the specific
/// signal the WHY names ("no tracked PR/issue the done-gate can verify"). Note
/// this is narrower than `has_derivable_signal`, which also counts a
/// self-affecting change; the WHY is precisely about the *tracked-artifact* gap.
fn has_tracked_pr_or_issue(state: &OodaState, id: &str) -> bool {
    state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == id)
        .map(|g| {
            g.wip_refs
                .iter()
                .any(|w| w.kind.eq_ignore_ascii_case("pr") || w.kind.eq_ignore_ascii_case("issue"))
        })
        .unwrap_or(false)
}

#[test]
fn unclear_criteria_escalation_links_tracking_issue_making_criteria_measurable() {
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "simard-identity-bursar-investment-portfolio";
    // The canonical shape: a synthetic identity goal with NO tracked artifact.
    let goal = stuck_goal(id);
    assert!(
        goal.wip_refs.is_empty(),
        "fixture must model a goal with structurally unmeasurable done-criteria"
    );
    let mut state = state_with(goal);
    assert!(
        !has_tracked_pr_or_issue(&state, id),
        "before escalation the goal has NO tracked PR/issue — the exact WHY"
    );
    // Pre-spend the one guided-engineer retry so the next stall reaches the
    // terminal Escalate rung directly.
    state.no_progress_tracker.mark_guided_retry(id);

    let evidence = FakeEvidence::stuck();
    // The production reasoner classifies a no-artifact stall UNCLEAR-CRITERIA
    // with the named unmeasurable criterion (non-empty evidence) — so the
    // terminal rung ESCALATES (files an issue), not surfaces.
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::UnclearCriteria,
        vec![Evidence::new(
            "done-criteria",
            id,
            "unmeasurable: no tracked PR/issue the done-gate can verify",
        )],
    );
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    let report = drive_episode(
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
        report.escalated.contains(&id.to_string()),
        "an UNCLEAR-CRITERIA goal past its guided retry must escalate: {report:?}"
    );
    assert_eq!(
        filer.calls.borrow().len(),
        1,
        "exactly one tracking issue is filed"
    );

    // THE fix: the filed tracking issue is LINKED back to the goal as a tracked
    // artifact, so the done-criteria are now machine-verifiable.
    let refs = tracking_issue_refs(&state, id);
    assert_eq!(
        refs.len(),
        1,
        "the filed tracking issue must be linked to the goal as an `issue` wip_ref"
    );
    assert_eq!(
        refs[0].ref_id, "9001",
        "the linked ref must carry the filed issue number: {:?}",
        refs[0]
    );

    assert!(
        has_tracked_pr_or_issue(&state, id),
        "after linking the tracking issue the goal HAS a tracked PR/issue — its \
         done-criteria are now measurable (no longer 'no tracked PR/issue')"
    );
    let goal = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == id)
        .expect("goal stays on the board");
    assert!(
        crate::goal_curation::completion_gate::has_derivable_signal(goal),
        "the linked tracking issue is a derivable signal the done-gate can check"
    );
    assert!(
        matches!(goal.status, GoalProgress::Blocked(_)),
        "the goal is Blocked pending the tracking issue's resolution"
    );
}

#[test]
fn re_escalation_is_idempotent_no_duplicate_tracking_issue() {
    // A goal already carrying its breaker tracking issue must never spawn a
    // DUPLICATE `ooda-stuck` issue on a re-stall — the link is idempotent.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "simard-identity-bursar-investment-portfolio";
    let mut state = state_with(stuck_goal(id));
    state.no_progress_tracker.mark_guided_retry(id);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::UnclearCriteria,
        vec![Evidence::new(
            "done-criteria",
            id,
            "unmeasurable: no tracked PR/issue the done-gate can verify",
        )],
    );
    let healer = RecordingHealer::ok();
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    // First escalation files + links exactly one issue.
    drive_episode(
        &mut state,
        id,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
        threshold,
    );
    assert_eq!(filer.calls.borrow().len(), 1);
    assert_eq!(tracking_issue_refs(&state, id).len(), 1);

    // Simulate an operator un-blocking the goal so it re-stalls, then escalate
    // again: the guarded escalation must NOT file a second issue nor append a
    // second link.
    if let Some(g) = state.active_goals.active.iter_mut().find(|g| g.id == id) {
        g.status = GoalProgress::NotStarted;
    }
    let report = drive_episode(
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
        report.escalated.contains(&id.to_string()),
        "the re-stall still escalates (Blocked WITH why): {report:?}"
    );
    assert_eq!(
        filer.calls.borrow().len(),
        1,
        "no DUPLICATE tracking issue may be filed for an already-tracked goal"
    );
    assert_eq!(
        tracking_issue_refs(&state, id).len(),
        1,
        "the goal keeps exactly one linked tracking issue"
    );
}

#[test]
fn heal_failure_escalation_also_links_the_tracking_issue() {
    // The MISSING-PRECONDITION heal-failure rung is the OTHER escalation branch
    // that files a tracking issue. It must ALSO link the issue back to the goal
    // (same measurability guarantee) — otherwise a permanently-unhealable
    // precondition would re-strand the goal with an orphaned issue, exactly the
    // bug this change closes for the UNCLEAR-CRITERIA path.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let id = "mirror-unclonable-repo";
    let mut goal = stuck_goal(id);
    goal.repo = Some("unclonable-repo".to_string());
    let mut state = state_with(goal);
    assert!(
        !has_tracked_pr_or_issue(&state, id),
        "fixture starts with no tracked PR/issue"
    );

    let mut evidence = FakeEvidence::stuck();
    evidence.repo_present = false;
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::MissingPrecondition,
        vec![Evidence::new("repo", "unclonable-repo", "absent")],
    );
    // Healing the precondition fails every cycle (e.g. the repo genuinely cannot
    // be cloned), so the breaker escalates WITH the clone error as evidence.
    let healer = RecordingHealer::failing("clone failed: repository not found");
    let dispatcher = RecordingDispatcher::ok();
    let filer = RecordingFiler::default();

    let report = drive_episode(
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
        report.escalated.contains(&id.to_string()),
        "a repeatedly-failing precondition heal must escalate: {report:?}"
    );
    assert_eq!(
        filer.calls.borrow().len(),
        1,
        "exactly one tracking issue is filed for the failed heal"
    );
    // THE fix, on this branch too: the filed issue is linked back to the goal.
    let refs = tracking_issue_refs(&state, id);
    assert_eq!(
        refs.len(),
        1,
        "the heal-failure escalation must LINK its tracking issue, not orphan it"
    );
    assert!(
        has_tracked_pr_or_issue(&state, id),
        "after linking, the goal has a tracked issue the done-gate can verify"
    );
    let goal = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == id)
        .expect("goal stays on the board");
    assert!(
        matches!(goal.status, GoalProgress::Blocked(_)),
        "the goal is Blocked pending the tracking issue's resolution"
    );
}
