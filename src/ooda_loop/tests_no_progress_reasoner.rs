//! Direct unit tests for the production [`DeterministicNoProgressReasoner`]
//! (issue #16 follow-up).
//!
//! These pin the classifier's **terminal-rung invariant**: it must never return
//! an empty-evidence [`NoProgressWhy`], because the breaker renders such a WHY as
//! a bare `evidence=[(none)]` block — the exact live-daemon defect that stranded
//! the synthetic `simard-identity-*` / coverage / parity goals (a goal with no
//! tracked PR/issue, a present repo, and no upstream dependency fell through the
//! ladder to `GENUINELY-STUCK` with an empty `stuck_evidence`).
//!
//! The fix splits the terminal rung:
//!   - open artifacts present -> `GENUINELY-STUCK` (evidence = those artifacts);
//!   - no tracked artifact    -> `UNCLEAR-CRITERIA` (evidence names the missing,
//!     unmeasurable done-criterion);
//!   - an errored auxiliary signal downgrades to `GENUINELY-STUCK` but tags the
//!     errored probe so the evidence is still non-empty.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{DependencyState, EvidenceSource};
use crate::goal_curation::no_progress_why::{NoProgressClass, NoProgressWhyReasoner};
use crate::goal_curation::{ActiveGoal, WipRef};

use super::no_progress::DeterministicNoProgressReasoner;

/// A canned evidence source. Every probe answers `stuck`-shaped (no completion,
/// repo present, no dependency) unless a field is overridden to model an error.
struct FakeEvidence {
    repo_present: SimardResult<bool>,
    dependency: SimardResult<DependencyState>,
}

impl FakeEvidence {
    /// The terminal-rung shape: nothing complete, repo on disk, no dependency.
    fn stuck() -> Self {
        Self {
            repo_present: Ok(true),
            dependency: Ok(DependencyState::None),
        }
    }
    fn with_repo_present_err() -> Self {
        Self {
            repo_present: Err(SimardError::VerificationFailed {
                reason: "gh api boom".to_string(),
            }),
            dependency: Ok(DependencyState::None),
        }
    }
    fn with_dependency_err() -> Self {
        Self {
            repo_present: Ok(true),
            dependency: Err(SimardError::VerificationFailed {
                reason: "graph query boom".to_string(),
            }),
        }
    }
    fn clone_result<T: Clone>(r: &SimardResult<T>) -> SimardResult<T> {
        match r {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(SimardError::VerificationFailed {
                reason: e.to_string(),
            }),
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
        Self::clone_result(&self.repo_present)
    }
    fn dependency_goal_state(&self, _goal: &ActiveGoal) -> SimardResult<DependencyState> {
        Self::clone_result(&self.dependency)
    }
}

fn goal(id: &str) -> ActiveGoal {
    ActiveGoal::new(id, "a synthetic identity goal", 1)
}

/// The canonical live-daemon defect goal: a `simard-identity-*` goal with no
/// tracked PR/issue, a present repo, no upstream. This is the population that was
/// parked with `evidence=[(none)]`.
#[test]
fn no_artifact_goal_is_unclear_criteria_with_concrete_evidence_never_none() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let why = reasoner
        .investigate(&goal("simard-identity-atelier-industrial-furniture-de"))
        .expect("deterministic reasoner returns Ok");

    assert_eq!(
        why.class,
        NoProgressClass::UnclearCriteria,
        "a stalled goal with no tracked artifact the done-gate can check has \
         structurally unmeasurable done-criteria -> UNCLEAR-CRITERIA, not GENUINELY-STUCK",
    );
    assert!(
        !why.evidence.is_empty(),
        "the classifier must never return empty evidence: {why:?}",
    );
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "the WHY must never render the bare `(none)` sentinel — the exact defect: {}",
        why.render_evidence(),
    );
    assert!(
        why.render_evidence().contains("done-criteria")
            && why.render_evidence().contains("unmeasurable"),
        "the evidence must name the missing, measurable criterion: {}",
        why.render_evidence(),
    );
}

/// A goal that still references open work is genuinely stuck WITH that work as
/// evidence — this remains GENUINELY-STUCK (non-empty), unchanged.
#[test]
fn goal_with_open_artifacts_stays_genuinely_stuck_with_those_artifacts() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let mut g = goal("stuck-with-open-pr");
    g.wip_refs = vec![WipRef {
        kind: "pr".to_string(),
        ref_id: "4242".to_string(),
        label: "PR #4242".to_string(),
        url: None,
    }];

    let why = reasoner.investigate(&g).expect("returns Ok");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert!(
        !why.evidence.is_empty(),
        "must carry the open artifact: {why:?}"
    );
    assert!(
        why.render_evidence().contains("pr")
            && why.render_evidence().contains("4242")
            && why.render_evidence().contains("OPEN"),
        "the open PR must be the evidence: {}",
        why.render_evidence(),
    );
}

/// A `repo_present` probe error fails closed to GENUINELY-STUCK but must still
/// carry non-empty, concrete evidence naming the errored signal.
#[test]
fn repo_presence_error_downgrades_to_genuinely_stuck_with_nonempty_evidence() {
    let evidence = FakeEvidence::with_repo_present_err();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let why = reasoner.investigate(&goal("g")).expect("returns Ok");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "downgrade must not be evidence-free"
    );
    assert!(
        why.render_evidence().contains("repo-presence"),
        "the errored probe must be named: {}",
        why.render_evidence(),
    );
}

/// A `dependency_goal_state` probe error likewise fails closed to
/// GENUINELY-STUCK with non-empty evidence naming the errored signal.
#[test]
fn dependency_state_error_downgrades_to_genuinely_stuck_with_nonempty_evidence() {
    let evidence = FakeEvidence::with_dependency_err();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let why = reasoner.investigate(&goal("g")).expect("returns Ok");

    assert_eq!(why.class, NoProgressClass::GenuinelyStuck);
    assert_ne!(why.render_evidence(), "(none)");
    assert!(
        why.render_evidence().contains("dependency-state"),
        "the errored probe must be named: {}",
        why.render_evidence(),
    );
}

// ---------------------------------------------------------------------------
// TDD (Step 7) — FAILING tests for the terminal-rung criteria-derivation fix.
//
// The secondary defect: at the terminal rung a stalled goal with NO tracked
// artifact is classified `UNCLEAR-CRITERIA` unconditionally, even when the
// goal's OWN description already expresses concrete, checkable done-criteria
// (an explicit acceptance-criteria / definition-of-done block). Such a goal is
// not criteria-unclear — its criteria are derivable — so routing it through the
// UNCLEAR-CRITERIA guided-retry → breaker path is a misclassification that
// widens the population feeding the duplicate-issue storm.
//
// Contract the fix must satisfy (via `investigate`, the observable seam):
//   * a no-artifact goal whose description carries DERIVABLE criteria proceeds
//     as `GENUINELY-STUCK` (non-empty evidence), NOT `UNCLEAR-CRITERIA`;
//   * derivation is CONSERVATIVE — a vague no-artifact goal with nothing to
//     derive still classifies `UNCLEAR-CRITERIA` (no over-triggering); and
//   * the terminal rung never emits empty evidence (the standing invariant).
//
// "Derivable" = the description contains a self-contained, checkable criteria
// section (e.g. an "Acceptance criteria:" / "Definition of Done" list with
// concrete, verifiable items) obtainable without external clarification.
// ---------------------------------------------------------------------------

/// A no-artifact goal whose DESCRIPTION spells out explicit, checkable
/// done-criteria must proceed as GENUINELY-STUCK (its criteria are derivable),
/// not be swept into UNCLEAR-CRITERIA. Today the terminal rung ignores the
/// description and returns UNCLEAR-CRITERIA → this fails until derivation lands.
#[test]
fn no_artifact_goal_with_derivable_criteria_is_genuinely_stuck_not_unclear() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    let mut g = goal("harden-supply-chain-provenance");
    g.description = "Harden supply-chain provenance.\n\n\
         Acceptance criteria:\n\
         - `cargo deny check` passes in CI\n\
         - every crate has a verified provenance attestation\n\
         - the `supply-chain/audits.toml` lists no unaudited dependencies\n"
        .to_string();

    let why = reasoner
        .investigate(&g)
        .expect("deterministic reasoner returns Ok");

    assert_eq!(
        why.class,
        NoProgressClass::GenuinelyStuck,
        "a goal whose description already expresses concrete, checkable \
         done-criteria has DERIVABLE criteria — it must proceed as \
         GENUINELY-STUCK, not be misclassified UNCLEAR-CRITERIA: {why:?}",
    );
    assert!(
        !why.evidence.is_empty(),
        "the terminal rung must never emit empty evidence: {why:?}",
    );
    assert_ne!(
        why.render_evidence(),
        "(none)",
        "a derivable-criteria goal must carry concrete evidence, never `(none)`",
    );
}

/// Conservatism guard (must PASS before and after the fix): a vague no-artifact
/// goal with nothing derivable stays UNCLEAR-CRITERIA. This pins that criteria
/// derivation does not over-trigger and mask genuine ambiguity.
#[test]
fn vague_no_artifact_goal_still_classifies_unclear_criteria() {
    let evidence = FakeEvidence::stuck();
    let reasoner = DeterministicNoProgressReasoner::new(&evidence);

    // The canonical live-daemon defect population: a vague synthetic goal with
    // no artifacts and no derivable criteria in its description.
    let why = reasoner
        .investigate(&goal("simard-identity-atelier-industrial-furniture-de"))
        .expect("returns Ok");

    assert_eq!(
        why.class,
        NoProgressClass::UnclearCriteria,
        "a vague goal with no artifacts and nothing derivable must remain \
         UNCLEAR-CRITERIA — derivation must be conservative, never over-trigger",
    );
    assert!(
        !why.evidence.is_empty(),
        "even the truly-unclear terminal rung must carry non-empty evidence",
    );
}
