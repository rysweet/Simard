//! Unit tests for the *production* `DeterministicNoProgressReasoner`
//! (`super::no_progress`), the deterministic classifier that produces a
//! [`NoProgressWhy`] for a stalled OODA goal.
//!
//! # Regression under test (2026-07-15 live-daemon defect)
//!
//! The reasoner's terminal step used to return
//! `GENUINELY-STUCK` with `stuck_evidence(goal)` — which is **empty** for any
//! goal that never produced a tracked open issue/PR. That rendered the
//! infamous `why=GENUINELY-STUCK evidence=[(none)]` block that stranded ~12–13
//! of 20 live goals (the six `simard-identity-*` seed goals among them).
//!
//! The invariant these tests pin: **the deterministic reasoner NEVER returns an
//! evidence-less classification**. A goal with no tracked, checkable artifact is
//! classified `UNCLEAR-CRITERIA` with a concrete "no measurable done-criteria"
//! finding; a goal with open artifacts is `GENUINELY-STUCK` evidenced by those
//! artifacts; a probe error carries the concrete error as evidence.

use super::no_progress::DeterministicNoProgressReasoner;
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_why::{
    CLASS_GENUINELY_STUCK, CLASS_UNCLEAR_CRITERIA, NoProgressClass, NoProgressWhyReasoner,
};
use crate::goal_curation::{ActiveGoal, GoalProgress, WipRef};

/// A fully controllable evidence source. All completion signals default to
/// `false` (never complete), the repo is present, and there is no upstream
/// dependency — so the reasoner falls through to its terminal step unless a test
/// opts into a probe error.
struct FakeEvidence {
    repo_present: SimardResult<bool>,
    dependency: SimardResult<DependencyState>,
}

impl FakeEvidence {
    fn terminal() -> Self {
        Self {
            repo_present: Ok(true),
            dependency: Ok(DependencyState::None),
        }
    }
}

impl EvidenceSource for FakeEvidence {
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
        self.repo_present.clone()
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        self.dependency.clone()
    }
}

/// A stalled goal with **no** tracked artifact — the exact shape of the
/// `simard-identity-*` seed goals that leaked `evidence=[(none)]`.
fn artifactless_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "an aspirational goal with no measurable outcome", 1);
    g.status = GoalProgress::NotStarted;
    g
}

#[test]
fn artifactless_goal_classifies_unclear_criteria_with_non_empty_evidence() {
    // The regression: a goal that never produced a tracked issue/PR reaches the
    // terminal rung. It MUST be classified UNCLEAR-CRITERIA (its done-criteria
    // are not expressed as anything the done-gate can certify) and MUST carry
    // concrete, non-empty evidence — never `(none)`.
    let evidence = FakeEvidence::terminal();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let why = reasoner
        .investigate(&artifactless_goal(
            "simard-identity-concierge-hospitality-design",
        ))
        .expect("deterministic reasoner never errors");

    assert_eq!(
        why.class,
        NoProgressClass::UnclearCriteria,
        "an artifactless stall is a criteria problem, not a bare GENUINELY-STUCK",
    );
    assert!(
        !why.evidence.is_empty(),
        "the reasoner must NEVER return an evidence-less classification",
    );
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "the exact live-daemon defect: evidence must never render `(none)`",
    );
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("criteria") && rendered.contains("measurable"),
        "the WHY must name the missing measurable done-criteria: {rendered:?}",
    );
}

#[test]
fn open_artifact_goal_classifies_genuinely_stuck_with_that_artifact() {
    // A goal that DID produce tracked work still open is a real stall on tracked
    // work — GENUINELY-STUCK, evidenced by the open artifact (never `(none)`).
    let evidence = FakeEvidence::terminal();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let mut goal = artifactless_goal("goal-with-open-pr");
    goal.wip_refs.push(WipRef {
        kind: "pr".to_string(),
        ref_id: "4242".to_string(),
        label: "in-flight fix".to_string(),
        url: None,
    });

    let why = reasoner
        .investigate(&goal)
        .expect("deterministic reasoner never errors");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(!why.evidence.is_empty());
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("pr") && rendered.contains("#4242") && rendered.contains("OPEN"),
        "GENUINELY-STUCK must be evidenced by the open artifact: {rendered:?}",
    );
}

#[test]
fn repo_probe_error_downgrades_to_genuinely_stuck_with_the_error_as_evidence() {
    // A failed `repo_present` probe downgrades to GENUINELY-STUCK. Even for an
    // artifactless goal that downgrade MUST carry the concrete probe error, so
    // it is never evidence-less.
    let evidence = FakeEvidence {
        repo_present: Err(SimardError::VerificationFailed {
            reason: "gh api rate-limited".to_string(),
        }),
        dependency: Ok(DependencyState::None),
    };
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let why = reasoner
        .investigate(&artifactless_goal("probe-error-goal"))
        .expect("a probe error downgrades, never errors the reasoner");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(
        !why.evidence.is_empty(),
        "a probe-error downgrade must never be evidence-less",
    );
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("reasoner-error") && rendered.contains("repo_present"),
        "the errored probe must be surfaced as evidence: {rendered:?}",
    );
}

#[test]
fn dependency_probe_error_downgrades_to_genuinely_stuck_with_the_error_as_evidence() {
    let evidence = FakeEvidence {
        repo_present: Ok(true),
        dependency: Err(SimardError::VerificationFailed {
            reason: "dependency graph query failed".to_string(),
        }),
    };
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let why = reasoner
        .investigate(&artifactless_goal("dep-error-goal"))
        .expect("a probe error downgrades, never errors the reasoner");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(!why.evidence.is_empty());
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("reasoner-error") && rendered.contains("dependency_goal_state"),
        "the errored dependency probe must be surfaced as evidence: {rendered:?}",
    );
}

#[test]
fn no_terminal_classification_is_ever_evidence_less() {
    // Belt-and-braces: across the terminal shapes the reasoner can reach, the
    // rendered evidence is never the `(none)` sentinel and the class token is one
    // of the two terminal-rung tokens.
    let cases: Vec<(&str, FakeEvidence, Option<WipRef>)> = vec![
        ("artifactless", FakeEvidence::terminal(), None),
        (
            "open-issue",
            FakeEvidence::terminal(),
            Some(WipRef {
                kind: "issue".to_string(),
                ref_id: "77".to_string(),
                label: "tracking".to_string(),
                url: None,
            }),
        ),
    ];

    for (name, evidence, wip) in cases {
        let reasoner = DeterministicNoProgressReasoner::new(&evidence);
        let mut goal = artifactless_goal(name);
        if let Some(w) = wip {
            goal.wip_refs.push(w);
        }
        let why = reasoner.investigate(&goal).expect("never errors");
        assert_ne!(
            why.render_evidence(),
            "(none)",
            "[{name}] must never render `(none)`",
        );
        let token = why.class.token();
        assert!(
            token == CLASS_UNCLEAR_CRITERIA || token == CLASS_GENUINELY_STUCK,
            "[{name}] a terminal stall is one of the two terminal-rung classes, got {token}",
        );
    }
}
