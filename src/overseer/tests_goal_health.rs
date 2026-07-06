//! Tests for the Overseer's **goal-board health** OBSERVE→ACT surface — the
//! defense-in-depth complement to #2609 (which exempts perpetual goals from the
//! no-progress hard-block). These pin the contract that:
//!
//! - the sensor projection surfaces `Blocked` goals into
//!   [`ObservedState::blocked_goals`] from the REAL board markers, reusing the
//!   EXISTING perpetual detection (#2589/#2609) and the safeguard-marker
//!   predicates — never a second notion;
//! - a `Signal::GoalBlocked` maps to a [`ProblemKind::GoalHygiene`] problem;
//! - a PERPETUAL goal false-parked by the no-progress safeguard is
//!   auto-unblocked + reactivated EXACTLY ONCE (dedup prevents a loop) and is
//!   NOT escalated;
//! - ANY goal carrying a "needs human review" marker fires `NotifyOperator` on
//!   BOTH channels (email + Signal) with the goal id + reason;
//! - the recursion guard / distinct identity fails CLOSED (no self-heal /
//!   self-whisper without a configured DISTINCT identity);
//! - the enable flag holds both actions when off;
//! - a single blocked-goal handling failure is isolated (the tick survives).
//!
//! Everything is exercised with injected fakes (goal store, notifier, clock via
//! the dedup gate, identity) — no network, no `~/.simard`.

use std::sync::{Arc, Mutex};

use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
use crate::ooda_actions::advance_goal::spawn::{
    BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX,
};

use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator,
    InFlightItem, IssueOutcome, MeetingHost, ObservedState, OrchestratorRunBrief, OverseerError,
    PrOps, RecipeBrief, RecipeLauncher, StatusReader, VerifyReport, WorkstreamHandle,
    WorkstreamStatus,
};
use crate::overseer::config::{SIMARD_OVERSEER_GOAL_HEALTH_ENV, goal_health_enabled_from};
use crate::overseer::guardrails::{AutonomyGate, RiskClass, classify};
use crate::overseer::intervention::Intervention;
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::sensor::blocked_goals_from_board;
use crate::overseer::signal::{Problem, ProblemKind, Signal, signals_from};
use crate::overseer::wiring::{overseer_identity, overseer_tick, run_overseer_tick_isolated};
use crate::overseer::{Capabilities, Overseer, decide, orient};

// ─────────────────────────── marker helpers ────────────────────────────────

/// The brain-failure safeguard `Blocked` reason for `n` cycles (mirrors the
/// deterministic marker `dispatch_spawn_engineer` writes).
fn brain_failure_reason(n: u32) -> String {
    format!("{BRAIN_FAILURE_BLOCKED_PREFIX}{n}{BRAIN_FAILURE_BLOCKED_SUFFIX}")
}

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
            id: "ws-1".to_string(),
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

struct FakeIssues;
impl crate::overseer::capabilities::IssueFiler for FakeIssues {
    fn file(&self, _run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
        Ok(IssueOutcome::FiledNew {
            url: "https://example/issues/1".to_string(),
        })
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

/// A goal-store fake: serves a fixed set of blocked goals and records every
/// `unblock` call so the self-heal path is observable without a real board.
/// `fail_unblock` makes `unblock` return an error so failure isolation can be
/// proven.
struct FakeGoalStore {
    blocked: Vec<BlockedGoal>,
    unblocked: Arc<Mutex<Vec<String>>>,
    fail_unblock: bool,
}
impl FakeGoalStore {
    fn new(blocked: Vec<BlockedGoal>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let unblocked = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                blocked,
                unblocked: unblocked.clone(),
                fail_unblock: false,
            },
            unblocked,
        )
    }
    fn failing(blocked: Vec<BlockedGoal>) -> Self {
        Self {
            blocked,
            unblocked: Arc::new(Mutex::new(Vec::new())),
            fail_unblock: true,
        }
    }
}
impl GoalCurator for FakeGoalStore {
    fn propose(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
        Ok(())
    }
    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
        Ok(vec![])
    }
    fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
        Ok(self.blocked.clone())
    }
    fn unblock(&self, goal_id: &str) -> Result<(), OverseerError> {
        if self.fail_unblock {
            return Err(OverseerError::Capability {
                what: "goal_board.unblock",
                detail: "board write failed".to_string(),
            });
        }
        self.unblocked.lock().unwrap().push(goal_id.to_string());
        Ok(())
    }
}

/// A notify channel that records every notification and always reports `Sent`.
struct RecordingChannel {
    name: String,
    seen: Arc<Mutex<Vec<OperatorNotification>>>,
}
impl RecordingChannel {
    fn new(name: &str) -> (Self, Arc<Mutex<Vec<OperatorNotification>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                name: name.to_string(),
                seen: seen.clone(),
            },
            seen,
        )
    }
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

/// Build the Overseer's capabilities around a goal store; everything else is a
/// canned fake (the status reader yields an otherwise-clean snapshot so the only
/// signals are the goal-board ones).
fn caps_with_goals(goals: Box<dyn GoalCurator>) -> Capabilities {
    Capabilities {
        status: Box::new(FakeStatus(ObservedState::default())),
        recipes: Box::new(FakeRecipes),
        prs: Box::new(FakePrs),
        deployer: Box::new(FakeDeployer),
        meetings: Box::new(FakeMeetings),
        issues: Box::new(FakeIssues),
        goals,
        auditor: Box::new(FakeAuditor),
        memory: Box::new(crate::overseer::capabilities::InertMemoryRecall),
    }
}

/// A `DualChannelNotifier` (the SAME mandatory notifier the merge path uses)
/// wired with two recording channels so a test can prove BOTH email + Signal
/// fired. Returns the notifier plus the two capture logs.
type NotifierAndLogs = (
    DualChannelNotifier,
    Arc<Mutex<Vec<OperatorNotification>>>,
    Arc<Mutex<Vec<OperatorNotification>>>,
);
fn dual_recording_notifier() -> NotifierAndLogs {
    let (email, email_log) = RecordingChannel::new("email");
    let (signal, signal_log) = RecordingChannel::new("signal");
    let notifier = DualChannelNotifier::new(vec![Box::new(email), Box::new(signal)]);
    (notifier, email_log, signal_log)
}

// ─────────────────── 1. Sensor: project the board → ObservedState ───────────

#[test]
fn blocked_goals_projection_surfaces_perpetual_and_needs_review_goals() {
    let mut board = GoalBoard::new();
    // A standing/perpetual research goal, false-parked by the no-progress
    // safeguard (perpetual + needs_review, count parsed from the marker).
    let mut research = ActiveGoal::new(
        "research",
        "Continuously research the frontier. Standing goal.",
        1,
    );
    research.status = GoalProgress::Blocked(no_progress_blocked_reason(4));
    // A NORMAL feature goal, genuinely blocked by the brain-failure safeguard
    // (needs_review, not perpetual).
    let mut feature = ActiveGoal::new("feature-x", "Ship feature X", 2);
    feature.status = GoalProgress::Blocked(brain_failure_reason(3));
    // A NORMAL goal blocked by an operator-set reason (no safeguard marker):
    // surfaced, but neither perpetual nor needs_review.
    let mut opsblock = ActiveGoal::new("ops", "Wait on infra", 3);
    opsblock.status = GoalProgress::Blocked("waiting on the operator".to_string());
    // A healthy in-progress goal: never surfaced.
    let mut healthy = ActiveGoal::new("healthy", "Make progress", 4);
    healthy.status = GoalProgress::InProgress { percent: 40 };
    board.active = vec![research, feature, opsblock, healthy];

    let blocked = blocked_goals_from_board(&board);
    assert_eq!(
        blocked.len(),
        3,
        "only the three Blocked goals are surfaced"
    );

    let research = blocked.iter().find(|b| b.id == "research").unwrap();
    assert!(
        research.perpetual,
        "standing goal is perpetual (#2589/#2609)"
    );
    assert!(
        research.needs_review,
        "no-progress marker needs human review"
    );
    assert_eq!(
        research.consecutive_no_action, 4,
        "count parsed from marker"
    );

    let feature = blocked.iter().find(|b| b.id == "feature-x").unwrap();
    assert!(!feature.perpetual);
    assert!(
        feature.needs_review,
        "brain-failure marker needs human review"
    );
    assert_eq!(feature.consecutive_no_action, 3);

    let ops = blocked.iter().find(|b| b.id == "ops").unwrap();
    assert!(!ops.perpetual);
    assert!(
        !ops.needs_review,
        "an operator-set block is not a review marker"
    );
    assert_eq!(ops.consecutive_no_action, 0);
}

#[test]
fn run_cycle_populates_observed_blocked_goals_and_emits_signals() {
    let blocked = vec![
        BlockedGoal {
            id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        },
        BlockedGoal {
            id: "feature-x".to_string(),
            reason: brain_failure_reason(3),
            perpetual: false,
            needs_review: true,
            consecutive_no_action: 3,
        },
    ];
    let (store, _log) = FakeGoalStore::new(blocked);
    let mut ov = Overseer::new(caps_with_goals(Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let cycle = ov.run_cycle().expect("cycle");
    assert_eq!(
        cycle.observed.blocked_goals.len(),
        2,
        "the Observe pass surfaces the board's blocked goals into ObservedState"
    );
    let goal_blocked: Vec<_> = cycle
        .signals
        .iter()
        .filter(|s| matches!(s, Signal::GoalBlocked { .. }))
        .collect();
    assert_eq!(
        goal_blocked.len(),
        2,
        "one GoalBlocked signal per blocked goal"
    );
}

// ─────────────────── 2. Signal → GoalHygiene problem ────────────────────────

#[test]
fn goal_blocked_signal_maps_to_a_goal_hygiene_problem() {
    let observed = ObservedState {
        blocked_goals: vec![BlockedGoal {
            id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        }],
        ..ObservedState::default()
    };
    let signals = signals_from(&observed);
    assert!(
        signals
            .iter()
            .any(|s| matches!(s, Signal::GoalBlocked { .. })),
        "a blocked goal produces a GoalBlocked signal"
    );
    let problems = orient(&signals, &[]);
    let problem = problems
        .iter()
        .find(|p| p.kind == ProblemKind::GoalHygiene)
        .expect("GoalBlocked classifies to GoalHygiene");
    assert_eq!(problem.dedup_key, "goal:blocked:research");
}

// ─────────────────── 3. Self-heal: unblock once, never escalate ─────────────

#[test]
fn perpetual_no_progress_goal_is_unblocked_once_and_not_escalated() {
    let blocked = vec![BlockedGoal {
        id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
        perpetual: true,
        needs_review: true,
        consecutive_no_action: 4,
    }];
    let (store, unblocked) = FakeGoalStore::new(blocked);
    let (notifier, email_log, signal_log) = dual_recording_notifier();

    let mut ov = Overseer::new(caps_with_goals(Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let first = overseer_tick(&mut ov);
    assert_eq!(first.goals_unblocked, 1, "the false-park is self-healed");
    assert_eq!(first.goals_escalated, 0, "a false-park is NOT escalated");
    assert_eq!(first.errors, 0);
    assert!(!first.panicked);

    // The SAME blocked goal is still served next tick (the fake store does not
    // reflect the unblock). Dedup must prevent a re-unblock loop.
    let second = overseer_tick(&mut ov);
    assert_eq!(
        second.goals_unblocked, 0,
        "dedup prevents a re-unblock loop"
    );
    assert_eq!(
        second.goals_health_suppressed, 1,
        "the duplicate is counted"
    );

    assert_eq!(
        &*unblocked.lock().unwrap(),
        &vec!["research".to_string()],
        "the goal store's unblock was called EXACTLY ONCE across two ticks"
    );
    assert!(
        email_log.lock().unwrap().is_empty() && signal_log.lock().unwrap().is_empty(),
        "a self-healed false-park never notifies the operator"
    );
}

// ─────────────────── 4. Escalate: NotifyOperator on both channels ───────────

#[test]
fn needs_review_goal_escalates_to_operator_on_both_channels() {
    let reason = brain_failure_reason(3);
    let blocked = vec![BlockedGoal {
        id: "feature-x".to_string(),
        reason: reason.clone(),
        perpetual: false,
        needs_review: true,
        consecutive_no_action: 3,
    }];
    let (store, unblocked) = FakeGoalStore::new(blocked);
    let (notifier, email_log, signal_log) = dual_recording_notifier();

    let mut ov = Overseer::new(caps_with_goals(Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let report = overseer_tick(&mut ov);
    assert_eq!(report.goals_escalated, 1, "the genuine block is escalated");
    assert_eq!(
        report.goals_unblocked, 0,
        "a normal goal is not self-healed"
    );
    assert!(
        unblocked.lock().unwrap().is_empty(),
        "escalation never unblocks the goal"
    );

    for (chan, log) in [("email", &email_log), ("signal", &signal_log)] {
        let seen = log.lock().unwrap();
        assert_eq!(seen.len(), 1, "the {chan} channel received the escalation");
        let n = &seen[0];
        assert!(
            n.headline.contains("feature-x"),
            "the {chan} notification names the goal id: {n:?}"
        );
        assert!(
            n.problem.contains(&reason) || n.problem.contains("needs human review"),
            "the {chan} notification carries the block reason: {n:?}"
        );
    }
}

// ─────────────────── 5. Anti-recursion: fail closed on no identity ──────────

#[test]
fn self_heal_and_escalate_fail_closed_without_a_distinct_identity() {
    // No `.with_identity(...)` ⇒ the default RecursionGuard is unconfigured, so
    // both goal-health actions must be REFUSED (fail closed) — the Overseer can
    // never self-heal / escalate (nor self-whisper) without a DISTINCT identity.
    let (store, unblocked) = FakeGoalStore::new(vec![]);
    let (notifier, email_log, signal_log) = dual_recording_notifier();
    let mut ov = Overseer::new(caps_with_goals(Box::new(store)))
        .with_goal_health_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let unblock = ov.act(&Intervention::UnblockGoal {
        goal_id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
    });
    assert!(
        matches!(unblock, Err(OverseerError::Recursion { .. })),
        "unconfigured identity refuses the self-heal (fail closed): {unblock:?}"
    );

    let escalate = ov.act(&Intervention::EscalateBlockedGoal {
        goal_id: "feature-x".to_string(),
        reason: brain_failure_reason(3),
    });
    assert!(
        matches!(escalate, Err(OverseerError::Recursion { .. })),
        "unconfigured identity refuses the escalation (fail closed): {escalate:?}"
    );

    assert!(
        unblocked.lock().unwrap().is_empty(),
        "no unblock reached the store"
    );
    assert!(
        email_log.lock().unwrap().is_empty() && signal_log.lock().unwrap().is_empty(),
        "no notification reached the operator"
    );
}

// ─────────────────── 6. Enable flag: disabled ⇒ no action ───────────────────

#[test]
fn disabled_goal_health_holds_both_actions() {
    let blocked = vec![
        BlockedGoal {
            id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        },
        BlockedGoal {
            id: "feature-x".to_string(),
            reason: brain_failure_reason(3),
            perpetual: false,
            needs_review: true,
            consecutive_no_action: 3,
        },
    ];
    let (store, unblocked) = FakeGoalStore::new(blocked);
    let (notifier, email_log, signal_log) = dual_recording_notifier();

    // Identity configured, notifier wired — only the enable flag is OFF.
    let mut ov = Overseer::new(caps_with_goals(Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(false)
        .with_operator_notifier(Box::new(notifier));

    let report = overseer_tick(&mut ov);
    assert_eq!(report.goals_unblocked, 0);
    assert_eq!(report.goals_escalated, 0);
    assert!(
        report.held >= 2,
        "both goal-health interventions are held: {report:?}"
    );
    assert!(
        unblocked.lock().unwrap().is_empty(),
        "disabled ⇒ no unblock is performed"
    );
    assert!(
        email_log.lock().unwrap().is_empty() && signal_log.lock().unwrap().is_empty(),
        "disabled ⇒ no escalation is dispatched"
    );
}

#[test]
fn goal_health_enable_flag_is_opt_out_and_off_when_overseer_off() {
    use crate::overseer::config::OVERSEER_ENABLED_ENV;
    let env = |pairs: &[(&str, &str)]| {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| owned.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone())
    };
    // Default (unset): ON, because the acting Overseer is opt-out ON.
    assert!(goal_health_enabled_from(env(&[])));
    // Explicit falsey on the goal-health flag: OFF.
    assert!(!goal_health_enabled_from(env(&[(
        SIMARD_OVERSEER_GOAL_HEALTH_ENV,
        "off"
    )])));
    // A disabled Overseer forces goal-health OFF regardless of the flag.
    assert!(!goal_health_enabled_from(env(&[
        (OVERSEER_ENABLED_ENV, "false"),
        (SIMARD_OVERSEER_GOAL_HEALTH_ENV, "on"),
    ])));
}

// ─────────────────── 7. Failure isolation: the tick survives ────────────────

#[test]
fn a_failing_unblock_is_isolated_and_the_tick_survives() {
    let blocked = vec![BlockedGoal {
        id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
        perpetual: true,
        needs_review: true,
        consecutive_no_action: 4,
    }];
    // The goal store errors on unblock — a capability failure, not a panic.
    let store = FakeGoalStore::failing(blocked);
    let mut ov = Overseer::new(caps_with_goals(Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let report = run_overseer_tick_isolated(&mut ov);
    assert_eq!(
        report.goals_unblocked, 0,
        "the failed self-heal took no effect"
    );
    assert_eq!(report.errors, 1, "the failure is counted, isolated");
    assert!(!report.panicked, "a capability error never panics the tick");
    assert!(report.duration_ms < 60_000);
}

// ─────────────────── decide/classify unit routing ──────────────────────────

#[test]
fn decide_routes_a_blocked_goal_by_shape() {
    // Perpetual + no-progress marker ⇒ self-heal (unblock), never escalate.
    let self_heal = decide(&Problem {
        kind: ProblemKind::GoalHygiene,
        priority: crate::overseer::signal::Priority::High,
        dedup_key: "goal:blocked:research".to_string(),
        summary: "blocked".to_string(),
        evidence: vec![Signal::GoalBlocked {
            goal_id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        }],
    });
    assert!(matches!(self_heal, Intervention::UnblockGoal { .. }));

    // Normal + needs_review ⇒ escalate.
    let escalate = decide(&Problem {
        kind: ProblemKind::GoalHygiene,
        priority: crate::overseer::signal::Priority::High,
        dedup_key: "goal:blocked:feature-x".to_string(),
        summary: "blocked".to_string(),
        evidence: vec![Signal::GoalBlocked {
            goal_id: "feature-x".to_string(),
            reason: brain_failure_reason(3),
            perpetual: false,
            needs_review: true,
            consecutive_no_action: 3,
        }],
    });
    assert!(matches!(escalate, Intervention::EscalateBlockedGoal { .. }));

    // A plain operator-set block (no marker) ⇒ Report only, no autonomous action.
    let report = decide(&Problem {
        kind: ProblemKind::GoalHygiene,
        priority: crate::overseer::signal::Priority::Normal,
        dedup_key: "goal:blocked:ops".to_string(),
        summary: "blocked".to_string(),
        evidence: vec![Signal::GoalBlocked {
            goal_id: "ops".to_string(),
            reason: "waiting on the operator".to_string(),
            perpetual: false,
            needs_review: false,
            consecutive_no_action: 0,
        }],
    });
    assert!(matches!(report, Intervention::Report));
}

#[test]
fn goal_health_interventions_are_routine_and_admitted_by_default_gate() {
    for iv in [
        Intervention::UnblockGoal {
            goal_id: "g".to_string(),
            reason: "r".to_string(),
        },
        Intervention::EscalateBlockedGoal {
            goal_id: "g".to_string(),
            reason: "r".to_string(),
        },
    ] {
        assert_eq!(classify(&iv), RiskClass::Routine);
        assert!(
            AutonomyGate::default().admit(&iv).is_ok(),
            "routine goal-health actions are admitted; their own dedup/identity gates apply in act"
        );
    }
    assert_eq!(
        Intervention::UnblockGoal {
            goal_id: "g".to_string(),
            reason: "r".to_string()
        }
        .label(),
        "unblock_goal"
    );
    assert_eq!(
        Intervention::EscalateBlockedGoal {
            goal_id: "g".to_string(),
            reason: "r".to_string()
        }
        .label(),
        "escalate_blocked_goal"
    );
}
