//! Unit tests for the production [`DeterministicNoProgressReasoner`].
//!
//! The live-daemon defect (2026-07-15): a stalled goal with **no tracked
//! PR/issue** (the `simard-identity-*` shape) fell through the reasoner's ladder
//! to `GENUINELY-STUCK`, and its `stuck_evidence` was empty — so the daemon
//! authored a `why=GENUINELY-STUCK evidence=[(none)]` diagnosis, a content-free
//! "stuck" that gave the guided engineer nothing to act on. Such a goal is not
//! genuinely stuck: its done-criteria are not expressed as anything the done-gate
//! can check — `UNCLEAR-CRITERIA`, a class the reasoner previously never emitted.
//!
//! These tests pin the fix: every finding the reasoner emits carries concrete
//! evidence (`render_evidence()` is never the `(none)` sentinel), and the
//! no-checkable-artifact shape classifies `UNCLEAR-CRITERIA` — routing to the
//! guided engineer to make the criteria measurable — rather than an evidence-less
//! `GENUINELY-STUCK`.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_why::{NoProgressClass, NoProgressWhyReasoner};
use crate::goal_curation::{ActiveGoal, GoalProgress, WipRef};
use crate::ooda_loop::no_progress::DeterministicNoProgressReasoner;

/// Canned evidence source. Each auxiliary probe can be forced to `Err` to
/// exercise the fail-closed downgrade paths.
struct FakeEvidence {
    pr_merged: bool,
    issue_closed: bool,
    deployed: bool,
    repo_present: SimardResult<bool>,
    dependency: SimardResult<DependencyState>,
}

impl FakeEvidence {
    /// Nothing merged/closed/deployed; repo present; no dependency — the shape
    /// that reaches the reasoner's step-5 fallthrough.
    fn stuck() -> Self {
        Self {
            pr_merged: false,
            issue_closed: false,
            deployed: false,
            repo_present: Ok(true),
            dependency: Ok(DependencyState::None),
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
        self.repo_present
            .as_ref()
            .copied()
            .map_err(|e| SimardError::VerificationFailed {
                reason: e.to_string(),
            })
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        self.dependency
            .as_ref()
            .cloned()
            .map_err(|e| SimardError::VerificationFailed {
                reason: e.to_string(),
            })
    }
}

fn err(reason: &str) -> SimardError {
    SimardError::VerificationFailed {
        reason: reason.to_string(),
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

/// A stalled goal targeting the daemon's own repo (`repo = None`) with no
/// tracked artifacts — the exact `simard-identity-*` shape.
fn bare_stuck_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "make the identity surface measurable", 1);
    g.status = GoalProgress::NotStarted;
    assert!(g.wip_refs.is_empty(), "fixture must have no wip_refs");
    g
}

// === the core fix ===========================================================

#[test]
fn stall_with_no_checkable_artifact_is_unclear_criteria_with_evidence() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let goal = bare_stuck_goal("simard-identity-gastronome-culinary-menu-event");

    let why = reasoner.investigate(&goal).expect("investigate");

    // The no-checkable-artifact shape is UNCLEAR-CRITERIA, not GENUINELY-STUCK…
    assert_eq!(
        why.class,
        NoProgressClass::UnclearCriteria,
        "a goal with no tracked PR/issue has done-criteria the gate can't check",
    );
    // …and it is NEVER evidence-less — the exact `evidence=[(none)]` defect.
    assert!(
        !why.evidence.is_empty(),
        "UNCLEAR-CRITERIA must carry evidence naming the gap",
    );
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "the reasoner must never emit evidence=[(none)]",
    );
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("done-criteria") && rendered.contains("not measurable"),
        "evidence must name the unmeasurable-criteria gap, got: {rendered}",
    );
}

#[test]
fn stall_with_open_pr_is_genuinely_stuck_carrying_that_artifact() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let mut goal = bare_stuck_goal("goal-with-open-pr");
    goal.wip_refs = vec![pr_ref("7")];

    let why = reasoner.investigate(&goal).expect("investigate");

    // A goal with an open, unmerged PR the gate examined but couldn't certify is
    // genuinely stuck — and the open artifact is the evidence.
    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(!why.evidence.is_empty());
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("pr") && rendered.contains("#7") && rendered.contains("OPEN"),
        "GENUINELY-STUCK evidence must reference the open PR, got: {rendered}",
    );
}

// === fail-closed downgrades stay evidence-bearing ===========================

#[test]
fn repo_present_error_downgrades_to_genuinely_stuck_with_error_evidence() {
    let mut evidence = FakeEvidence::stuck();
    evidence.repo_present = Err(err("gh api boom"));
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let goal = bare_stuck_goal("repo-probe-errored");

    let why = reasoner.investigate(&goal).expect("investigate");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert_ne!(why.render_evidence(), "(none)");
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("investigation-error") && rendered.contains("repo_present"),
        "an errored repo probe must attach the concrete error as evidence, got: {rendered}",
    );
}

#[test]
fn dependency_error_downgrades_to_genuinely_stuck_with_error_evidence() {
    let mut evidence = FakeEvidence::stuck();
    evidence.dependency = Err(err("dependency lookup boom"));
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let goal = bare_stuck_goal("dep-probe-errored");

    let why = reasoner.investigate(&goal).expect("investigate");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert_ne!(why.render_evidence(), "(none)");
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("investigation-error") && rendered.contains("dependency_goal_state"),
        "an errored dependency probe must attach the concrete error, got: {rendered}",
    );
}

// === guard: the reasoner can NEVER emit evidence=[(none)] ====================

#[test]
fn reasoner_never_emits_evidence_none_across_representative_shapes() {
    let bare = bare_stuck_goal("bare");
    let mut with_pr = bare_stuck_goal("with-pr");
    with_pr.wip_refs = vec![pr_ref("42")];

    let cases: Vec<(&str, FakeEvidence, &ActiveGoal)> = vec![
        ("bare/stuck", FakeEvidence::stuck(), &bare),
        ("with-open-pr", FakeEvidence::stuck(), &with_pr),
        (
            "repo-probe-error",
            FakeEvidence {
                repo_present: Err(err("x")),
                ..FakeEvidence::stuck()
            },
            &bare,
        ),
        (
            "dep-probe-error",
            FakeEvidence {
                dependency: Err(err("x")),
                ..FakeEvidence::stuck()
            },
            &bare,
        ),
    ];

    for (name, evidence, goal) in cases {
        let reasoner = DeterministicNoProgressReasoner::new(&evidence);
        let why = reasoner.investigate(goal).expect("investigate");
        assert_ne!(
            why.render_evidence(),
            "(none)",
            "case {name}: reasoner produced an evidence-less finding",
        );
        assert!(
            !why.evidence.is_empty(),
            "case {name}: reasoner produced empty evidence",
        );
    }
}
