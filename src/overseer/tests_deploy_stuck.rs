//! TDD (RED) tests for the self-deploy **loop-halt escalation** on a
//! persistently-red canary (Problem 1 / issue #4420 — the 8h+ self-deploy
//! crash-loop that silently grew DeployDrift from 1 → 6 commits behind main).
//!
//! These are written BEFORE the implementation and reference API that does not
//! exist yet (`deploy_trigger::{record_red_canary_result, red_canary_streak_for,
//! red_canary_halt_threshold, reset_red_canary_streak}` and the new
//! `"deploy-stuck"` [`OperatorNotification`] kind), so the crate test build
//! FAILS to compile until Seam B is wired into `observe_deploy_drift`. That
//! compile failure IS the red state of red→green→refactor.
//!
//! Contract under test (Seam B, the OBSERVE-rail READ side — recording happens
//! at the ACT/canary site, this rail only reads):
//!
//!   1. **Halt the crash-loop.** Once a target SHA has accumulated
//!      `red_canary_halt_threshold()` consecutive red canaries, the OBSERVE rail
//!      STOPS re-signalling that SHA — it no longer silently re-attempts the same
//!      failing deploy every tick while drift grows.
//!   2. **Surface to the operator.** The rail fires a ONE-SHOT, operator-visible
//!      `deploy-stuck` escalation (on BOTH channels) naming the stuck SHA — a
//!      persistently-red canary escalates to a human instead of looping blind.
//!   3. **No over-suppression.** Below the threshold, legitimate drift is still
//!      signalled normally — the guard is bounded, not a blanket mute.
//!   4. **One-shot.** A stuck SHA escalates at most ONCE, even across ticks (the
//!      daemon rebuilds the Overseer every tick), so a stuck loop cannot flood
//!      the operator with alerts.
//!
//! Everything is hermetic: injected capability fakes + recording channels, the
//! process-global streak/throttle statics serialised via
//! `deploy_trigger::deploy_throttle_test_guard()` and reset each test.

use std::sync::{Arc, Mutex};

use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, BlockedGoal, DeployDriftObservation, DeployReport, Deployer,
    GoalBrief, GoalCurator, InFlightItem, IssueFiler, IssueOutcome, MeetingHost, ObservedState,
    OrchestratorRunBrief, OverseerError, PrOps, RecipeBrief, RecipeLauncher, StatusReader,
    VerifyReport, WorkstreamHandle, WorkstreamStatus,
};
use crate::overseer::deploy_trigger::{self, DeployDriftObserver};
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::signal::Signal;
use crate::overseer::{Capabilities, Overseer};

// ─────────────────────────── capability fakes ──────────────────────────────

struct FakeStatus(ObservedState);
impl StatusReader for FakeStatus {
    fn snapshot(&self) -> Result<ObservedState, OverseerError> {
        Ok(self.0.clone())
    }
}

struct FakeRecipes;
impl RecipeLauncher for FakeRecipes {
    fn launch(&self, _b: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        Ok(WorkstreamHandle {
            id: "ws-inert".to_string(),
        })
    }
    fn poll(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        Ok(WorkstreamStatus::Running)
    }
}

struct FakePrs;
impl PrOps for FakePrs {
    fn verify(&self, _r: &str, _p: u32) -> Result<VerifyReport, OverseerError> {
        Ok(VerifyReport {
            ready: false,
            checks: vec![],
        })
    }
    fn merge(&self, _r: &str, _p: u32) -> Result<(), OverseerError> {
        Ok(())
    }
    fn resolve_conflict(&self, _r: &str, _p: u32) -> Result<(), OverseerError> {
        Ok(())
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

/// A fake drift sensor returning a fixed observation so the OBSERVE rail's
/// loop-halt wiring can be exercised without a live git checkout.
struct FakeDriftObserver(Option<DeployDriftObservation>);
impl DeployDriftObserver for FakeDriftObserver {
    fn observe(&self) -> Option<DeployDriftObservation> {
        self.0.clone()
    }
}

/// A notify channel that records every notification and always reports `Sent`.
struct RecordingChannel {
    name: String,
    seen: Arc<Mutex<Vec<OperatorNotification>>>,
}
impl NotifyChannel for RecordingChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        self.seen.lock().unwrap().push(n.clone());
        ChannelDelivery::Sent
    }
}

/// Build a dual-channel notifier backed by the CALLER's shared logs, so the
/// one-shot test can reuse the same email/signal logs across two ticks (each
/// tick rebuilds the Overseer and therefore its notifier).
fn recording_notifier_into(
    email_log: Arc<Mutex<Vec<OperatorNotification>>>,
    signal_log: Arc<Mutex<Vec<OperatorNotification>>>,
) -> DualChannelNotifier {
    DualChannelNotifier::new(vec![
        Box::new(RecordingChannel {
            name: "email".to_string(),
            seen: email_log,
        }),
        Box::new(RecordingChannel {
            name: "signal".to_string(),
            seen: signal_log,
        }),
    ])
}

fn caps() -> Capabilities {
    Capabilities {
        status: Box::new(FakeStatus(ObservedState::default())),
        recipes: Box::new(FakeRecipes),
        prs: Box::new(FakePrs),
        deployer: Box::new(FakeDeployer),
        meetings: Box::new(FakeMeetings),
        issues: Box::new(FakeIssues),
        goals: Box::new(FakeGoals),
        auditor: Box::new(FakeAuditor),
        memory: Box::new(crate::overseer::capabilities::InertMemoryRecall),
    }
}

/// Does any recorded notification of the given kind identify the target SHA
/// (by full hash or its short 12-char prefix in any human field)?
fn any_notice_names(log: &Arc<Mutex<Vec<OperatorNotification>>>, kind: &str, sha: &str) -> bool {
    let short = &sha[..sha.len().min(12)];
    log.lock()
        .unwrap()
        .iter()
        .filter(|n| n.kind == kind)
        .any(|n| {
            n.problem.contains(sha)
                || n.headline.contains(short)
                || n.problem.contains(short)
                || n.next_step.contains(short)
        })
}

fn count_kind(log: &Arc<Mutex<Vec<OperatorNotification>>>, kind: &str) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|n| n.kind == kind)
        .count()
}

// ─────────────────────────────── tests ─────────────────────────────────────

/// T4 — a persistently-red SHA HALTS: past the threshold the OBSERVE rail stops
/// re-signalling the stuck SHA (no more silent drift growth) AND fires a
/// one-shot operator escalation on both channels naming that SHA.
#[test]
fn observe_rail_halts_and_escalates_a_persistently_red_sha() {
    let _guard = deploy_trigger::deploy_throttle_test_guard();
    deploy_trigger::reset_global_deploy_throttle();
    deploy_trigger::reset_red_canary_streak();

    let stuck = "a".repeat(40);
    // Simulate the ACT path having recorded `threshold` consecutive red canaries
    // for this SHA over the prior stuck ticks.
    let threshold = deploy_trigger::red_canary_halt_threshold();
    for _ in 0..threshold {
        deploy_trigger::record_red_canary_result(&stuck, true);
    }

    let email_log = Arc::new(Mutex::new(Vec::new()));
    let signal_log = Arc::new(Mutex::new(Vec::new()));
    let mut ov = Overseer::new(caps())
        .with_high_risk_autonomy(true)
        .with_operator_notifier(Box::new(recording_notifier_into(
            email_log.clone(),
            signal_log.clone(),
        )))
        .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(DeployDriftObservation {
            target_commit: stuck.clone(),
            behind_commits: 6,
        }))));

    let report = ov.run_cycle().expect("cycle");

    // (a) the stuck SHA's deploy signal is SUPPRESSED — the crash-loop halts.
    assert!(
        !report.signals.iter().any(|s| matches!(
            s,
            Signal::DeployDriftDetected { target_commit, .. } if *target_commit == stuck
        )),
        "a persistently-red SHA must stop re-signalling instead of looping silently"
    );

    // (b) exactly one operator-visible 'stuck' escalation fired, naming the SHA…
    assert_eq!(
        count_kind(&email_log, "deploy-stuck"),
        1,
        "exactly one stuck escalation reaches the operator (email channel)"
    );
    assert!(
        any_notice_names(&email_log, "deploy-stuck", &stuck),
        "the stuck escalation identifies the stuck target SHA"
    );
    // …and it is delivered on BOTH channels (never suppressed from the operator).
    assert_eq!(
        count_kind(&signal_log, "deploy-stuck"),
        1,
        "the stuck escalation is operator-visible on the second channel too"
    );

    deploy_trigger::reset_red_canary_streak();
    deploy_trigger::reset_global_deploy_throttle();
}

/// T5 (non-regression) — BELOW the threshold the guard stays disengaged:
/// legitimate drift is still signalled and NO stuck escalation fires. Guards
/// against over-suppression regressing normal self-deploy.
#[test]
fn observe_rail_still_signals_below_the_halt_threshold() {
    let _guard = deploy_trigger::deploy_throttle_test_guard();
    deploy_trigger::reset_global_deploy_throttle();
    deploy_trigger::reset_red_canary_streak();

    let sha = "c".repeat(40);
    let threshold = deploy_trigger::red_canary_halt_threshold();
    // One short of the threshold — the guard must NOT engage.
    for _ in 0..threshold.saturating_sub(1) {
        deploy_trigger::record_red_canary_result(&sha, true);
    }

    let email_log = Arc::new(Mutex::new(Vec::new()));
    let signal_log = Arc::new(Mutex::new(Vec::new()));
    let mut ov = Overseer::new(caps())
        .with_high_risk_autonomy(true)
        .with_operator_notifier(Box::new(recording_notifier_into(
            email_log.clone(),
            signal_log,
        )))
        .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(DeployDriftObservation {
            target_commit: sha.clone(),
            behind_commits: 2,
        }))));

    let report = ov.run_cycle().expect("cycle");

    assert!(
        report.signals.iter().any(|s| matches!(
            s,
            Signal::DeployDriftDetected { target_commit, .. } if *target_commit == sha
        )),
        "below the halt threshold legitimate drift is still signalled (no over-suppression)"
    );
    assert_eq!(
        count_kind(&email_log, "deploy-stuck"),
        0,
        "no stuck escalation below the threshold"
    );

    deploy_trigger::reset_red_canary_streak();
    deploy_trigger::reset_global_deploy_throttle();
}

/// T6 (one-shot) — a stuck SHA escalates AT MOST ONCE even across ticks. The
/// throttle is reset between the two ticks so BOTH reach the stuck check,
/// proving the one-shot latch (not the anti-thrash throttle) is what caps the
/// escalation at one — no alert flooding while the loop is stuck.
#[test]
fn observe_rail_escalates_a_stuck_sha_at_most_once_across_ticks() {
    let _guard = deploy_trigger::deploy_throttle_test_guard();
    deploy_trigger::reset_global_deploy_throttle();
    deploy_trigger::reset_red_canary_streak();

    let stuck = "d".repeat(40);
    let threshold = deploy_trigger::red_canary_halt_threshold();
    for _ in 0..threshold {
        deploy_trigger::record_red_canary_result(&stuck, true);
    }

    let email_log = Arc::new(Mutex::new(Vec::new()));
    let signal_log = Arc::new(Mutex::new(Vec::new()));
    for tick in 0..2usize {
        // Fresh Overseer each tick (as the daemon rebuilds it); reset the
        // throttle so the tick is not short-circuited before the stuck check.
        deploy_trigger::reset_global_deploy_throttle();
        let mut ov = Overseer::new(caps())
            .with_high_risk_autonomy(true)
            .with_operator_notifier(Box::new(recording_notifier_into(
                email_log.clone(),
                signal_log.clone(),
            )))
            .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(
                DeployDriftObservation {
                    target_commit: stuck.clone(),
                    behind_commits: 6 + tick,
                },
            ))));
        let _ = ov.run_cycle().expect("cycle");
    }

    assert_eq!(
        count_kind(&email_log, "deploy-stuck"),
        1,
        "a stuck SHA escalates at most once, even across repeated stuck ticks"
    );

    deploy_trigger::reset_red_canary_streak();
    deploy_trigger::reset_global_deploy_throttle();
}
