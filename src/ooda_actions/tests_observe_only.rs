//! Simard #3125 — tests for the OBSERVE-ONLY Act phase (L1 cognition branch).
//!
//! When the active identity's posture is read-only, the Act phase must run an
//! observe-only branch instead of the engineer-dispatching one. That branch:
//!   * consults an agentic [`ObserveOnlyBrain`] over the identity's TARGET repo
//!     set (what to observe / what goals to propose is a reasoner decision —
//!     kept agentic, with NO caller-imposed wall-clock timeout),
//!   * records the returned observations, and appends the proposed goals to the
//!     board scoped to the identity's targets (`repo = Some(target)`), and
//!   * NEVER calls `dispatch_spawn_engineer` and NEVER writes to a target repo
//!     (zero write-bearing dispatch — don't burn AI credits on work the
//!     guardrail would block).
//!
//! Fail-closed contract (no fallbacks / no silent degradation):
//!   * a proposal whose repo is absent or outside `targets` is a hard error
//!     (never silently re-scoped to `rysweet/Simard`), and
//!   * a failing observe brain surfaces its error — it must NOT fall back to
//!     the engineer-dispatching Act phase.
//!
//! Contract encoded here (implementation must provide, in
//! `src/ooda_actions/observe_only.rs`):
//!   pub trait ObserveOnlyBrain: Send + Sync {
//!       fn observe(&self, targets: &[String]) -> SimardResult<ObserveOutcome>;
//!   }
//!   pub struct ObserveOutcome {
//!       pub observations: Vec<String>,
//!       pub proposals: Vec<crate::identity::SeedGoal>,
//!   }
//!   pub(crate) fn act_observe_only(
//!       brain: &dyn ObserveOnlyBrain,
//!       targets: &[String],
//!       state: &mut OodaState,
//!   ) -> SimardResult<usize>;   // returns the number of goals proposed

use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::GoalBoard;
use crate::identity::SeedGoal;
use crate::ooda_actions::observe_only::{ObserveOnlyBrain, ObserveOutcome, act_observe_only};
use crate::ooda_loop::OodaState;

/// A configurable observe-only brain that also records the target set it was
/// asked to reason over, so tests can assert scope (AC4).
struct MockObserveBrain {
    result: Mutex<Option<SimardResult<ObserveOutcome>>>,
    seen_targets: Mutex<Option<Vec<String>>>,
}

impl MockObserveBrain {
    fn ok(observations: Vec<&str>, proposals: Vec<SeedGoal>) -> Self {
        Self {
            result: Mutex::new(Some(Ok(ObserveOutcome {
                observations: observations.into_iter().map(String::from).collect(),
                proposals,
            }))),
            seen_targets: Mutex::new(None),
        }
    }

    fn err(msg: &str) -> Self {
        Self {
            result: Mutex::new(Some(Err(SimardError::BridgeTransportError {
                bridge: "observe-brain".to_string(),
                reason: msg.to_string(),
            }))),
            seen_targets: Mutex::new(None),
        }
    }
}

impl ObserveOnlyBrain for MockObserveBrain {
    fn observe(&self, targets: &[String]) -> SimardResult<ObserveOutcome> {
        *self.seen_targets.lock().unwrap() = Some(targets.to_vec());
        self.result
            .lock()
            .unwrap()
            .take()
            .expect("observe called exactly once per test")
    }
}

fn seed_goal(priority: u32, title: &str, repo: Option<&str>) -> SeedGoal {
    SeedGoal {
        priority,
        title: title.to_string(),
        description: format!("OBSERVE ONLY: {title}"),
        repo: repo.map(str::to_string),
    }
}

fn empty_state() -> OodaState {
    OodaState::new(GoalBoard::new())
}

// ── AC4: observations + proposals are scoped to the identity's targets ───────

#[test]
fn observe_only_proposes_target_scoped_goals() {
    let targets = vec!["hyenas/repo-a".to_string(), "hyenas/repo-b".to_string()];
    let brain = MockObserveBrain::ok(
        vec!["repo-a has no CODEOWNERS", "repo-b has stale branches"],
        vec![
            seed_goal(80, "Add CODEOWNERS", Some("hyenas/repo-a")),
            seed_goal(70, "Prune stale branches", Some("hyenas/repo-b")),
        ],
    );
    let mut state = empty_state();

    let proposed = act_observe_only(&brain, &targets, &mut state).unwrap();

    assert_eq!(proposed, 2, "both proposed goals should be recorded");
    assert_eq!(state.active_goals.active.len(), 2);

    // The brain reasoned over exactly the identity's target set.
    assert_eq!(
        brain.seen_targets.lock().unwrap().as_ref().unwrap(),
        &targets
    );

    // Every proposed goal is scoped to a target repo — never rysweet/Simard.
    let repos: Vec<&str> = state
        .active_goals
        .active
        .iter()
        .filter_map(|g| g.repo.as_deref())
        .collect();
    assert!(repos.contains(&"hyenas/repo-a"));
    assert!(repos.contains(&"hyenas/repo-b"));
    assert!(
        state
            .active_goals
            .active
            .iter()
            .all(|g| g.repo.as_deref() != Some("Simard"))
    );
}

// ── AC3: the observe-only branch NEVER dispatches an engineer ────────────────

#[test]
fn observe_only_never_assigns_an_engineer() {
    let targets = vec!["hyenas/repo-a".to_string()];
    let brain = MockObserveBrain::ok(
        vec!["repo-a missing LICENSE"],
        vec![seed_goal(90, "Add LICENSE", Some("hyenas/repo-a"))],
    );
    let mut state = empty_state();

    act_observe_only(&brain, &targets, &mut state).unwrap();

    // Zero write-bearing dispatch: proposed goals are parked (unassigned),
    // never handed to a subordinate engineer.
    assert!(
        state
            .active_goals
            .active
            .iter()
            .all(|g| g.assigned_to.is_none()),
        "observe-only must not assign any goal to an engineer"
    );
}

// ── Fail-closed: a proposal escaping the target scope is a hard error ─────────

#[test]
fn observe_only_fails_closed_on_out_of_targets_proposal() {
    let targets = vec!["hyenas/repo-a".to_string()];
    let brain = MockObserveBrain::ok(
        vec!["observation"],
        vec![seed_goal(80, "Escapes scope", Some("rysweet/Simard"))],
    );
    let mut state = empty_state();

    let result = act_observe_only(&brain, &targets, &mut state);
    assert!(
        result.is_err(),
        "a proposal outside the target set must fail closed"
    );
    // Fail-closed leaves the board untouched — no partially-scoped goal leaks in.
    assert!(
        state.active_goals.active.is_empty(),
        "no goal may be seeded when a proposal escapes the target scope"
    );
}

#[test]
fn observe_only_fails_closed_on_unscoped_proposal() {
    let targets = vec!["hyenas/repo-a".to_string()];
    let brain = MockObserveBrain::ok(vec!["observation"], vec![seed_goal(80, "No repo", None)]);
    let mut state = empty_state();

    let result = act_observe_only(&brain, &targets, &mut state);
    assert!(
        result.is_err(),
        "a proposal with repo=None must fail closed (no implicit Simard scope)"
    );
    assert!(state.active_goals.active.is_empty());
}

// ── No fallback: a broken observe brain surfaces, never dispatches ───────────

#[test]
fn observe_only_surfaces_brain_error_without_fallback() {
    let targets = vec!["hyenas/repo-a".to_string()];
    let brain = MockObserveBrain::err("observe brain exploded");
    let mut state = empty_state();

    let result = act_observe_only(&brain, &targets, &mut state);
    assert!(
        result.is_err(),
        "a failing observe brain must surface as an error (no silent fallback to engineer dispatch)"
    );
    assert!(
        state.active_goals.active.is_empty(),
        "a failed observe pass must not mutate the board"
    );
}
