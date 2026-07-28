//! TEST-FIRST (Step 7 TDD) — the **churn-stopping** side effects of the OODA
//! breaker terminal-quarantine rung (process_health, HIGH).
//!
//! # The churn these tests kill
//!
//! A permanently-`UNCLEAR-CRITERIA` goal is re-selected by the already-blocked
//! re-investigation pass every cycle, rides the ladder to the evidence-less
//! terminal rung, is surfaced + un-blocked, gets re-selected, and repeats —
//! filing near-identical `ooda-stuck` issues each time. The single behaviour that
//! stops the churn: once a goal trips the terminal-quarantine rung it is durably
//! marked and **never re-scheduled, re-classified, or re-escalated again**.
//!
//! # The contract (side-effecting layer)
//!
//! 1. `reinvestigate_bare_blocked_goals` **skips any quarantined goal** — the
//!    reasoner is never even consulted for it (the churn-stopper).
//! 2. When the pure ladder returns `QuarantineTerminal`, the adapter Blocks the
//!    goal with a WHY-bearing reason (never bare / never `(none)`) AND writes the
//!    durable quarantine marker through the goal board.
//! 3. The marker write is idempotent (≤ 1 marker per goal) and a quarantined goal
//!    is never re-filed — no duplicate-issue storm.
//!
//! Every dependency is an injected hermetic fake — no `gh`, no clone, no
//! subprocess. RED until the quarantine variant, the extended
//! `resolution_for_why`, the marker helpers, the `QuarantineTerminal` side-effect
//! handler, and the `is_quarantined` re-schedule exclusion all exist.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::no_progress::{
    NoProgressBreakerReport, NoProgressEngineerDispatcher, NoProgressIssueFiler,
    PreconditionHealer, reinvestigate_bare_blocked_goals,
};
use crate::error::SimardResult;
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, SURFACED_INVESTIGATION_FAILURE_LIMIT, is_bare_no_progress_block,
    is_quarantined, no_progress_blocked_reason, quarantine_marker,
};
use crate::goal_curation::no_progress_why::{
    NoProgressClass, NoProgressWhy, NoProgressWhyReasoner,
};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
use crate::ooda_loop::OodaState;

// --- fakes ------------------------------------------------------------------

/// Canned "still stuck" evidence source (classification is driven by the
/// injected reasoner; the source is taken only for API symmetry).
struct StuckEvidence;
impl EvidenceSource for StuckEvidence {
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
        Ok(true)
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        Ok(DependencyState::None)
    }
}

/// A reasoner that returns `UNCLEAR-CRITERIA` with NO evidence and counts how
/// many times it is consulted.
#[derive(Default)]
struct CountingUnclearReasoner {
    calls: AtomicUsize,
}
impl NoProgressWhyReasoner for CountingUnclearReasoner {
    fn investigate(&self, _goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(NoProgressWhy::new(NoProgressClass::UnclearCriteria, vec![]))
    }
}

/// A reasoner that MUST NOT be consulted — panics if it is. Proves the
/// quarantine exclusion short-circuits before any investigation.
struct PanicReasoner;
impl NoProgressWhyReasoner for PanicReasoner {
    fn investigate(&self, goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        panic!(
            "a quarantined goal must NEVER be re-investigated — reasoner consulted for {:?}",
            goal.id
        )
    }
}

struct NoopHealer;
impl PreconditionHealer for NoopHealer {
    fn heal(&self, _goal: &ActiveGoal, _why: &NoProgressWhy) -> Result<(), String> {
        Ok(())
    }
}

/// A dispatcher that must not be asked to spawn a fixer for a terminal goal.
#[derive(Default)]
struct CountingDispatcher {
    calls: AtomicUsize,
}
impl NoProgressEngineerDispatcher for CountingDispatcher {
    fn spawn_engineer(&self, _goal_id: &str, _task: &str) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        true
    }
}

/// Counts every issue-filing attempt so we can prove the storm is gone.
#[derive(Default)]
struct CountingFiler {
    calls: AtomicUsize,
}
impl NoProgressIssueFiler for CountingFiler {
    fn file_issue(&self, _title: &str, _body: &str) -> Option<super::no_progress::FiledIssue> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Some(super::no_progress::FiledIssue {
            number: format!("{}", 9000 + self.calls.load(Ordering::SeqCst)),
            url: None,
        })
    }
}

// --- fixtures ---------------------------------------------------------------

fn state_with(goal: ActiveGoal) -> OodaState {
    let mut board = GoalBoard::new();
    board.active.push(goal);
    OodaState::new(board)
}

fn bare_blocked(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "keep the simard identity coherent", 1);
    g.status = GoalProgress::Blocked(no_progress_blocked_reason(NO_PROGRESS_BREAKER_THRESHOLD));
    g
}

fn only_goal(state: &OodaState) -> &ActiveGoal {
    &state.active_goals.active[0]
}

#[allow(clippy::too_many_arguments)]
fn drive(
    state: &mut OodaState,
    reasoner: &dyn NoProgressWhyReasoner,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
) -> NoProgressBreakerReport {
    let evidence = StuckEvidence;
    let healer = NoopHealer;
    reinvestigate_bare_blocked_goals(
        state,
        &evidence,
        reasoner,
        &healer,
        dispatcher,
        filer,
        NO_PROGRESS_BREAKER_THRESHOLD,
    )
}

// === (1) the churn-stopper: a quarantined goal is never re-investigated ======

#[test]
fn a_quarantined_goal_is_skipped_by_reinvestigation() {
    // A goal parked BARE (so the deterministic rail WOULD normally select it)
    // that ALSO carries the durable quarantine marker must be short-circuited:
    // the reasoner is never consulted, no fixer is spawned, no issue is filed,
    // and its status is left exactly as-is.
    let id = "simard-identity-atelier-industrial-furniture-de";
    let mut goal = bare_blocked(id);
    goal.wip_refs.push(quarantine_marker());
    let before = goal.status.clone();
    let mut state = state_with(goal);

    let dispatcher = CountingDispatcher::default();
    let filer = CountingFiler::default();

    // PanicReasoner: any investigation of the quarantined goal fails the test.
    let report = drive(&mut state, &PanicReasoner, &dispatcher, &filer);

    assert_eq!(
        dispatcher.calls.load(Ordering::SeqCst),
        0,
        "a quarantined goal must never spawn a fixer"
    );
    assert_eq!(
        filer.calls.load(Ordering::SeqCst),
        0,
        "a quarantined goal must never file (or re-file) an ooda-stuck issue"
    );
    assert!(
        report.reinvestigated.is_empty() && !report.fired(),
        "a quarantined goal must not appear in any breaker action bucket"
    );
    assert_eq!(
        only_goal(&state).status,
        before,
        "a quarantined goal's status must be left untouched"
    );
    assert!(
        is_quarantined(only_goal(&state)),
        "the goal must remain quarantined"
    );
}

// === (2) at the bound, the pass QUARANTINES: marker + non-bare block =========

#[test]
fn evidenceless_stall_at_the_bound_is_quarantined_and_marked() {
    let id = "simard-identity-luxe-coastal-lighting-collectiv";
    let mut state = state_with(bare_blocked(id));

    // The goal has spent its guided retry and already accrued the bounded number
    // of evidence-less surfaced failures — the exact terminal condition.
    state.no_progress_tracker.mark_guided_retry(id);
    for _ in 0..SURFACED_INVESTIGATION_FAILURE_LIMIT {
        state.no_progress_tracker.record_surfaced_failure(id);
    }
    assert_eq!(
        state.no_progress_tracker.surfaced_failures(id),
        SURFACED_INVESTIGATION_FAILURE_LIMIT,
        "precondition: the goal is at the surfaced-failure bound"
    );

    let reasoner = CountingUnclearReasoner::default();
    let dispatcher = CountingDispatcher::default();
    let filer = CountingFiler::default();

    drive(&mut state, &reasoner, &dispatcher, &filer);

    assert_eq!(
        reasoner.calls.load(Ordering::SeqCst),
        1,
        "the goal is investigated exactly once before being quarantined"
    );
    let g = only_goal(&state);
    assert!(
        is_quarantined(g),
        "an at-bound evidence-less UNCLEAR-CRITERIA stall must be durably quarantined"
    );
    match &g.status {
        GoalProgress::Blocked(reason) => {
            assert!(
                !is_bare_no_progress_block(reason),
                "the quarantine block must carry a concrete WHY, never a bare 'needs human \
                 review': {reason}"
            );
            assert!(
                !reason.contains("evidence=[(none)]"),
                "the quarantine block must render the surfaced count as REAL evidence, never \
                 evidence=[(none)]: {reason}"
            );
        }
        other => panic!("a quarantined goal must be Blocked, got {other:?}"),
    }
    assert_eq!(
        dispatcher.calls.load(Ordering::SeqCst),
        0,
        "quarantine is terminal — it must not spawn another fixer"
    );
}

// === (3) idempotent marker + no re-file storm ===============================

#[test]
fn quarantine_is_idempotent_and_never_refiles() {
    let id = "simard-identity-artisan-heritage-textiles-studi";
    let mut state = state_with(bare_blocked(id));
    state.no_progress_tracker.mark_guided_retry(id);
    for _ in 0..SURFACED_INVESTIGATION_FAILURE_LIMIT {
        state.no_progress_tracker.record_surfaced_failure(id);
    }

    let reasoner = CountingUnclearReasoner::default();
    let dispatcher = CountingDispatcher::default();
    let filer = CountingFiler::default();

    // Pass 1: quarantine.
    drive(&mut state, &reasoner, &dispatcher, &filer);
    assert!(is_quarantined(only_goal(&state)), "pass 1 must quarantine");
    let filings_after_first = filer.calls.load(Ordering::SeqCst);
    assert!(
        filings_after_first <= 1,
        "quarantine must file AT MOST one tracking issue, got {filings_after_first}"
    );

    // Pass 2: identical inputs. The quarantined goal must be skipped entirely —
    // no second investigation, no duplicate marker, no re-file.
    drive(&mut state, &reasoner, &dispatcher, &filer);

    assert_eq!(
        reasoner.calls.load(Ordering::SeqCst),
        1,
        "a quarantined goal must NOT be re-investigated on the next cycle"
    );
    assert_eq!(
        filer.calls.load(Ordering::SeqCst),
        filings_after_first,
        "a quarantined goal must NEVER be re-filed — this is the churn that is being killed"
    );
    let markers = only_goal(&state)
        .wip_refs
        .iter()
        .filter(|w| crate::goal_curation::no_progress_breaker::is_quarantine_ref(w))
        .count();
    assert_eq!(
        markers, 1,
        "the durable quarantine marker must be written at most once (≤ 1 per goal)"
    );
}

// === (4) below the bound: NOT quarantined (retriable) ========================

#[test]
fn below_the_bound_the_goal_is_not_quarantined() {
    let id = "simard-identity-modernist-ceramic-tableware-lin";
    let mut state = state_with(bare_blocked(id));
    state.no_progress_tracker.mark_guided_retry(id);
    // One below the bound.
    for _ in 0..(SURFACED_INVESTIGATION_FAILURE_LIMIT - 1) {
        state.no_progress_tracker.record_surfaced_failure(id);
    }

    let reasoner = CountingUnclearReasoner::default();
    let dispatcher = CountingDispatcher::default();
    let filer = CountingFiler::default();

    drive(&mut state, &reasoner, &dispatcher, &filer);

    assert!(
        !is_quarantined(only_goal(&state)),
        "below the surfaced-failure bound the goal must stay retriable, never quarantined"
    );
}
