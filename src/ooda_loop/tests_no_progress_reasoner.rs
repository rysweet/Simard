//! Unit tests for the production [`DeterministicNoProgressReasoner`] classifier
//! (issue #16 follow-up).
//!
//! # The live-daemon defect these pin
//!
//! A goal that reached the no-progress breaker with **no tracked artifact** (no
//! PR, no issue) and no machine-resolvable cause was classified
//! `GENUINELY-STUCK` with evidence derived *only* from its `wip_refs` — an
//! **empty** list — so it was parked / spawned with
//! `why=GENUINELY-STUCK evidence=[(none)]`. That generic, evidence-free stamp
//! stranded goals such as `advance-…-to-full-parity` whose real cause was an
//! **unmeasurable** done-criterion ("full parity" is nothing the done-gate can
//! ever check).
//!
//! The reasoner now:
//! - classifies a goal with **no derivable completion signal** as
//!   `UNCLEAR-CRITERIA` (with the criteria named) rather than `GENUINELY-STUCK`,
//!   because an unmeasurable done-criterion is a *clarify-the-criteria* problem,
//!   not a *no-cause-found* one; and
//! - guarantees **every** WHY it returns carries non-empty evidence, so
//!   `evidence=[(none)]` can never again originate from the classifier.

use super::no_progress::DeterministicNoProgressReasoner;
use crate::error::SimardResult;
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_why::{NoProgressClass, NoProgressWhyReasoner};
use crate::goal_curation::{ActiveGoal, GoalProgress, WipRef};

/// Canned evidence source that never certifies completion, has the repo present,
/// and reports no upstream dependency — i.e. it exercises the reasoner's
/// fall-through (steps 5/6) deterministically.
struct FallThroughEvidence;

impl EvidenceSource for FallThroughEvidence {
    fn any_pr_merged(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(false)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        // Not consulted for a non-self-affecting goal; for a self-affecting one a
        // `false` keeps it un-certified (deploy drift outstanding).
        Ok(false)
    }
    fn repo_present(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(true)
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        Ok(DependencyState::None)
    }
}

/// A goal targeting an **external** governed repo (so it is not self-affecting)
/// with no tracked PR/issue — the exact `agent-kgpacks-rs` shape.
fn external_repo_goal_no_artifacts(id: &str, repo: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "advance the port to full parity", 1);
    g.status = GoalProgress::NotStarted;
    g.repo = Some(repo.to_string());
    g.wip_refs = Vec::new();
    g
}

fn open_pr_ref(num: &str) -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: num.to_string(),
        label: format!("PR #{num}"),
        url: None,
    }
}

#[test]
fn no_derivable_signal_classifies_unclear_criteria_with_nonempty_evidence() {
    // The kgpacks-rs parity goal: external repo, no tracked artifact, not
    // self-affecting => the done-gate can NEVER certify it. That is an
    // unmeasurable done-criterion, i.e. UNCLEAR-CRITERIA — never GENUINELY-STUCK,
    // and never an empty-evidence stamp.
    let evidence = FallThroughEvidence;
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let goal = external_repo_goal_no_artifacts(
        "advance-rysweet-agent-kgpacks-rs-to-full-parity",
        "agent-kgpacks-rs-audit",
    );

    let why = reasoner
        .investigate(&goal)
        .expect("investigate must succeed");

    assert_eq!(
        why.class,
        NoProgressClass::UnclearCriteria,
        "a goal with no derivable done-signal must be UNCLEAR-CRITERIA, not GENUINELY-STUCK"
    );
    assert!(
        !why.evidence.is_empty(),
        "UNCLEAR-CRITERIA must carry concrete evidence, never (none): {:?}",
        why.evidence
    );
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "the classifier must never render evidence=[(none)]"
    );
}

#[test]
fn genuinely_stuck_fallthrough_never_renders_empty_evidence() {
    // A self-affecting (repo-less => routes-to-Simard) goal HAS a derivable
    // signal (its deploy state), so it is GENUINELY-STUCK when nothing else
    // resolves — but with no PR/issue ref the naive evidence list would be empty.
    // The fall-through must still carry concrete, non-empty evidence.
    let evidence = FallThroughEvidence;
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let mut goal = ActiveGoal::new("self-affecting-stuck", "change Simard's own runtime", 1);
    goal.status = GoalProgress::NotStarted;

    let why = reasoner
        .investigate(&goal)
        .expect("investigate must succeed");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(
        !why.evidence.is_empty(),
        "GENUINELY-STUCK must never render empty evidence: {:?}",
        why.evidence
    );
    assert_ne!(why.render_evidence(), "(none)");
}

#[test]
fn open_pr_goal_is_genuinely_stuck_with_that_pr_as_evidence() {
    // A goal with an OPEN tracked PR HAS a derivable signal, so it stays
    // GENUINELY-STUCK (not UNCLEAR-CRITERIA) and its evidence names the open PR.
    let evidence = FallThroughEvidence;
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);
    let mut goal = external_repo_goal_no_artifacts("stuck-with-open-pr", "some-repo");
    goal.wip_refs = vec![open_pr_ref("42")];

    let why = reasoner
        .investigate(&goal)
        .expect("investigate must succeed");

    assert_eq!(
        why.class,
        NoProgressClass::GenuinelyStuck,
        "a tracked-but-unmerged PR is a derivable signal => GENUINELY-STUCK, not UNCLEAR"
    );
    let rendered = why.render_evidence();
    assert!(
        rendered.contains("#42") && rendered.contains("OPEN"),
        "evidence must name the open PR: {rendered}"
    );
}

#[test]
fn every_fallthrough_classification_carries_nonempty_evidence() {
    // Invariant: across the reasoner's terminal classifications, evidence is
    // always non-empty — the property that makes `evidence=[(none)]` unreachable
    // from the classifier.
    let evidence = FallThroughEvidence;
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let cases = [
        external_repo_goal_no_artifacts("ext-no-artifacts", "ext-repo"),
        {
            let mut g = ActiveGoal::new("self-affecting", "touch Simard runtime", 1);
            g.status = GoalProgress::NotStarted;
            g
        },
    ];

    for goal in cases {
        let why = reasoner
            .investigate(&goal)
            .expect("investigate must succeed");
        assert!(
            !why.evidence.is_empty(),
            "goal {} classified {} with EMPTY evidence — reproduces the (none) defect",
            goal.id,
            why.class.token(),
        );
    }
}
