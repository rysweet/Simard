//! TDD (Step 7) — FAILING tests that pin the **verify-and-merge escalation
//! convergence** rail (#4344 / #4145).
//!
//! The live defect: the `Intervention::VerifyAndMergePr { repo, pr }` act arm in
//! [`crate::overseer`] emits `ActOutcome::Escalated` **unconditionally, every
//! tick**, with no memory that the same `repo#pr` was already escalated and is
//! still in the same state. Two green, `mergeable = MERGEABLE`, `state = CLEAN`
//! PRs — `rysweet/Simard#4344` and `rysweet/Simard#4145` — were therefore
//! re-escalated as `DeliveryReady` on 14+ consecutive ticks over 5+ hours, each
//! emitting the identical `escalated to operator: verify-and-merge …` line, yet
//! neither ever merged and neither escalation ever *resolved*. The escalation was
//! a symptom broadcast on a loop, not a driver of progress.
//!
//! The fix (see
//! `docs/reference/overseer-merge-escalation-convergence.md`): the Overseer gains
//! one per-`repo#pr` [`BackoffGate`](crate::overseer::guardrails::BackoffGate)
//! field — `merge_escalation_gate` — reusing the same `SIMARD_OVERSEER_BACKOFF_*`
//! primitive as `coverage_backoff`. The act arm follows a
//! **peek → attempt real progress → commit-on-escalate** discipline:
//!
//!   1. A successful `merge()` returns `ActOutcome::Merged` and **never touches
//!      the gate** (a merged PR needs no suppression).
//!   2. On a non-merge outcome, `peek(key, now)` the gate:
//!      - `Admit` (first escalation, or the backoff window elapsed, or the
//!        classified [`MergeBlocker`] changed class) → emit `Escalated` **with**
//!        the classified blocker, then `commit(key, now)`.
//!      - `Suppress` (an UNCHANGED escalation inside the current window) → return
//!        the new `ActOutcome::MergeEscalationSuppressed { reason }` (an
//!        acknowledged held/pending state), and do **not** commit again.
//!
//! These tests reference the TARGET API — the new
//! `ActOutcome::MergeEscalationSuppressed` variant and the per-PR convergence
//! behavior — and MUST fail to compile / fail to pass against the current tree
//! (whose arm returns bare `Escalated` every tick). They go GREEN only once the
//! convergence rail lands in Step 8.
//!
//! Everything is exercised with injected fakes and a virtual clock — no network,
//! no real `gh`, no wall-clock dependence.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator,
    InFlightItem, InertMemoryRecall, IssueFiler, IssueOutcome, MeetingHost, ObservedState,
    OrchestratorRunBrief, OverseerError, PrOps, RecipeBrief, RecipeLauncher, StatusReader,
    VerifyReport, WorkstreamHandle, WorkstreamStatus,
};
use crate::overseer::intervention::Intervention;
use crate::overseer::{ActOutcome, Capabilities, Overseer};

const REPO: &str = "rysweet/Simard";
const PR_A: u32 = 4344;
const PR_B: u32 = 4145;

// ─────────────────────────── the PR-ops fake ───────────────────────────────

/// The per-`repo#pr` merge outcome the act arm will observe on a tick. The TEST
/// controls it explicitly per PR before each `act()` call, so the "state" a tick
/// sees is deterministic and the fake never advances on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrState {
    /// `verify()` objective pre-filter says the PR is NOT ready → the act arm
    /// classifies a `MergeBlocker::NotReady` and escalates (never merges).
    NotReady,
    /// `verify()` passes the objective pre-filter but the authoritative agentic
    /// merge-judge refuses → `merge()` returns `OverseerError::NotMergeReady` → a
    /// `MergeBlocker::JudgeRefused` (a DIFFERENT blocker class from `NotReady`).
    JudgeRefused,
    /// `verify()` ready AND `merge()` succeeds → `ActOutcome::Merged`.
    Mergeable,
}

/// A `PrOps` fake keyed by PR number, whose per-PR [`PrState`] the test sets
/// before each `act()`. Counts successful merges so the authority-preservation
/// invariant (a suppressed convergence NEVER merges) is observable.
struct MapPrs {
    states: Arc<Mutex<HashMap<u32, PrState>>>,
    merges: Arc<Mutex<usize>>,
}
impl MapPrs {
    fn state_for(&self, pr: u32) -> PrState {
        self.states
            .lock()
            .unwrap()
            .get(&pr)
            .copied()
            .unwrap_or(PrState::NotReady)
    }
}
impl PrOps for MapPrs {
    fn verify(&self, _repo: &str, pr: u32) -> Result<VerifyReport, OverseerError> {
        // Only `NotReady` fails the objective pre-filter; the judge refusal is
        // discovered later, at `merge()`.
        Ok(VerifyReport {
            ready: !matches!(self.state_for(pr), PrState::NotReady),
            checks: vec![],
        })
    }
    fn merge(&self, _repo: &str, pr: u32) -> Result<(), OverseerError> {
        match self.state_for(pr) {
            PrState::Mergeable => {
                *self.merges.lock().unwrap() += 1;
                Ok(())
            }
            // The authoritative agentic judge refused (or failed closed) → an
            // escalation, never an error, never a blind merge.
            _ => Err(OverseerError::NotMergeReady {
                pr,
                reason: "the merge-readiness judge did not approve".to_string(),
            }),
        }
    }
    fn resolve_conflict(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
        Ok(())
    }
}

// ─────────────────────────── other capability fakes ────────────────────────

struct FakeStatus;
impl StatusReader for FakeStatus {
    fn snapshot(&self) -> Result<ObservedState, OverseerError> {
        Ok(ObservedState::default())
    }
}

struct FakeRecipes;
impl RecipeLauncher for FakeRecipes {
    fn launch(&self, _b: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        Ok(WorkstreamHandle {
            id: "ws-noop".to_string(),
        })
    }
    fn poll(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        Ok(WorkstreamStatus::Running)
    }
}

struct FakeDeployer;
impl Deployer for FakeDeployer {
    fn deploy(&self, commit: &str) -> Result<DeployReport, OverseerError> {
        Ok(DeployReport {
            deployed_commit: commit.to_string(),
            gates_passed: true,
        })
    }
    fn deployed_commit(&self) -> Result<String, OverseerError> {
        Ok("deadbeef".to_string())
    }
}

struct FakeMeetings;
impl MeetingHost for FakeMeetings {
    fn transfer_goal(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
        Ok(())
    }
}

struct FakeAuditor;
impl Auditor for FakeAuditor {
    fn run_audit(&self, scope: &AuditScope) -> Result<AuditReport, OverseerError> {
        Ok(AuditReport {
            scope: scope.clone(),
            passed: true,
            findings: vec![],
        })
    }
}

struct FakeIssues;
impl IssueFiler for FakeIssues {
    fn file(&self, _run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
        Ok(IssueOutcome::FiledNew {
            url: "https://example/issues/1".to_string(),
        })
    }
}

struct FakeGoals;
impl GoalCurator for FakeGoals {
    fn propose(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
        Ok(())
    }
    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
        Ok(vec![])
    }
    fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
        Ok(vec![])
    }
    fn workstream_gaps(&self, _anomalies: &[String]) -> Result<Vec<GapItemAlias>, OverseerError> {
        Ok(vec![])
    }
}
// `workstream_gaps` returns `Vec<GapItem>`; alias keeps the fake readable while
// avoiding an extra import churn if the signal module path shifts.
type GapItemAlias = crate::overseer::signal::GapItem;

// ─────────────────────────── harness ───────────────────────────────────────

/// A virtual clock the test advances; injected via `Overseer::with_clock`.
fn virtual_clock() -> (Arc<AtomicI64>, Box<dyn Fn() -> i64 + Send + Sync>) {
    let now = Arc::new(AtomicI64::new(0));
    let handle = now.clone();
    (now, Box::new(move || handle.load(Ordering::SeqCst)))
}

/// Build an `Overseer` wired around the `MapPrs` fake and a virtual clock.
/// Returns the overseer, the shared per-PR state handle, the shared merge
/// counter, and the clock handle the test advances.
#[allow(clippy::type_complexity)]
fn wired() -> (
    Overseer,
    Arc<Mutex<HashMap<u32, PrState>>>,
    Arc<Mutex<usize>>,
    Arc<AtomicI64>,
) {
    let states = Arc::new(Mutex::new(HashMap::new()));
    let merges = Arc::new(Mutex::new(0usize));
    let caps = Capabilities {
        status: Box::new(FakeStatus),
        recipes: Box::new(FakeRecipes),
        prs: Box::new(MapPrs {
            states: states.clone(),
            merges: merges.clone(),
        }),
        deployer: Box::new(FakeDeployer),
        meetings: Box::new(FakeMeetings),
        issues: Box::new(FakeIssues),
        goals: Box::new(FakeGoals),
        auditor: Box::new(FakeAuditor),
        memory: Box::new(InertMemoryRecall),
    };
    let (now, clock) = virtual_clock();
    let ov = Overseer::new(caps).with_clock(clock);
    (ov, states, merges, now)
}

fn set_state(states: &Arc<Mutex<HashMap<u32, PrState>>>, pr: u32, st: PrState) {
    states.lock().unwrap().insert(pr, st);
}

fn verify_and_merge(ov: &mut Overseer, pr: u32) -> ActOutcome {
    ov.act(&Intervention::VerifyAndMergePr {
        repo: REPO.to_string(),
        pr,
    })
    .expect("a non-merge outcome must be an escalation/held state, never an Err")
}

fn is_suppressed(out: &ActOutcome) -> bool {
    matches!(out, ActOutcome::MergeEscalationSuppressed { .. })
}

// ─────────────────────────── the convergence contract ──────────────────────

/// 1. Escalate ONCE, then suppress the unchanged repeat. The first tick with a
///    stuck PR escalates; a second tick in the same state, inside the backoff
///    window, is HELD (acknowledged-pending) — NOT re-escalated.
#[test]
fn escalates_once_then_suppresses_the_unchanged_repeat() {
    let (mut ov, states, merges, now) = wired();
    set_state(&states, PR_A, PrState::NotReady);

    now.store(0, Ordering::SeqCst);
    let first = verify_and_merge(&mut ov, PR_A);
    assert_eq!(
        first,
        ActOutcome::Escalated,
        "the first surfacing of a stuck merge-ready PR escalates to the operator: {first:?}"
    );

    // Same state, well inside the 900s base backoff window.
    now.store(300, Ordering::SeqCst);
    let second = verify_and_merge(&mut ov, PR_A);
    assert!(
        is_suppressed(&second),
        "an UNCHANGED escalation inside the backoff window is HELD (acknowledged \
         pending), never re-escalated every tick (#4344 / #4145): {second:?}"
    );
    assert_ne!(
        second,
        ActOutcome::Escalated,
        "the second identical tick must NOT re-page the operator"
    );

    // Authority invariant: a suppressed convergence NEVER merged anything.
    assert_eq!(
        *merges.lock().unwrap(),
        0,
        "acknowledged-blocked convergence must never advance a PR toward merge"
    );
}

/// 2. The window elapses → re-surface. Suppression is BOUNDED: past the backoff
///    window a still-stuck PR is escalated again (never permanently silenced).
#[test]
fn re_surfaces_once_the_backoff_window_elapses() {
    let (mut ov, states, _merges, now) = wired();
    set_state(&states, PR_A, PrState::NotReady);

    now.store(0, Ordering::SeqCst);
    assert_eq!(verify_and_merge(&mut ov, PR_A), ActOutcome::Escalated);

    // Inside the window → held.
    now.store(800, Ordering::SeqCst);
    assert!(
        is_suppressed(&verify_and_merge(&mut ov, PR_A)),
        "still inside the 900s window ⇒ held"
    );

    // Past the 900s base window → the still-stuck PR re-surfaces.
    now.store(1000, Ordering::SeqCst);
    let later = verify_and_merge(&mut ov, PR_A);
    assert_eq!(
        later,
        ActOutcome::Escalated,
        "past the backoff window a still-stuck PR is escalated again (bounded \
         suppression, never permanent silence): {later:?}"
    );
}

/// 3. A real state change re-pages immediately. When the classified
///    `MergeBlocker` changes CLASS (here `NotReady` → `JudgeRefused`) for the same
///    `repo#pr`, that is a genuine state change worth re-paging — it is NOT the
///    "unchanged repeat" the gate suppresses, even inside the window.
#[test]
fn a_changed_blocker_re_pages_within_the_window() {
    let (mut ov, states, _merges, now) = wired();

    // Tick 1 @ t=0: blocker = NotReady (pre-filter said not ready).
    set_state(&states, PR_A, PrState::NotReady);
    now.store(0, Ordering::SeqCst);
    assert_eq!(verify_and_merge(&mut ov, PR_A), ActOutcome::Escalated);

    // Tick 2 @ t=120 (still inside the window) but the blocker changed class:
    // the PR now clears the objective pre-filter and it is the JUDGE that refuses.
    set_state(&states, PR_A, PrState::JudgeRefused);
    now.store(120, Ordering::SeqCst);
    let second = verify_and_merge(&mut ov, PR_A);
    assert_eq!(
        second,
        ActOutcome::Escalated,
        "a CHANGED blocker class is a fresh escalation, not the unchanged repeat \
         the gate suppresses — it re-pages immediately: {second:?}"
    );
}

/// 4. Success converges with NO escalation and NO gate write. A successful merge
///    returns `Merged` and must not arm the backoff window — proven behaviorally:
///    a PR that merges at t=0 and then (hypothetically) re-surfaces stuck at t=1
///    is escalated immediately, because success never consumed a dedup slot.
#[test]
fn a_successful_merge_neither_escalates_nor_writes_the_gate() {
    let (mut ov, states, merges, now) = wired();

    set_state(&states, PR_B, PrState::Mergeable);
    now.store(0, Ordering::SeqCst);
    let merged = verify_and_merge(&mut ov, PR_B);
    assert_eq!(
        merged,
        ActOutcome::Merged,
        "a green, judge-approved PR merges: {merged:?}"
    );
    assert_eq!(*merges.lock().unwrap(), 1, "exactly one squash-merge");

    // The very next tick the same key is stuck again (contrived — a merged PR
    // normally disappears). Because the success did NOT arm the window, this
    // FIRST stuck observation must escalate, not be suppressed.
    set_state(&states, PR_B, PrState::NotReady);
    now.store(1, Ordering::SeqCst);
    let after = verify_and_merge(&mut ov, PR_B);
    assert_eq!(
        after,
        ActOutcome::Escalated,
        "a merged PR needs no suppression, so success must NOT commit the gate — \
         a subsequent stuck observation escalates on its first surfacing: {after:?}"
    );
}

/// 5. Distinct PRs do not suppress each other — the gate keys on `repo#pr`, so one
///    stuck PR's backoff window can never silence an unrelated PR.
#[test]
fn distinct_prs_have_independent_convergence_keys() {
    let (mut ov, states, _merges, now) = wired();
    set_state(&states, PR_A, PrState::NotReady);
    set_state(&states, PR_B, PrState::NotReady);

    now.store(0, Ordering::SeqCst);
    assert_eq!(
        verify_and_merge(&mut ov, PR_A),
        ActOutcome::Escalated,
        "#4344 escalates on its first surfacing"
    );
    assert_eq!(
        verify_and_merge(&mut ov, PR_B),
        ActOutcome::Escalated,
        "#4145 escalates independently — #4344's window must not suppress it"
    );

    // And #4344's own repeat is still suppressed (its key is armed).
    now.store(300, Ordering::SeqCst);
    assert!(
        is_suppressed(&verify_and_merge(&mut ov, PR_A)),
        "#4344's unchanged repeat is still held within its own window"
    );
}

/// 6. Clock regression fails toward SURFACING — a `now` before the last admit is
///    treated as "window elapsed" and re-admits, so the loop can never wedge
///    silent on an untrustworthy clock.
#[test]
fn clock_regression_fails_toward_surfacing() {
    let (mut ov, states, _merges, now) = wired();
    set_state(&states, PR_A, PrState::NotReady);

    now.store(1000, Ordering::SeqCst);
    assert_eq!(verify_and_merge(&mut ov, PR_A), ActOutcome::Escalated);

    // The clock jumps BACKWARDS (now < last_admit). The gate must fail toward
    // surfacing rather than suppress on a clock it cannot trust.
    now.store(200, Ordering::SeqCst);
    let regressed = verify_and_merge(&mut ov, PR_A);
    assert_eq!(
        regressed,
        ActOutcome::Escalated,
        "a backwards clock jump re-admits (fail toward surfacing), never wedges \
         the escalation silent: {regressed:?}"
    );
}

/// 7. Authority invariant, pinned explicitly: an acknowledged-blocked convergence
///    NEVER produces `ActOutcome::Merged`. Suppressing a repeat escalation only
///    stops the operator notification; it never advances the PR toward merge.
#[test]
fn a_suppressed_convergence_never_becomes_a_merge() {
    let (mut ov, states, merges, now) = wired();
    set_state(&states, PR_A, PrState::NotReady);

    now.store(0, Ordering::SeqCst);
    let _first = verify_and_merge(&mut ov, PR_A);

    // Many suppressed ticks inside the window.
    for t in [100, 200, 300, 400, 500] {
        now.store(t, Ordering::SeqCst);
        let out = verify_and_merge(&mut ov, PR_A);
        assert!(
            !matches!(out, ActOutcome::Merged),
            "a suppressed/held convergence must never merge: {out:?}"
        );
    }
    assert_eq!(
        *merges.lock().unwrap(),
        0,
        "no merge ever happened through the convergence (held) path"
    );
}
