//! Tests for the autonomous self-deploy rail (#2590): OBSERVE drift → DECIDE
//! `Deploy` → guarded ACT, plus the anti-thrash, fail-safe, and opt-out rails.
//!
//! These are the test-first contract for wiring Simard's already-built
//! self-deploy machinery into the overseer loop. They exercise the DETERMINISTIC
//! rail end-to-end while mocking the binary swap so CI never reinstalls: a fake
//! [`BinaryDeployer`] records swaps instead of running
//! [`SelfDeployOrchestrator`](crate::self_deploy::SelfDeployOrchestrator).

use std::sync::{Arc, Mutex};

use crate::overseer::capabilities::{Deployer, OverseerError};
use crate::overseer::deploy::{
    AncestryOracle, BinaryDeployer, CRASH_LOOP_CHURN_THRESHOLD, CanaryResult, CanaryRunner,
    DeployContext, DeployRefusal, GuardedDeployer, evaluate_deploy_gate,
};
use crate::overseer::deploy_trigger::{
    DEFAULT_TRANSIENT_BACKOFF_BASE_SECS, MIN_TRANSIENT_BACKOFF_BASE_SECS,
    TRANSIENT_BACKOFF_BASE_ENV, TRANSIENT_BACKOFF_CEILING_SECS, TransientRedBackoff,
    deploy_drift_signal, deploy_throttle_test_guard, global_deploy_throttle_allow,
    reset_global_deploy_throttle, transient_backoff_base_secs, transient_backoff_secs,
};
use crate::overseer::intervention::Intervention;
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::signal::{Priority, Problem, ProblemKind, Signal};
use crate::overseer::{decide, orient};
use crate::self_deploy::{DeployDrift, GitDeploySource, ReconcileDetector};

// ─────────────────────────── fakes ─────────────────────────────────────────

struct FakeCanary(bool);
impl CanaryRunner for FakeCanary {
    fn run_canary(&self, _t: &str) -> Result<CanaryResult, OverseerError> {
        Ok(CanaryResult {
            passed: self.0,
            detail: "4/4 gates".to_string(),
            failing_gate: None,
            failing_detail: None,
            transient: false,
        })
    }
}

struct FakeAncestry(bool);
impl AncestryOracle for FakeAncestry {
    fn is_ancestor(&self, _a: &str, _d: &str) -> Result<bool, OverseerError> {
        Ok(self.0)
    }
}

/// Records each binary swap instead of performing one (mocks the reinstall).
struct CountingDeployer(Arc<Mutex<usize>>);
impl BinaryDeployer for CountingDeployer {
    fn deploy_binary(&self, target: &str) -> Result<String, OverseerError> {
        *self.0.lock().unwrap() += 1;
        Ok(target.to_string())
    }
}

/// A binary deployer whose swap always fails — models a swap/restart failure so
/// the rollback/notify tail can be asserted.
struct FailingDeployer;
impl BinaryDeployer for FailingDeployer {
    fn deploy_binary(&self, _t: &str) -> Result<String, OverseerError> {
        Err(OverseerError::Capability {
            what: "binary_swap",
            detail: "swap boom".to_string(),
        })
    }
}

struct Capture(Arc<Mutex<Vec<OperatorNotification>>>);
impl NotifyChannel for Capture {
    fn name(&self) -> &str {
        "capture"
    }
    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        self.0.lock().unwrap().push(n.clone());
        ChannelDelivery::Sent
    }
}

#[allow(clippy::type_complexity)]
fn guarded(
    canary_passed: bool,
    is_ancestor: bool,
    churn: u64,
    running: &str,
) -> (
    GuardedDeployer,
    Arc<Mutex<usize>>,
    Arc<Mutex<Vec<OperatorNotification>>>,
) {
    let deployed = Arc::new(Mutex::new(0));
    let seen = Arc::new(Mutex::new(vec![]));
    let notifier = DualChannelNotifier::new(vec![Box::new(Capture(seen.clone()))]);
    let gd = GuardedDeployer::new(
        Box::new(FakeCanary(canary_passed)),
        Box::new(CountingDeployer(deployed.clone())),
        Box::new(FakeAncestry(is_ancestor)),
        notifier,
        running.to_string(),
        churn,
        "rysweet/Simard".to_string(),
    );
    (gd, deployed, seen)
}

fn drift_problem(target_commit: &str, behind: usize) -> Problem {
    let sig = Signal::DeployDriftDetected {
        target_commit: target_commit.to_string(),
        behind_commits: behind,
    };
    let problems = orient(&[sig], &[]);
    assert_eq!(problems.len(), 1, "one drift signal → one problem");
    problems.into_iter().next().unwrap()
}

// ─────────────────────────── DECIDE rail ────────────────────────────────────

#[test]
fn decide_maps_deploy_drift_to_deploy_of_merged_head() {
    let problem = drift_problem("merged_head_abc123", 4);
    assert_eq!(problem.kind, ProblemKind::DeployDrift);
    assert_eq!(problem.priority, Priority::High);
    assert_eq!(
        decide(&problem),
        Intervention::Deploy {
            commit: "merged_head_abc123".to_string()
        },
        "a deploy-drift problem must yield Deploy{{ merged_head }}"
    );
}

#[test]
fn decide_deploy_drift_is_fail_closed_without_a_target_commit() {
    // A DeployDrift problem carrying no resolvable target commit must ESCALATE,
    // never deploy blind.
    let problem = Problem {
        kind: ProblemKind::DeployDrift,
        priority: Priority::High,
        dedup_key: "deploy:drift".to_string(),
        summary: "running binary is behind merged main".to_string(),
        evidence: vec![],
        why: None,
    };
    assert!(
        matches!(decide(&problem), Intervention::Escalate { .. }),
        "no target commit → escalate, never a blind deploy"
    );

    // An empty/whitespace target commit is likewise fail-closed.
    let problem = drift_problem_with_empty_target();
    assert!(matches!(decide(&problem), Intervention::Escalate { .. }));
}

fn drift_problem_with_empty_target() -> Problem {
    Problem {
        kind: ProblemKind::DeployDrift,
        priority: Priority::High,
        dedup_key: "deploy:drift".to_string(),
        summary: "drift".to_string(),
        evidence: vec![Signal::DeployDriftDetected {
            target_commit: "   ".to_string(),
            behind_commits: 1,
        }],
        why: None,
    }
}

#[test]
fn decide_does_not_deploy_when_there_is_no_drift() {
    // Any non-drift problem must never produce a Deploy intervention.
    let problems = orient(
        &[Signal::PrReadyToMerge {
            repo: "rysweet/Simard".to_string(),
            pr: 7,
        }],
        &[],
    );
    let iv = decide(&problems[0]);
    assert!(
        !matches!(iv, Intervention::Deploy { .. }),
        "a non-drift problem must not deploy"
    );
}

// ─────────────────────────── OBSERVE (signal) ──────────────────────────────

#[test]
fn deploy_drift_signal_is_absent_when_current() {
    assert!(
        deploy_drift_signal(&DeployDrift::current(), "head").is_none(),
        "no drift → no signal → no deploy"
    );
}

#[test]
fn failsafe_missing_repo_yields_no_drift_and_no_signal_without_panicking() {
    // ReconcileDetector.detect() fails safe on a git/source error (missing repo)
    // → no drift → the rail emits no signal → no deploy. It must never panic the
    // overseer loop.
    let detector = ReconcileDetector::new(GitDeploySource::at("/no-such-repo-xyz-2590"));
    let drift = detector.detect();
    assert!(
        !drift.needs_deploy,
        "a missing checkout must report no drift"
    );
    assert!(
        deploy_drift_signal(&drift, "irrelevant").is_none(),
        "no drift → no deploy signal"
    );
}

// ─────────────────────────── guarded ACT ────────────────────────────────────

#[test]
fn guarded_deploy_happy_path_swaps_once_and_notifies_success() {
    let (gd, deployed, seen) = guarded(true, false, 0, "runningOLD");
    let report = gd.deploy("mergedNEW").expect("clean forward deploy");
    assert!(report.gates_passed);
    assert_eq!(report.deployed_commit, "mergedNEW");
    assert_eq!(*deployed.lock().unwrap(), 1, "binary swapped exactly once");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "operator notified before and after success");
    assert_eq!(seen[0].kind, "deploy-starting");
    assert_eq!(seen[1].kind, "deploy");
}

#[test]
fn guarded_deploy_refuses_no_op_and_notifies_without_swapping() {
    let (gd, deployed, seen) = guarded(true, false, 0, "sameCommit");
    let err = gd.deploy("sameCommit").unwrap_err();
    assert!(format!("{err}").contains("no-op"));
    assert_eq!(*deployed.lock().unwrap(), 0, "no swap on refusal");
    assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
}

#[test]
fn guarded_deploy_refuses_rollback_and_notifies_without_swapping() {
    // Target is an ancestor of running → a rollback → refused.
    let (gd, deployed, seen) = guarded(true, true, 0, "runningNEW");
    let err = gd.deploy("olderAncestor").unwrap_err();
    assert!(format!("{err}").contains("rollback"));
    assert_eq!(*deployed.lock().unwrap(), 0);
    assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
}

#[test]
fn guarded_deploy_red_canary_refuses_and_rolls_back_leaving_no_swap() {
    // Canary build+verify fails → refuse BEFORE any binary swap (nothing is left
    // deployed) and notify the operator of the aborted attempt.
    let (gd, deployed, seen) = guarded(false, false, 0, "runningOLD");
    let err = gd.deploy("mergedNEW").unwrap_err();
    assert!(format!("{err}").contains("red canary"));
    assert_eq!(
        *deployed.lock().unwrap(),
        0,
        "a red canary must leave no swap in place"
    );
    assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
}

#[test]
fn guarded_deploy_refuses_crash_loop_and_notifies_without_swapping() {
    let (gd, deployed, seen) = guarded(true, false, CRASH_LOOP_CHURN_THRESHOLD, "runningOLD");
    let err = gd.deploy("mergedNEW").unwrap_err();
    assert!(format!("{err}").contains("crash-loop"));
    assert_eq!(*deployed.lock().unwrap(), 0);
    assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
}

#[test]
fn guarded_deploy_notifies_on_a_failed_binary_swap() {
    // The gate passes but the swap itself fails: the operator must still be told
    // (deploy attempt on EVERY outcome) and the error surfaces.
    let seen = Arc::new(Mutex::new(vec![]));
    let notifier = DualChannelNotifier::new(vec![Box::new(Capture(seen.clone()))]);
    let gd = GuardedDeployer::new(
        Box::new(FakeCanary(true)),
        Box::new(FailingDeployer),
        Box::new(FakeAncestry(false)),
        notifier,
        "runningOLD".to_string(),
        0,
        "rysweet/Simard".to_string(),
    );
    let err = gd.deploy("mergedNEW").unwrap_err();
    assert!(format!("{err}").contains("swap boom"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].kind, "deploy-starting");
    assert_eq!(seen[1].kind, "deploy-refused");
}

// ─────────────────────────── anti-thrash ────────────────────────────────────

#[test]
fn anti_thrash_two_ticks_within_min_interval_do_not_double_deploy() {
    // Drift persists across ticks until the swap lands. The PROCESS-GLOBAL
    // throttle production actually uses must ensure two ticks inside the
    // min-interval window produce at most ONE guarded deploy.
    let _guard = deploy_throttle_test_guard();
    reset_global_deploy_throttle();
    let problem = drift_problem("mergedHEAD", 1);
    let deployed = Arc::new(Mutex::new(0usize));

    for now in [1_000u64, 1_300u64] {
        if !global_deploy_throttle_allow(now, 900) {
            continue; // in-window: the rail skips this tick.
        }
        if let Intervention::Deploy { commit } = decide(&problem) {
            let notifier =
                DualChannelNotifier::new(vec![Box::new(Capture(Arc::new(Mutex::new(vec![]))))]);
            let gd = GuardedDeployer::new(
                Box::new(FakeCanary(true)),
                Box::new(CountingDeployer(deployed.clone())),
                Box::new(FakeAncestry(false)),
                notifier,
                "runningOLD".to_string(),
                0,
                "rysweet/Simard".to_string(),
            );
            let _ = gd.deploy(&commit);
        }
    }

    assert_eq!(
        *deployed.lock().unwrap(),
        1,
        "two ticks within the min-interval must deploy at most once"
    );
    reset_global_deploy_throttle();
}

// ─────────────────────────── QA / gadugi scenario ───────────────────────────

#[test]
fn wired_daemon_behind_main_autonomously_emits_and_executes_a_guarded_deploy() {
    // End-to-end (binary swap MOCKED so CI never reinstalls): a daemon whose
    // running binary is behind merged main OBSERVEs drift, DECIDEs a Deploy to
    // the merged head, and the guarded executor performs the (mocked) swap and
    // notifies the operator — no operator command.
    let drift = DeployDrift::from_parts(2, Vec::new());
    assert!(drift.needs_deploy, "behind main → needs deploy");

    let signal = deploy_drift_signal(&drift, "mergedHEAD").expect("drift signal");
    let problems = orient(&[signal], &[]);
    assert_eq!(problems[0].kind, ProblemKind::DeployDrift);

    let intervention = decide(&problems[0]);
    let commit = match intervention {
        Intervention::Deploy { commit } => commit,
        other => panic!("expected a Deploy intervention, got {other:?}"),
    };
    assert_eq!(commit, "mergedHEAD");

    let (gd, deployed, seen) = guarded(true, false, 0, "runningOLD");
    let report = gd.deploy(&commit).expect("guarded deploy");
    assert!(report.gates_passed);
    assert_eq!(*deployed.lock().unwrap(), 1, "the (mocked) swap ran once");
    assert_eq!(
        seen.lock().unwrap()[0].kind,
        "deploy-starting",
        "operator notified before the autonomous deploy swap"
    );
}

// ─────────────── STEP 7 TDD (#4415): transient-red classification ───────────
//
// A1 (reproduce-first): before `CanaryResult.transient`, a transient build
// failure reddened IDENTICALLY to a genuine regression, so the self-deploy loop
// could not tell a flaky red (safe to retry) from a real one (must not thrash).
// The new flag distinguishes them WITHOUT changing the deploy decision. These
// tests are written FIRST and fail until the field and the class-aware backoff
// exist.

#[test]
fn transient_and_deterministic_red_are_distinguishable_but_both_refuse() {
    let transient_red = CanaryResult {
        passed: false,
        detail: "target canary build failed: linker OOM".to_string(),
        failing_gate: Some("build".to_string()),
        failing_detail: Some("SIGKILL (out of memory)".to_string()),
        transient: true,
    };
    let regression_red = CanaryResult {
        passed: false,
        detail: "3/4 gates".to_string(),
        failing_gate: Some("unit-test".to_string()),
        failing_detail: Some("assertion failed: overseer::tick::latch".to_string()),
        transient: false,
    };

    // The reproduce-first crux: the two reds are now distinguishable.
    assert!(transient_red.transient, "a build/OOM red is transient");
    assert!(
        !regression_red.transient,
        "a real gate regression is deterministic"
    );
    assert_ne!(
        transient_red.transient, regression_red.transient,
        "the two red classes must be distinguishable (the whole point of #4415)"
    );

    // Contract preserved: `transient` NEVER influences the deploy gate — a red is
    // a red; both refuse regardless of class.
    for red in [&transient_red, &regression_red] {
        assert!(!red.passed);
        let ctx = DeployContext {
            running_commit: "aaaaaaaaaaaa".to_string(),
            target_commit: "bbbbbbbbbbbb".to_string(),
            target_is_ancestor_of_running: false,
            canary_passed: red.passed,
            recent_restart_churn: 0,
        };
        assert_eq!(
            evaluate_deploy_gate(&ctx),
            Err(DeployRefusal::RedCanary),
            "any red canary must still refuse the deploy"
        );
    }
}

#[test]
fn green_canary_is_never_transient_invariant() {
    let green = CanaryResult {
        passed: true,
        detail: "4/4 gates".to_string(),
        failing_gate: None,
        failing_detail: None,
        transient: false,
    };
    assert!(green.passed);
    assert!(
        !green.transient,
        "invariant: transient == false whenever the canary is green"
    );
}

// ───────────── STEP 7 TDD (#4415): class-aware bounded backoff (Brick C) ─────
//
// A transient red must back off with bounded exponential growth so it neither
// hammers the tick (busy-loop) nor sleeps unboundedly, and a green canary must
// RESET the backoff so self-deploy can converge to merged main. The math is
// saturating (no overflow), floored to a non-zero base, and capped at an
// absolute ceiling. Written FIRST — fail until the backoff API exists.

#[test]
fn backoff_is_zero_when_there_are_no_transient_failures() {
    assert_eq!(
        transient_backoff_secs(0, 60),
        0,
        "no transient failures → no backoff (nothing to recover from)"
    );
}

#[test]
fn backoff_grows_exponentially_with_consecutive_transient_failures() {
    assert_eq!(transient_backoff_secs(1, 60), 60);
    assert_eq!(transient_backoff_secs(2, 60), 120);
    assert_eq!(transient_backoff_secs(3, 60), 240);
    assert_eq!(transient_backoff_secs(4, 60), 480);
}

#[test]
fn backoff_saturates_at_the_absolute_ceiling_without_overflow() {
    // A pathological streak must not overflow (saturating math) and must never
    // exceed the absolute ceiling.
    assert_eq!(
        transient_backoff_secs(64, 60),
        TRANSIENT_BACKOFF_CEILING_SECS,
        "backoff must clamp to the absolute ceiling"
    );
    assert_eq!(
        transient_backoff_secs(u32::MAX, 60),
        TRANSIENT_BACKOFF_CEILING_SECS,
        "a huge streak must clamp, never panic on overflow"
    );
}

#[test]
fn backoff_base_floor_prevents_a_busy_loop() {
    // A mis-set base of 0 must clamp to a non-zero floor so a transient red can
    // never produce a zero-delay retry storm (self-DoS).
    let d = transient_backoff_secs(1, 0);
    assert!(
        d >= MIN_TRANSIENT_BACKOFF_BASE_SECS,
        "base=0 must clamp up to the non-zero floor, got {d}"
    );
    assert!(
        d > 0,
        "a transient failure must always back off by a non-zero amount"
    );
}

#[test]
#[serial_test::serial(transient_backoff_env)]
fn backoff_base_defaults_when_env_is_unset() {
    let prev = std::env::var(TRANSIENT_BACKOFF_BASE_ENV).ok();
    // SAFETY: serialized env toggle, restored below.
    unsafe {
        std::env::remove_var(TRANSIENT_BACKOFF_BASE_ENV);
    }
    assert_eq!(
        transient_backoff_base_secs(),
        DEFAULT_TRANSIENT_BACKOFF_BASE_SECS
    );
    unsafe {
        match prev {
            Some(v) => std::env::set_var(TRANSIENT_BACKOFF_BASE_ENV, v),
            None => std::env::remove_var(TRANSIENT_BACKOFF_BASE_ENV),
        }
    }
}

#[test]
#[serial_test::serial(transient_backoff_env)]
fn backoff_base_env_override_is_parsed_and_floored() {
    let prev = std::env::var(TRANSIENT_BACKOFF_BASE_ENV).ok();
    // SAFETY: serialized env toggle, restored below.
    unsafe {
        std::env::set_var(TRANSIENT_BACKOFF_BASE_ENV, "0");
    }
    assert!(
        transient_backoff_base_secs() >= MIN_TRANSIENT_BACKOFF_BASE_SECS,
        "an env base of 0 must clamp to the non-zero floor"
    );
    unsafe {
        std::env::set_var(TRANSIENT_BACKOFF_BASE_ENV, "30");
    }
    assert_eq!(
        transient_backoff_base_secs(),
        30,
        "a valid env base is honoured"
    );
    unsafe {
        match prev {
            Some(v) => std::env::set_var(TRANSIENT_BACKOFF_BASE_ENV, v),
            None => std::env::remove_var(TRANSIENT_BACKOFF_BASE_ENV),
        }
    }
}

#[test]
fn backoff_state_grows_then_resets_on_green() {
    // Convergence: consecutive transient reds back off increasingly, then a green
    // canary RESETS the streak so the running binary can advance to merged main.
    let mut b = TransientRedBackoff::new();
    let d1 = b.record_transient_red();
    let d2 = b.record_transient_red();
    let d3 = b.record_transient_red();
    assert!(
        d1 > 0 && d2 > d1 && d3 > d2,
        "consecutive transient reds must back off increasingly: {d1}, {d2}, {d3}"
    );
    b.record_green();
    assert_eq!(
        b.current_delay_secs(),
        0,
        "a green canary must reset the transient backoff so self-deploy converges"
    );
}

#[test]
fn backoff_state_is_idempotent_on_repeated_green() {
    let mut b = TransientRedBackoff::new();
    b.record_transient_red();
    b.record_green();
    b.record_green();
    assert_eq!(
        b.current_delay_secs(),
        0,
        "repeated greens stay reset (idempotent — no double-count, no thrash)"
    );
}

#[test]
fn backoff_state_deterministic_red_does_not_fast_retry() {
    // A genuine regression is NOT fast-retried by the transient backoff; the
    // existing global min-interval throttle owns its cadence. A deterministic red
    // clears the transient streak.
    let mut b = TransientRedBackoff::new();
    b.record_transient_red();
    b.record_transient_red();
    b.record_deterministic_red();
    assert_eq!(
        b.current_delay_secs(),
        0,
        "a deterministic red must clear the transient fast-retry streak"
    );
}
