//! TEST-FIRST (Step 7 TDD) for the **side-effecting** already-blocked
//! re-investigation pass (issue #17):
//! [`reinvestigate_bare_blocked_goals`].
//!
//! The on-transition breaker (issue #16, PR #2960) investigates WHY a goal is
//! stuck **only at the cycle it crosses the threshold**. That leaves goals that
//! were parked *bare* by an older daemon build — or on a cycle the reasoner erred
//! — stranded forever with a bare `[OODA-SAFEGUARD] … needs human review` marker
//! and never re-examined. This pass closes that gap: every cycle it scans the
//! ACTIVE board for goals in a **bare** blocked state and runs the SAME injected
//! WHY reasoner + resolution ladder over them, so no goal is ever left with a
//! bare "needs human review" — each is upgraded to a concrete WHY and, when the
//! WHY is actionable, completed / dropped / healed / deferred / handed to a
//! spawned fixer.
//!
//! The population-driven pass under test (mirrors the sibling auto-clear scan —
//! it takes NO `outcomes`, it scans board state directly):
//!
//! ```text
//! for each ACTIVE goal G with a BARE no-progress block, non-perpetual:
//!   investigate(G) ONCE via the injected reasoner
//!     Err                  -> FAIL CLOSED: leave bare, no action, retriable
//!     ALREADY-COMPLETE     -> Completed                            (never bare)
//!     OBSOLETE             -> dropped from board                   (never bare)
//!     MISSING-PRECONDITION -> heal + UN-BLOCK to NotStarted        (never bare)
//!     UPSTREAM-DEPENDENCY  -> Paused + named blocking ref          (never bare)
//!     UNCLEAR / STUCK      -> spawn ONE fixer + UN-BLOCK to NotStarted;
//!                             once that retry is spent -> Blocked WITH why
//!   dedupe (goal, class): at most ONE terminal action, across restarts
//! ```
//!
//! Every dependency (evidence, reasoner, healer, dispatcher, filer) is an
//! injected hermetic fake — no `gh`, no clone, no subprocess.
//!
//! RED until `reinvestigate_bare_blocked_goals`, the `NoProgressBreakerReport.
//! reinvestigated` field, and the tracker dedupe set exist.

use std::cell::RefCell;
use std::collections::HashMap;

use super::no_progress::{
    NoProgressBreakerReport, NoProgressEngineerDispatcher, NoProgressIssueFiler,
    PreconditionHealer, reinvestigate_bare_blocked_goals,
};
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BLOCKED_PREFIX, NO_PROGRESS_BREAKER_THRESHOLD, is_bare_no_progress_block,
    is_no_progress_marker, no_progress_blocked_reason,
};
use crate::goal_curation::no_progress_why::{
    Evidence, NoProgressClass, NoProgressWhy, NoProgressWhyReasoner,
};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_loop::OodaState;

// --- fakes ------------------------------------------------------------------

/// Canned evidence source (the pass still takes one for API symmetry with the
/// on-transition driver; classification itself is driven by the injected
/// reasoner). `dependency` is interior-mutable so a test can model an upstream
/// state.
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

/// A reasoner that returns a per-goal canned finding, falling back to a default.
/// Supports a canned error to exercise the fail-closed path.
struct FakeReasoner {
    by_goal: HashMap<String, Result<NoProgressWhy, String>>,
    default: Result<NoProgressWhy, String>,
}

impl FakeReasoner {
    fn classifying(class: NoProgressClass, evidence: Vec<Evidence>) -> Self {
        Self {
            by_goal: HashMap::new(),
            default: Ok(NoProgressWhy::new(class, evidence)),
        }
    }
    fn failing() -> Self {
        Self {
            by_goal: HashMap::new(),
            default: Err("root-cause recipe transport failed".to_string()),
        }
    }
    fn per_goal(map: HashMap<String, NoProgressWhy>) -> Self {
        Self {
            by_goal: map.into_iter().map(|(k, v)| (k, Ok(v))).collect(),
            default: Err("no canned finding for goal".to_string()),
        }
    }
}

impl NoProgressWhyReasoner for FakeReasoner {
    fn investigate(&self, goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        self.by_goal
            .get(&goal.id)
            .unwrap_or(&self.default)
            .clone()
            .map_err(|reason| SimardError::VerificationFailed { reason })
    }
}

/// A reasoner that MUST NOT be consulted — panics if it is. Proves the perpetual
/// exemption / rail excludes a goal from the population before investigation.
struct PanicReasoner;
impl NoProgressWhyReasoner for PanicReasoner {
    fn investigate(&self, goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        panic!("the reasoner must NOT be consulted for goal {:?}", goal.id)
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

/// Records escalation issue filings.
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

/// A goal already parked in the exact BARE state the incident left behind:
/// `🔒 [OODA-SAFEGUARD] … {threshold} consecutive no-action cycles; needs human review`.
fn bare_blocked_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "advance kgpacks-rs to full parity", 1);
    g.status = GoalProgress::Blocked(no_progress_blocked_reason(NO_PROGRESS_BREAKER_THRESHOLD));
    g
}

fn state_with(goal: ActiveGoal) -> OodaState {
    let mut board = GoalBoard::new();
    board.active.push(goal);
    OodaState::new(board)
}

/// Drive the re-investigation pass for one cycle.
fn drive(
    state: &mut OodaState,
    evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
) -> NoProgressBreakerReport {
    reinvestigate_bare_blocked_goals(
        state,
        evidence,
        reasoner,
        healer,
        dispatcher,
        filer,
        NO_PROGRESS_BREAKER_THRESHOLD,
    )
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

/// Assert a goal is no longer sitting with a BARE block — the core #17 invariant
/// (I1: no bare survivors). Completed / removed / NotStarted / Paused /
/// Blocked-with-why all satisfy it.
fn assert_not_bare(state: &OodaState, id: &str) {
    if let Some(g) = state.active_goals.active.iter().find(|g| g.id == id) {
        if let GoalProgress::Blocked(reason) = &g.status {
            assert!(
                !is_bare_no_progress_block(reason),
                "goal {id} must NEVER remain a bare '[OODA-SAFEGUARD] … needs human review' block; \
                 got: {reason}"
            );
        }
    }
}

// === (a) ALREADY-COMPLETE: an already-done bare goal is completed ============

#[test]
fn a_bare_goal_that_is_already_done_is_completed_not_left_blocked() {
    // The kgpacks-rs incident, as it survives in the bare-blocked population: the
    // referenced issue is CLOSED and the PR MERGED, but the goal was parked bare
    // before the reasoner shipped. Re-investigation must read the live artifacts,
    // complete the goal, and never leave it "needs human review".
    let mut goal = bare_blocked_goal("advance-kgpacks-rs-to-full-parity");
    goal.wip_refs = vec![issue_ref("16", "eval baseline"), pr_ref("40")];
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
            Evidence::new("pr", "#40", "MERGED"),
        ],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert_eq!(
        report.reinvestigated,
        vec!["advance-kgpacks-rs-to-full-parity".to_string()],
        "the bare goal must be recorded as re-investigated this cycle"
    );
    assert_eq!(
        report.marked_done,
        vec!["advance-kgpacks-rs-to-full-parity".to_string()],
        "an already-complete bare goal must be marked done"
    );
    assert!(
        matches!(
            status_of(&state, "advance-kgpacks-rs-to-full-parity"),
            GoalProgress::Completed
        ),
        "the goal must be set Completed for archival"
    );
    assert!(
        report.escalated.is_empty() && filer.calls.borrow().is_empty(),
        "an already-complete goal must NEVER be escalated to a human"
    );
    assert!(
        dispatcher.calls.borrow().is_empty() && healer.calls.borrow().is_empty(),
        "no fixer / heal for an already-complete goal"
    );
}

// === (b) UPSTREAM-DEPENDENCY: the #17 case — Paused with the upstream named ===

#[test]
fn a_bare_goal_gated_on_an_upstream_is_deferred_with_the_upstream_named() {
    // The exact fix-agent-kgpacks-rs-issue-17 evidence: #17's done-criterion is
    // gated on #16's eval baseline (#16 still OPEN). Re-investigation must replace
    // the bare marker with a concrete WHY that NAMES the real upstream dependency,
    // deferring (Paused) rather than "needs human review".
    let mut state = state_with(bare_blocked_goal("fix-agent-kgpacks-rs-issue-17"));

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::UpstreamDependency,
        vec![Evidence::new("dependency", "kgpacks-rs-issue-16", "OPEN")],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert_eq!(
        report.reinvestigated,
        vec!["fix-agent-kgpacks-rs-issue-17".to_string()]
    );
    assert_eq!(
        report.deferred,
        vec!["fix-agent-kgpacks-rs-issue-17".to_string()],
        "an upstream-gated bare goal must be deferred, not left blocked"
    );
    assert!(
        matches!(
            status_of(&state, "fix-agent-kgpacks-rs-issue-17"),
            GoalProgress::Paused
        ),
        "an upstream-gated goal is Paused (deferred), never bare Blocked"
    );
    let goal = &state.active_goals.active[0];
    assert!(
        goal.wip_refs.iter().any(|w| {
            w.kind.eq_ignore_ascii_case("dependency")
                && (w.ref_id.contains("kgpacks-rs-issue-16")
                    || w.label.contains("kgpacks-rs-issue-16"))
        }),
        "the specific blocking upstream must be recorded on the goal as the WHY, \
         got wip_refs = {:?}",
        goal.wip_refs
    );
    assert!(
        report.escalated.is_empty() && filer.calls.borrow().is_empty(),
        "deferring on a dependency must NEVER escalate to a human"
    );
    assert_not_bare(&state, "fix-agent-kgpacks-rs-issue-17");
}

// === (c) GENUINELY-STUCK first time: spawn a fixer AND un-block the goal ======

#[test]
fn a_bare_stuck_goal_spawns_a_fixer_and_is_unblocked_so_the_fix_can_run() {
    // A bare-blocked goal with an unresolved (genuinely stuck) root cause must
    // spawn ONE guided fixer engineer AND be un-blocked back to NotStarted — an
    // already-blocked goal the brain never re-selects would strand the fix. This
    // is the requirement "when the why is actionable, spawn a fixer engineer".
    let id = "fix-agent-kgpacks-rs-issue-21";
    let mut goal = bare_blocked_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert_eq!(report.reinvestigated, vec![id.to_string()]);
    assert_eq!(
        report.engineer_spawned,
        vec![id.to_string()],
        "a genuinely-stuck bare goal must spawn a fixer on first re-investigation"
    );
    assert_eq!(
        dispatcher.calls.borrow().len(),
        1,
        "exactly one guided fixer is spawned"
    );
    let (spawn_goal, spawn_task) = dispatcher.calls.borrow()[0].clone();
    assert_eq!(spawn_goal, id);
    assert!(
        spawn_task.contains("#7") || spawn_task.to_ascii_lowercase().contains("stuck"),
        "the fixer task must embed the WHY/evidence as guidance: {spawn_task:?}"
    );
    assert!(
        matches!(status_of(&state, id), GoalProgress::NotStarted),
        "a re-investigated goal handed to a fixer must be UN-BLOCKED to NotStarted \
         so the brain can re-select it and the fix can advance it"
    );
    assert!(
        filer.calls.borrow().is_empty(),
        "spawning a fixer is not a human escalation — no issue on first stall"
    );
    assert_not_bare(&state, id);
}

// === (d) GENUINELY-STUCK after the guided retry is spent: Blocked WITH why ====

#[test]
fn a_bare_stuck_goal_whose_retry_is_spent_is_blocked_with_a_concrete_why() {
    // If the goal already spent its one guided retry, re-investigation must not
    // spawn a second fixer; it escalates — but the resulting block MUST carry the
    // concrete WHY + evidence (never a bare "needs human review").
    let id = "fix-agent-kgpacks-rs-issue-22";
    let mut goal = bare_blocked_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);
    // Pre-condition: the goal already spent its one guided fixer retry.
    state.no_progress_tracker.mark_guided_retry(id);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert_eq!(report.reinvestigated, vec![id.to_string()]);
    assert_eq!(
        report.escalated,
        vec![id.to_string()],
        "a stuck goal past its guided retry must escalate"
    );
    assert!(
        dispatcher.calls.borrow().is_empty(),
        "no SECOND fixer — the guided retry is bounded to one"
    );
    assert_eq!(
        filer.calls.borrow().len(),
        1,
        "exactly one tracking issue is filed"
    );
    match status_of(&state, id) {
        GoalProgress::Blocked(reason) => {
            assert!(
                is_no_progress_marker(reason),
                "the block must keep the [OODA-SAFEGUARD] marker: {reason}"
            );
            assert!(
                reason.starts_with(NO_PROGRESS_BLOCKED_PREFIX),
                "I6: the rewritten reason must preserve the marker prefix so overseer \
                 + load-time self-heal still recognise it: {reason}"
            );
            assert!(
                reason.contains(NoProgressClass::GenuinelyStuck.token()),
                "the block reason MUST name the classification WHY: {reason}"
            );
            assert!(
                reason.contains("#7"),
                "the block reason MUST attach the evidence link: {reason}"
            );
            assert!(
                !is_bare_no_progress_block(reason),
                "the escalated block must be WHY-bearing, NEVER bare: {reason}"
            );
        }
        other => panic!("expected Blocked-with-why, got {other:?}"),
    }
}

// === (e) MISSING-PRECONDITION: heal + un-block, never a human block ==========

#[test]
fn a_bare_goal_with_a_missing_precondition_is_healed_and_unblocked() {
    let id = "fix-agent-kgpacks-rs-issue-23";
    let mut goal = bare_blocked_goal(id);
    goal.repo = Some("kgpacks-rs".to_string());
    let mut state = state_with(goal);

    let mut evidence = FakeEvidence::stuck();
    evidence.repo_present = false;
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::MissingPrecondition,
        vec![Evidence::new("repo", "kgpacks-rs", "absent")],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert_eq!(report.reinvestigated, vec![id.to_string()]);
    assert_eq!(
        healer.calls.borrow().as_slice(),
        &[id.to_string()],
        "the missing precondition must be healed (repo cloned)"
    );
    assert_eq!(report.healed, vec![id.to_string()]);
    assert!(
        matches!(status_of(&state, id), GoalProgress::NotStarted),
        "a healed re-investigated goal must be UN-BLOCKED to NotStarted for retry"
    );
    assert!(
        report.escalated.is_empty() && filer.calls.borrow().is_empty(),
        "healing a precondition must NOT escalate to a human"
    );
    assert_not_bare(&state, id);
}

// === (f) fail-closed: a reasoner error leaves the bare block untouched ========

#[test]
fn a_reasoner_error_leaves_the_goal_bare_and_retriable_taking_no_action() {
    // Fail-closed (I2): if investigation errors, the pass must take NO terminal
    // action, leave the bare marker EXACTLY as-is (so it is retried next cycle),
    // record NOTHING in the dedupe set, and surface the error.
    let id = "fix-agent-kgpacks-rs-issue-18";
    let mut goal = bare_blocked_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let original_reason = match &goal.status {
        GoalProgress::Blocked(r) => r.clone(),
        _ => unreachable!(),
    };
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::failing();
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert_eq!(
        report.investigation_errors,
        vec![id.to_string()],
        "a reasoner error must be surfaced, not swallowed"
    );
    assert!(
        report.reinvestigated.is_empty()
            && report.marked_done.is_empty()
            && report.dropped.is_empty()
            && report.escalated.is_empty()
            && report.healed.is_empty()
            && report.deferred.is_empty()
            && report.engineer_spawned.is_empty(),
        "a reasoner error must take NO terminal action (fail closed)"
    );
    match status_of(&state, id) {
        GoalProgress::Blocked(reason) => assert_eq!(
            reason, &original_reason,
            "the bare marker must be left EXACTLY unchanged on a reasoner error"
        ),
        other => panic!("the goal must stay Blocked on a reasoner error, got {other:?}"),
    }
    assert!(
        filer.calls.borrow().is_empty()
            && dispatcher.calls.borrow().is_empty()
            && healer.calls.borrow().is_empty(),
        "no side effects on a fail-closed investigation error"
    );
    assert!(
        !state
            .no_progress_tracker
            .reinvestigated(id, NoProgressClass::GenuinelyStuck),
        "a fail-closed goal must NOT be inserted into the dedupe set (so it retries)"
    );
}

// === (g) primary idempotency: the WHY-rewrite removes the goal from the pool ==

#[test]
fn a_reinvestigated_goal_is_not_processed_again_next_cycle() {
    // Once re-investigation rewrites a goal's reason to a WHY-bearing (non-bare)
    // block, the deterministic rail excludes it from the population next cycle —
    // the primary idempotency guarantee (I4). A second pass must do nothing and
    // must NOT file a duplicate issue.
    let id = "fix-agent-kgpacks-rs-issue-22";
    let mut goal = bare_blocked_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);
    state.no_progress_tracker.mark_guided_retry(id); // force the terminal escalate

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let first = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );
    assert_eq!(first.escalated, vec![id.to_string()]);
    assert_eq!(filer.calls.borrow().len(), 1);

    // Second cycle, identical inputs: the goal is now WHY-bearing (non-bare), so
    // it is not in the population and nothing happens.
    let second = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );
    assert!(
        second.reinvestigated.is_empty() && !second.fired(),
        "a goal already upgraded to a WHY-bearing block must not be re-processed"
    );
    assert_eq!(
        filer.calls.borrow().len(),
        1,
        "no duplicate tracking issue on the second cycle"
    );
    assert_not_bare(&state, id);
}

// === (h) belt idempotency: dedupe set prevents a duplicate fixer post-restart =

#[test]
fn a_goal_reappearing_bare_after_a_fixer_spawn_never_spawns_a_second_fixer() {
    // Belt-and-suspenders (I3): even if a goal reappears BARE after a fixer was
    // already spawned for it (e.g. a crash re-parked it, or the fixer failed and
    // an older code path re-blocked it bare), the persisted (goal, class) dedupe
    // set must short-circuit before any terminal action — so at most ONE fixer is
    // ever spawned per (goal, class), surviving a daemon restart.
    let id = "fix-agent-kgpacks-rs-issue-23";
    let mut goal = bare_blocked_goal(id);
    goal.wip_refs = vec![pr_ref("7")];
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = FakeReasoner::classifying(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    // Pass 1: fixer spawned, goal un-blocked, dedupe recorded.
    let first = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );
    assert_eq!(first.engineer_spawned, vec![id.to_string()]);
    assert_eq!(dispatcher.calls.borrow().len(), 1);
    assert!(
        state
            .no_progress_tracker
            .reinvestigated(id, NoProgressClass::GenuinelyStuck),
        "a spawned fixer must record the (goal, class) dedupe entry"
    );

    // Simulate a restart that re-parked the goal BARE again (round-trip the
    // tracker through serde to prove the dedupe entry persists).
    let json = serde_json::to_string(&state.no_progress_tracker).expect("serialise");
    state.no_progress_tracker = serde_json::from_str(&json).expect("deserialise");
    state.active_goals.active[0].status =
        GoalProgress::Blocked(no_progress_blocked_reason(NO_PROGRESS_BREAKER_THRESHOLD));

    // Pass 2: same class → dedupe short-circuits the terminal action.
    let _second = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );
    assert_eq!(
        dispatcher.calls.borrow().len(),
        1,
        "the (goal, class) dedupe set must prevent a SECOND fixer spawn across a restart"
    );
    assert_not_bare(&state, id);
}

// === (i) perpetual exemption: a bare-blocked perpetual goal is skipped ========

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
fn a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked() {
    // The perpetual exemption (I5) mirrors the on-transition path: a standing goal
    // is excluded from the population BEFORE investigation, so the reasoner is
    // never consulted (a panicking reasoner proves it).
    let id = "continuously-research-and-improve";
    let mut goal = perpetual_goal(id);
    goal.status = GoalProgress::Blocked(no_progress_blocked_reason(NO_PROGRESS_BREAKER_THRESHOLD));
    let mut state = state_with(goal);

    let evidence = FakeEvidence::stuck();
    let reasoner = PanicReasoner;
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert!(
        report.reinvestigated.is_empty() && !report.fired(),
        "a perpetual goal must never be re-investigated or fire the pass"
    );
    assert!(
        filer.calls.borrow().is_empty() && dispatcher.calls.borrow().is_empty(),
        "a perpetual goal must never file an issue or spawn a fixer"
    );
}

// === (j) rail: a non-bare / other-kind block is left untouched ===============

#[test]
fn an_operator_or_why_bearing_block_is_not_touched_by_the_pass() {
    // The pass keys strictly on the BARE no-progress marker. A goal blocked by an
    // operator (or already WHY-bearing) must be excluded from the population — the
    // reasoner is never consulted (panicking reasoner proves it) and the status is
    // unchanged.
    let mut operator_blocked = ActiveGoal::new("operator-hold", "paused by a human", 1);
    operator_blocked.status =
        GoalProgress::Blocked("blocked by operator: waiting on design review".to_string());
    let mut state = state_with(operator_blocked);

    let evidence = FakeEvidence::stuck();
    let reasoner = PanicReasoner;
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    assert!(
        report.reinvestigated.is_empty() && !report.fired(),
        "a non-no-progress block must never be re-investigated"
    );
    match status_of(&state, "operator-hold") {
        GoalProgress::Blocked(reason) => assert_eq!(
            reason, "blocked by operator: waiting on design review",
            "an operator block must be left byte-for-byte unchanged"
        ),
        other => panic!("expected the operator block preserved, got {other:?}"),
    }
}

// === (k) the 5-goal validation, in miniature: no bare survivors =============

#[test]
fn every_bare_goal_in_the_population_is_upgraded_in_a_single_pass() {
    // A miniature of the live deploy-#41 population: several goals parked bare with
    // distinct root causes. After ONE pass none may remain bare — each is
    // completed / deferred / fixer-spawned / blocked-with-why. This is the direct
    // proof of the deliverable: "none left as a bare 'needs human review'".
    let ids = [
        "advance-rysweet-agent-kgpacks-rs-to-full-parity",
        "fix-agent-kgpacks-rs-issue-18",
        "fix-agent-kgpacks-rs-issue-21",
        "fix-agent-kgpacks-rs-issue-22",
        "fix-agent-kgpacks-rs-issue-23",
    ];
    let mut board = GoalBoard::new();
    for id in ids {
        let mut g = bare_blocked_goal(id);
        g.wip_refs = vec![pr_ref("7")];
        board.active.push(g);
    }
    let mut state = OodaState::new(board);

    // Distinct classifications per goal, spanning the ladder.
    let mut findings: HashMap<String, NoProgressWhy> = HashMap::new();
    findings.insert(
        ids[0].to_string(),
        NoProgressWhy::new(
            NoProgressClass::AlreadyComplete,
            vec![Evidence::new("pr", "#40", "MERGED")],
        ),
    );
    findings.insert(
        ids[1].to_string(),
        NoProgressWhy::new(
            NoProgressClass::UpstreamDependency,
            vec![Evidence::new("dependency", "kgpacks-rs-issue-16", "OPEN")],
        ),
    );
    findings.insert(
        ids[2].to_string(),
        NoProgressWhy::new(
            NoProgressClass::GenuinelyStuck,
            vec![Evidence::new("pr", "#7", "OPEN")],
        ),
    );
    findings.insert(
        ids[3].to_string(),
        NoProgressWhy::new(
            NoProgressClass::Obsolete,
            vec![Evidence::new("obsolete", ids[3], "tracked elsewhere")],
        ),
    );
    findings.insert(
        ids[4].to_string(),
        NoProgressWhy::new(
            NoProgressClass::GenuinelyStuck,
            vec![Evidence::new("pr", "#7", "OPEN")],
        ),
    );

    let evidence = FakeEvidence {
        pr_merged: true,
        issue_closed: true,
        deployed: true,
        repo_present: true,
        dependency: std::sync::RwLock::new(DependencyState::None),
    };
    let reasoner = FakeReasoner::per_goal(findings);
    let (healer, dispatcher, filer) = (
        RecordingHealer::ok(),
        RecordingDispatcher::ok(),
        RecordingFiler::default(),
    );

    let report = drive(
        &mut state,
        &evidence,
        &reasoner,
        &healer,
        &dispatcher,
        &filer,
    );

    // Every one of the bare goals was re-investigated this cycle.
    let mut reinvestigated = report.reinvestigated.clone();
    reinvestigated.sort();
    let mut expected: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(
        reinvestigated, expected,
        "every bare goal in the population must be re-investigated in one pass"
    );

    // No bare survivors anywhere on the board — the deliverable invariant (I1).
    for id in ids {
        assert_not_bare(&state, id);
    }
    // And the outcomes span the ladder: one completed, one deferred, one dropped,
    // and fixers spawned for the genuinely-stuck ones.
    assert!(report.marked_done.contains(&ids[0].to_string()));
    assert!(report.deferred.contains(&ids[1].to_string()));
    assert!(report.dropped.contains(&ids[3].to_string()));
    assert!(
        report.engineer_spawned.contains(&ids[2].to_string())
            && report.engineer_spawned.contains(&ids[4].to_string()),
        "genuinely-stuck goals must get a spawned fixer"
    );
    assert!(
        report.fired(),
        "a cycle that re-investigated the whole bare population counts as a firing"
    );
}
