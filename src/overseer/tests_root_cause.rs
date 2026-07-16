//! TDD (RED) tests for the Overseer's MANDATORY ROOT-CAUSE ("WHY") principle
//! (issue #2635). These are written BEFORE the implementation and therefore
//! reference API that does not exist yet — the crate test build will FAIL to
//! compile until the feature lands. That failure IS the red state of
//! red→green→refactor; each assertion below is the executable contract the
//! implementation must satisfy.
//!
//! Binding principle under test: whenever the Overseer detects a `Problem` it
//! MUST first determine **WHY** (a structured root-cause analysis derived from
//! evidence signals + observed telemetry + cognitive-memory recall of prior
//! same-signature occurrences) before/while acting. The chosen action MUST
//! target the root cause when possible; a symptom-only mitigation MUST be
//! explicitly labelled with the root cause recorded as **unaddressed** and
//! surfaced — never silently patched. Deliberate operator/dependency blocks are
//! **acknowledged** (addressed, not a symptom) so they never cry wolf.
//!
//! Canonical antipattern eliminated: blindly `UnblockGoal` a perpetual goal
//! every cycle instead of asking *why it keeps getting blocked* and
//! fixing/escalating that cause.
//!
//! Everything is hermetic: injected capability fakes + a real in-memory
//! `LibraryCognitiveMemory` (amplihack-memory-lib, G2). No network, no
//! `~/.simard`.

use std::sync::{Arc, Mutex};

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;
use crate::ooda_actions::advance_goal::spawn::{
    BRAIN_FAILURE_BLOCKED_PREFIX, BRAIN_FAILURE_BLOCKED_SUFFIX,
};

use crate::overseer::activity::{ProblemEntry, humanize_tick};
use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator,
    InFlightItem, IssueOutcome, MeetingHost, ObservedState, OrchestratorRunBrief, OverseerError,
    PrOps, RecipeBrief, RecipeLauncher, StatusReader, VerifyReport, WorkstreamHandle,
    WorkstreamStatus,
};
use crate::overseer::intervention::{
    Intervention, PlannedIntervention, Remediation, RemediationClass,
};
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::root_cause::{
    PriorOccurrence, RECURRENCE_ESCALATION_THRESHOLD, analyze, root_cause_signature,
};
use crate::overseer::signal::{
    CauseSource, Confidence, Likelihood, Priority, Problem, ProblemKind, RootCause, Signal,
};
use crate::overseer::wiring::{OverseerTickReport, overseer_identity, overseer_tick};
use crate::overseer::{ActOutcome, Capabilities, Overseer, decide, orient};

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

/// A goal-store fake serving a fixed set of blocked goals and recording every
/// `unblock` call so the self-heal path is observable without a real board.
struct FakeGoalStore {
    blocked: Vec<BlockedGoal>,
    unblocked: Arc<Mutex<Vec<String>>>,
}
impl FakeGoalStore {
    fn new(blocked: Vec<BlockedGoal>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let unblocked = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                blocked,
                unblocked: unblocked.clone(),
            },
            unblocked,
        )
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

/// Capabilities around a goal store + an explicit base status snapshot (so a
/// test can inject telemetry such as `distill_fail_pct` / budget pressure).
fn caps_with(status: ObservedState, goals: Box<dyn GoalCurator>) -> Capabilities {
    Capabilities {
        status: Box::new(FakeStatus(status)),
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

/// A real in-memory cognitive-memory handle (amplihack-memory-lib, G2) for the
/// recall/store round-trip tests.
fn in_memory() -> Arc<dyn CognitiveMemoryOps> {
    Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory cognitive store"))
}

// ─────────────────────────── problem builders ──────────────────────────────

/// A GoalHygiene problem for a perpetual goal false-parked by the no-progress
/// safeguard. `why` is attached when `recurrence` is `Some` (as `orient` +
/// recall would populate it), else left `None` (raw, pre-analysis).
fn perpetual_blocked_problem(recurrence: Option<u32>) -> Problem {
    let why = recurrence.map(|n| RootCause {
        candidates: vec![crate::overseer::signal::CauseCandidate {
            label: "parked-by-no-progress-safeguard".to_string(),
            likelihood: Likelihood::High,
            evidence: vec!["blocked_goal.reason: no-progress OODA-SAFEGUARD".to_string()],
        }],
        primary_rationale: "perpetual goal parked by the no-progress safeguard (false park)"
            .to_string(),
        confidence: Confidence::High,
        source: if n > 0 {
            CauseSource::MemoryRecall
        } else {
            CauseSource::Telemetry
        },
        recurrence: n,
    });
    Problem {
        kind: ProblemKind::GoalHygiene,
        priority: Priority::High,
        dedup_key: "goal:blocked:research".to_string(),
        summary: "goal research blocked — needs human review (4 no-action cycle(s))".to_string(),
        evidence: vec![Signal::GoalBlocked {
            goal_id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        }],
        why,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Section A — pure `root_cause::analyze` contract (no memory required)
// ════════════════════════════════════════════════════════════════════════════

/// The analyzer produces a structured, human-readable WHY for a blocked
/// perpetual goal: ≥1 ranked candidate, a non-empty rationale, and a non-empty
/// `Display` one-liner naming the cause.
#[test]
fn analyze_yields_structured_why_for_a_blocked_perpetual_goal() {
    let problem = perpetual_blocked_problem(None);
    let rc = analyze(&problem, &ObservedState::default(), &[]);

    assert!(
        !rc.candidates.is_empty(),
        "a root-cause analysis must offer at least one candidate cause"
    );
    assert!(
        !rc.primary_rationale.trim().is_empty(),
        "the WHY must carry a human-readable primary rationale"
    );
    let rendered = rc.to_string();
    assert!(
        !rendered.trim().is_empty() && rendered.contains(&rc.primary_rationale),
        "Display renders the canonical one-line WHY containing the rationale: {rendered:?}"
    );
}

/// A distill-failure-rate process-health problem also yields a structured WHY
/// (candidates such as schema/format drift, model regression, upstream change).
#[test]
fn analyze_yields_structured_why_for_distill_failure_rate() {
    let problem = Problem {
        kind: ProblemKind::ProcessHealth,
        priority: Priority::High,
        dedup_key: "process:distill_fail".to_string(),
        summary: "distillation parse-failure rate 35%".to_string(),
        evidence: vec![Signal::DistillFailureRate { pct: 35.0 }],
        why: None,
    };
    let observed = ObservedState {
        distill_fail_pct: Some(35.0),
        ..ObservedState::default()
    };
    let rc = analyze(&problem, &observed, &[]);
    assert!(!rc.candidates.is_empty());
    assert!(!rc.to_string().trim().is_empty());
}

/// MANDATORY-WHY invariant: `analyze` ALWAYS returns a usable WHY (≥1 candidate,
/// non-empty rationale) for EVERY problem kind — including kinds without a
/// bespoke analyzer branch (fallback to a single low-confidence "unknown"
/// candidate). The Overseer must never face a problem with no WHY.
#[test]
fn analyze_always_produces_a_nonempty_why_for_every_problem_kind() {
    let kinds = [
        ProblemKind::ProcessHealth,
        ProblemKind::ResourcePressure,
        ProblemKind::DeliveryReady,
        ProblemKind::QualityRegression,
        ProblemKind::GoalHygiene,
        ProblemKind::CrossCutting,
        ProblemKind::LoopDetected,
        ProblemKind::DriftCorrection,
    ];
    for kind in kinds {
        let problem = Problem {
            kind,
            priority: Priority::Normal,
            dedup_key: format!("k:{kind:?}"),
            summary: format!("synthetic {kind:?} problem"),
            evidence: vec![],
            why: None,
        };
        let rc = analyze(&problem, &ObservedState::default(), &[]);
        assert!(
            !rc.candidates.is_empty(),
            "{kind:?}: analyze must yield ≥1 candidate even with no bespoke branch"
        );
        assert!(
            !rc.primary_rationale.trim().is_empty(),
            "{kind:?}: analyze must yield a non-empty rationale"
        );
    }
}

/// With no recall, the WHY is derived from telemetry alone: `source` is
/// `Telemetry` and `recurrence` is 0 (nothing seen before). This is also the
/// graceful-degrade shape when memory is unavailable.
#[test]
fn analyze_without_recall_is_telemetry_sourced_with_zero_recurrence() {
    let problem = perpetual_blocked_problem(None);
    let rc = analyze(&problem, &ObservedState::default(), &[]);
    assert_eq!(rc.source, CauseSource::Telemetry);
    assert_eq!(rc.recurrence, 0);
}

/// Recall of prior same-signature occurrences promotes the matching cause and
/// raises `recurrence`; the source is no longer telemetry-only. Uses the
/// analyzer's OWN primary label so the test does not couple to an exact string.
#[test]
fn analyze_promotes_recall_and_records_recurrence_and_memory_source() {
    let problem = perpetual_blocked_problem(None);
    let base = analyze(&problem, &ObservedState::default(), &[]);
    let primary = base.candidates[0].label.clone();

    let recall = vec![
        PriorOccurrence {
            cause_label: primary.clone(),
            action: "unblock_goal".to_string(),
            outcome: "re-blocked next cycle".to_string(),
        },
        PriorOccurrence {
            cause_label: primary.clone(),
            action: "unblock_goal".to_string(),
            outcome: "re-blocked next cycle".to_string(),
        },
    ];
    let rc = analyze(&problem, &ObservedState::default(), &recall);
    assert!(
        rc.recurrence >= 1,
        "recall of a prior same-cause occurrence must raise recurrence: {rc:?}"
    );
    assert_ne!(
        rc.source,
        CauseSource::Telemetry,
        "a recall-informed WHY is MemoryRecall/Both, not telemetry-only: {rc:?}"
    );
}

/// The root-cause signature (used for deduped escalation) is stable and
/// combines the problem's dedup key with the primary cause label — so a filed
/// issue describes the ROOT CAUSE and dedups across symptom recurrences.
#[test]
fn root_cause_signature_is_stable_and_combines_key_and_cause() {
    let problem = perpetual_blocked_problem(None);
    let rc = analyze(&problem, &ObservedState::default(), &[]);
    let primary = &rc.candidates[0];

    let sig = root_cause_signature(&problem, primary);
    assert!(
        sig.contains(&problem.dedup_key),
        "signature carries the problem dedup key: {sig}"
    );
    assert!(
        sig.contains(&primary.label),
        "signature carries the primary cause label: {sig}"
    );
    assert_eq!(
        sig,
        root_cause_signature(&problem, primary),
        "signature is deterministic"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section B — remediation routing on the blocked-goal path (pure `decide`)
// ════════════════════════════════════════════════════════════════════════════

/// FIRST-TIME false park (recurrence 0) still self-heals as a ROOT-CAUSE action
/// (`UnblockGoal`) — pins the #2609 goal-health behaviour against regression.
#[test]
fn first_time_false_park_self_heals_as_root_cause() {
    let iv = decide(&perpetual_blocked_problem(Some(0)));
    assert!(
        matches!(iv, Intervention::UnblockGoal { .. }),
        "a first-time false park is unblocked (root-cause self-heal): {iv:?}"
    );
}

/// RECURRING re-block (recurrence ≥ N) must NOT blindly re-`UnblockGoal` every
/// cycle. It escalates the root cause to the operator without creating another
/// GitHub issue.
#[test]
fn recurring_reblock_escalates_root_cause_not_blind_unblock() {
    let iv = decide(&perpetual_blocked_problem(Some(
        RECURRENCE_ESCALATION_THRESHOLD,
    )));
    assert!(
        !matches!(iv, Intervention::UnblockGoal { .. }),
        "a repeatedly re-parked perpetual goal must not be blindly re-unblocked: {iv:?}"
    );
    assert!(
        matches!(iv, Intervention::EscalateBlockedGoal { .. }),
        "the recurring root cause is escalated to the operator: {iv:?}"
    );
}

/// Recurring reblocks remain operator escalations as the recurrence count
/// climbs; they never become issue-filing actions.
#[test]
fn recurring_reblock_never_files_an_issue() {
    for recurrence in [
        RECURRENCE_ESCALATION_THRESHOLD,
        RECURRENCE_ESCALATION_THRESHOLD + 5,
    ] {
        let intervention = decide(&perpetual_blocked_problem(Some(recurrence)));
        assert!(
            matches!(intervention, Intervention::EscalateBlockedGoal { .. }),
            "recurrence {recurrence} must notify, not file: {intervention:?}"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Section C — run_cycle / tick integration (fakes; memory optional)
// ════════════════════════════════════════════════════════════════════════════

/// EVERY detected problem carries a populated WHY after a cycle — the mandatory
/// principle. A blocked perpetual goal AND a high distill-fail-rate both surface
/// with `why = Some(RootCause)` whose `Display` is a non-empty human WHY.
#[test]
fn run_cycle_populates_a_why_on_every_problem() {
    let status = ObservedState {
        distill_fail_pct: Some(30.0),
        ..ObservedState::default()
    };
    let blocked = vec![BlockedGoal {
        id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
        perpetual: true,
        needs_review: true,
        consecutive_no_action: 4,
    }];
    let (store, _log) = FakeGoalStore::new(blocked);
    let mut ov = Overseer::new(caps_with(status, Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let cycle = ov.run_cycle().expect("cycle");
    assert!(
        cycle.problems.len() >= 2,
        "both the distill-fail and blocked-goal problems are raised: {:?}",
        cycle.problems
    );
    for p in &cycle.problems {
        let why = p.why.as_ref().unwrap_or_else(|| {
            panic!("every problem must carry a WHY; missing on {:?}", p.summary)
        });
        assert!(
            !why.to_string().trim().is_empty(),
            "the WHY renders to a non-empty human-readable string for {:?}",
            p.summary
        );
        assert!(!why.candidates.is_empty());
    }
}

/// `orient` stays PURE: it folds signals into problems WITHOUT running the
/// analyzer or touching memory, so every problem it emits has `why = None`. WHY
/// enrichment is a distinct step in `run_cycle` (read-only recall), keeping
/// `orient`'s existing unit tests valid.
#[test]
fn orient_stays_pure_and_leaves_why_none() {
    let observed = ObservedState {
        blocked_goals: vec![BlockedGoal {
            id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        }],
        distill_fail_pct: Some(40.0),
        ..ObservedState::default()
    };
    let signals = crate::overseer::signal::signals_from(&observed);
    let problems = orient(&signals, &[]);
    assert!(!problems.is_empty());
    for p in &problems {
        assert!(
            p.why.is_none(),
            "orient must not populate WHY (pure); enrichment happens in run_cycle: {:?}",
            p.summary
        );
    }
}

/// A DELIBERATE operator/dependency block (not perpetual, not needs-review) is
/// ACKNOWLEDGED — addressed, cause is intentional, nothing to fix. It must NOT
/// be labelled a symptom-mitigation, must NOT bump `symptom_mitigations`, and
/// the feed must NOT raise a "root cause unaddressed" alarm for it.
#[test]
fn deliberate_operator_block_is_acknowledged_not_symptom() {
    let blocked = vec![BlockedGoal {
        id: "ops".to_string(),
        reason: "waiting on the operator to provision infra".to_string(),
        perpetual: false,
        needs_review: false,
        consecutive_no_action: 0,
    }];
    let (store, _log) = FakeGoalStore::new(blocked);
    let mut ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let cycle = ov.run_cycle().expect("cycle");
    let planned = cycle
        .plan
        .iter()
        .find(|p| matches!(p.intervention, Intervention::Report))
        .expect("a deliberate block is surfaced via Report");
    assert_eq!(planned.remediation.class, RemediationClass::Acknowledged);
    assert!(
        planned.remediation.root_cause_addressed,
        "an acknowledged deliberate block counts as addressed"
    );
    assert!(
        planned.remediation.unaddressed_note.is_none(),
        "acknowledged ⇒ no unaddressed-cause note (no false alarm)"
    );

    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.symptom_mitigations, 0,
        "a deliberate block must not inflate the symptom-mitigation count"
    );
    assert!(
        !humanize_tick(&report).contains("root cause unaddressed"),
        "no symptom-mitigation ⇒ the feed raises no 'root cause unaddressed' alarm"
    );
}

/// A genuine problem answered only by a hand-off that leaves the cause LIVE is
/// an explicit SYMPTOM-MITIGATION: budget pressure escalated to the operator
/// does not fix WHY spend is climbing (spike / runaway retry / mis-set budget).
/// It must be labelled `SymptomMitigation`, record the cause as UNADDRESSED, and
/// surface — never silently patch (design §3.3).
#[test]
fn resource_pressure_escalation_is_labelled_symptom_mitigation() {
    let status = ObservedState {
        spent_today_usd: Some(450.0),
        daily_budget_usd: Some(500.0),
        ..ObservedState::default()
    };
    let (store, _log) = FakeGoalStore::new(vec![]);
    // Opt into HIGH-RISK autonomy so the escalation is admitted + executed
    // (Escalate classifies HighRisk); the remediation label is what we pin.
    let mut ov = Overseer::new(caps_with(status, Box::new(store)))
        .with_identity(overseer_identity())
        .with_high_risk_autonomy(true);

    let cycle = ov.run_cycle().expect("cycle");
    let planned = cycle
        .plan
        .iter()
        .find(|p| matches!(p.intervention, Intervention::Escalate { .. }))
        .expect("budget pressure escalates to the operator");
    assert_eq!(
        planned.remediation.class,
        RemediationClass::SymptomMitigation,
        "escalating budget pressure mitigates the symptom, not the cause"
    );
    assert!(
        !planned.remediation.root_cause_addressed,
        "a symptom mitigation leaves the root cause unaddressed"
    );
    assert!(
        planned.remediation.unaddressed_note.is_some(),
        "the unaddressed root cause is surfaced (never silent)"
    );

    let report = overseer_tick(&mut ov);
    assert!(
        report.symptom_mitigations >= 1,
        "the symptom mitigation is counted on the tick report: {report:?}"
    );
}

/// Graceful memory degrade (G2): with NO memory wired, the cycle still produces
/// a WHY for every problem — telemetry-sourced, zero recurrence — and never
/// panics. No silent fallback: the WHY is present and honestly labelled.
#[test]
fn graceful_degrade_without_memory_still_produces_a_why() {
    let blocked = vec![BlockedGoal {
        id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
        perpetual: true,
        needs_review: true,
        consecutive_no_action: 4,
    }];
    let (store, _log) = FakeGoalStore::new(blocked);
    // Deliberately NO `.with_memory(..)`.
    let mut ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let cycle = ov.run_cycle().expect("cycle must not fail without memory");
    let p = cycle
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::GoalHygiene)
        .expect("blocked-goal problem present");
    let why = p.why.as_ref().expect("WHY present even without memory");
    assert_eq!(
        why.source,
        CauseSource::Telemetry,
        "no memory ⇒ telemetry-sourced WHY"
    );
    assert_eq!(why.recurrence, 0, "no memory ⇒ no recalled recurrence");
}

/// The activity FEED surfaces per-problem entries: each carries the problem,
/// the WHY, the action, and the remediation class — so an operator sees
/// problem + WHY + action + root/symptom for every tick entry.
#[test]
fn cycle_report_entries_render_problem_why_action_and_remediation() {
    let blocked = vec![BlockedGoal {
        id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
        perpetual: true,
        needs_review: true,
        consecutive_no_action: 4,
    }];
    let (store, _log) = FakeGoalStore::new(blocked);
    let mut ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let cycle = ov.run_cycle().expect("cycle");
    assert!(
        !cycle.entries.is_empty(),
        "the cycle emits a per-problem entry for the feed"
    );
    let entry: &ProblemEntry = cycle
        .entries
        .iter()
        .find(|e| e.key.contains("research") || e.summary.contains("research"))
        .expect("an entry for the blocked research goal");
    assert!(
        !entry.why.primary_rationale.trim().is_empty(),
        "the feed entry carries the WHY rationale"
    );
    assert!(
        !entry.action.trim().is_empty(),
        "the entry names the action"
    );
    // The remediation class is present and consistent with its addressed flag.
    match entry.remediation.class {
        RemediationClass::SymptomMitigation => {
            assert!(!entry.remediation.root_cause_addressed);
            assert!(entry.remediation.unaddressed_note.is_some());
        }
        RemediationClass::RootCause | RemediationClass::Acknowledged => {
            assert!(entry.remediation.root_cause_addressed);
        }
    }
}

/// `humanize_tick` surfaces the symptom-mitigation count with an explicit
/// "root cause unaddressed" cue when any symptom mitigation occurred — so the
/// operator sees that a cause was left live (design §3.6).
#[test]
fn humanize_tick_surfaces_symptom_mitigation_count() {
    let with_symptom = OverseerTickReport {
        problems: 2,
        escalations: 1,
        symptom_mitigations: 1,
        ..OverseerTickReport::default()
    };
    let rendered = humanize_tick(&with_symptom);
    assert!(
        rendered.contains("symptom-mitigation"),
        "the tick summary names symptom mitigation(s): {rendered}"
    );
    assert!(
        rendered.contains("root cause unaddressed"),
        "the tick summary flags the unaddressed root cause: {rendered}"
    );

    let clean = OverseerTickReport {
        problems: 1,
        symptom_mitigations: 0,
        ..OverseerTickReport::default()
    };
    assert!(
        !humanize_tick(&clean).contains("root cause unaddressed"),
        "no symptom mitigation ⇒ no unaddressed-cause cue"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section D — operator notification + memory accumulation
// ════════════════════════════════════════════════════════════════════════════

/// The blocked-goal escalation notification carries the WHY in its body, so the
/// operator receives the root-cause analysis (not just the bare symptom).
#[test]
fn escalate_blocked_goal_notification_carries_the_why() {
    let (store, _log) = FakeGoalStore::new(vec![]);
    let (notifier, email_log, signal_log) = dual_recording_notifier();
    let mut ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let why = "brain-failure safeguard tripped 3× — upstream reasoner regression in advance_goal";
    let out = ov.act(&Intervention::EscalateBlockedGoal {
        goal_id: "feature-x".to_string(),
        reason: brain_failure_reason(3),
        why: why.to_string(),
    });
    assert!(
        matches!(out, Ok(ActOutcome::GoalEscalated { .. })),
        "the escalation is dispatched: {out:?}"
    );

    for (chan, log) in [("email", &email_log), ("signal", &signal_log)] {
        let seen = log.lock().unwrap();
        assert_eq!(seen.len(), 1, "the {chan} channel received the escalation");
        let n = &seen[0];
        assert!(
            n.problem.contains(why),
            "the {chan} notification body carries the WHY: {:?}",
            n.problem
        );
        assert!(
            n.problem.to_lowercase().contains("why"),
            "the {chan} notification labels the root-cause line: {:?}",
            n.problem
        );
    }
}

/// Occurrence memory ACCUMULATES across ticks: after the Overseer acts on a
/// blocked goal (storing the occurrence via amplihack-memory-lib), a subsequent
/// cycle RECALLS it, so the WHY reports `recurrence ≥ 1`. This is the feedback
/// loop that turns a one-off false-park into a detected recurring root cause.
#[test]
fn occurrence_recall_accumulates_recurrence_across_ticks() {
    let mem = in_memory();
    let blocked = vec![BlockedGoal {
        id: "research".to_string(),
        reason: no_progress_blocked_reason(4),
        perpetual: true,
        needs_review: true,
        consecutive_no_action: 4,
    }];
    let (store, _log) = FakeGoalStore::new(blocked);
    let mut ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true)
        .with_memory(Arc::clone(&mem));

    // Tick 1: recall is empty ⇒ recurrence 0; acting records the occurrence.
    let first = ov.run_cycle().expect("cycle 1");
    let p1 = first
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::GoalHygiene)
        .expect("blocked-goal problem");
    assert_eq!(
        p1.why.as_ref().unwrap().recurrence,
        0,
        "first observation has no prior recurrence"
    );
    let _ = overseer_tick(&mut ov); // acts + best-effort stores the occurrence

    // Tick 2: the same signature is re-observed; recall must now find the prior
    // occurrence and raise recurrence.
    let second = ov.run_cycle().expect("cycle 2");
    let p2 = second
        .problems
        .iter()
        .find(|p| p.kind == ProblemKind::GoalHygiene)
        .expect("blocked-goal problem still present");
    assert!(
        p2.why.as_ref().unwrap().recurrence >= 1,
        "the recalled prior occurrence raises recurrence on the next cycle: {:?}",
        p2.why
    );
}

/// The remediation-class labelling invariant holds for ANY planned
/// intervention: `SymptomMitigation` ⟺ (root cause NOT addressed AND an
/// unaddressed note is surfaced); `RootCause`/`Acknowledged` ⟹ addressed.
/// No unlabelled silent symptom patching is possible by construction.
#[test]
fn remediation_class_and_addressed_flag_are_consistent() {
    fn check(r: &Remediation) {
        match r.class {
            RemediationClass::SymptomMitigation => {
                assert!(
                    !r.root_cause_addressed,
                    "SymptomMitigation must record the cause as unaddressed"
                );
                assert!(
                    r.unaddressed_note.is_some(),
                    "SymptomMitigation must surface an unaddressed-cause note"
                );
            }
            RemediationClass::RootCause | RemediationClass::Acknowledged => {
                assert!(
                    r.root_cause_addressed,
                    "RootCause/Acknowledged remediations are addressed"
                );
            }
        }
    }

    // Drive a mix of scenarios through run_cycle and assert the invariant on
    // every planned remediation.
    let status = ObservedState {
        spent_today_usd: Some(450.0),
        daily_budget_usd: Some(500.0),
        ..ObservedState::default()
    };
    let blocked = vec![
        BlockedGoal {
            id: "research".to_string(),
            reason: no_progress_blocked_reason(4),
            perpetual: true,
            needs_review: true,
            consecutive_no_action: 4,
        },
        BlockedGoal {
            id: "ops".to_string(),
            reason: "waiting on the operator".to_string(),
            perpetual: false,
            needs_review: false,
            consecutive_no_action: 0,
        },
    ];
    let (store, _log) = FakeGoalStore::new(blocked);
    let mut ov = Overseer::new(caps_with(status, Box::new(store)))
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true)
        .with_high_risk_autonomy(true);

    let cycle = ov.run_cycle().expect("cycle");
    assert!(!cycle.plan.is_empty());
    for planned in &cycle.plan {
        let _: &PlannedIntervention = planned;
        check(&planned.remediation);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Section G — issue #4128: self-observation stability (RED, TDD Step 7)
//
// The live incident: the Overseer re-observes its OWN emitted observation
// (`overseer-obs:…`) and the recurrence counter self-amplifies, so a static,
// unresolved set surfaces as "recurring signature seen 2× in cognitive memory".
// These tests pin the two overseer-side fixes:
//   D1  — emission hygiene: a recall-derived `overseer-obs:*` problem must NEVER
//         re-enter the write-back, so the loop is broken at the write boundary.
//   D2b — recurrence stability: occurrence memory UPSERTS one count-in-content
//         fact per signature (not one appended fact per cycle), so recurrence is
//         honest and stable, never self-amplified.
// RED until the D1 filter + D2b count-in-content upsert land.
// ════════════════════════════════════════════════════════════════════════════

/// A first-order problem the Overseer genuinely observed this cycle.
fn first_order_blocked_problem() -> Problem {
    Problem {
        kind: ProblemKind::GoalHygiene,
        priority: Priority::High,
        dedup_key: "goal:blocked:research".to_string(),
        summary: "goal research blocked — needs human review".to_string(),
        evidence: vec![],
        why: None,
    }
}

/// A recall-derived problem: the Overseer's OWN prior observation, read back out
/// of cognitive memory as a `RecurringSignature` and admitted as a ProcessHealth
/// problem whose dedup key is the `overseer-obs:` write-back signature. Feeding
/// THIS back into a write-back is the self-referential loop under repair.
fn recall_derived_self_observation() -> Problem {
    Problem {
        kind: ProblemKind::ProcessHealth,
        priority: Priority::High,
        dedup_key: "overseer-obs:goal:blocked:research".to_string(),
        summary: "recurring signature seen 2× in cognitive memory \
                  (overseer-obs:goal:blocked:research)"
            .to_string(),
        evidence: vec![],
        why: None,
    }
}

/// D1: the write-back signature MUST exclude recall-derived `overseer-obs:*`
/// problems, so the Overseer never records an observation OF its own observation.
/// The only surviving key is the first-order problem's — no nested prefix.
#[test]
fn observation_signature_excludes_recall_derived_overseer_obs_problems() {
    let problems = vec![
        first_order_blocked_problem(),
        recall_derived_self_observation(),
    ];

    let sig = super::observation_signature(&problems);

    assert_eq!(
        sig, "overseer-obs:goal:blocked:research",
        "the write-back signature keys ONLY the first-order problem; the \
         recall-derived overseer-obs:* key is filtered out: {sig}"
    );
    assert!(
        !sig.contains("overseer-obs:overseer-obs:"),
        "the write-back signature must never nest an overseer-obs: prefix: {sig}"
    );
}

/// D1: a tick whose ONLY problems are recall-derived self-observations records
/// NOTHING (`Ok(None)`). Without this, each window records an observation of the
/// prior observation, which recall re-surfaces next window — the 2× loop.
#[test]
fn write_back_of_only_recall_derived_problems_records_nothing() {
    let (store, _log) = FakeGoalStore::new(vec![]);
    let mut ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_memory_recall_enabled(true);

    let only_self = vec![recall_derived_self_observation()];
    let outcome = ov
        .write_back_observation(&only_self)
        .expect("write-back must not error");

    assert!(
        outcome.is_none(),
        "a purely self-referential observation set records nothing (breaks the \
         overseer-obs self-observation loop): {outcome:?}"
    );
}

/// D2b: occurrence memory UPSERTS one count-in-content fact per signature rather
/// than appending a new fact every cycle. After K record_occurrence calls for the
/// SAME (signature, cause), exactly ONE occurrence fact exists and it carries a
/// bounded `count` reflecting all K occurrences — so recurrence is stable and
/// still reaches the escalation threshold, without self-amplifying row growth.
#[test]
fn record_occurrence_upserts_a_single_count_in_content_fact() {
    let mem = in_memory();
    let (store, _log) = FakeGoalStore::new(vec![]);
    let ov = Overseer::new(caps_with(ObservedState::default(), Box::new(store)))
        .with_memory(Arc::clone(&mem));

    let entry = ProblemEntry {
        key: "goal:blocked:research".to_string(),
        summary: "goal research blocked".to_string(),
        why: RootCause {
            candidates: vec![crate::overseer::signal::CauseCandidate {
                label: "parked-by-no-progress-safeguard".to_string(),
                likelihood: Likelihood::High,
                evidence: vec!["blocked_goal.reason: no-progress OODA-SAFEGUARD".to_string()],
            }],
            primary_rationale: "perpetual goal parked by the no-progress safeguard".to_string(),
            confidence: Confidence::High,
            source: CauseSource::Telemetry,
            recurrence: 0,
        },
        action: "unblock_goal".to_string(),
        remediation: Remediation {
            class: RemediationClass::SymptomMitigation,
            root_cause_addressed: false,
            unaddressed_note: Some("root cause remains unaddressed".to_string()),
        },
    };
    let outcome = ActOutcome::GoalUnblocked {
        goal_id: "research".to_string(),
    };

    // Record the SAME occurrence across several cycles (as a persistently-blocked
    // goal would each tick).
    const K: usize = RECURRENCE_ESCALATION_THRESHOLD as usize + 2;
    for _ in 0..K {
        ov.record_occurrence(&entry, &outcome);
    }

    let concept = super::occurrence_concept(&entry.key);
    let facts = mem
        .search_facts(&concept, 256, 0.0)
        .expect("occurrence recall must not error");
    let occurrences: Vec<&crate::memory_cognitive::CognitiveFact> = facts
        .iter()
        .filter(|f| f.content.contains(&entry.key))
        .collect();

    assert_eq!(
        occurrences.len(),
        1,
        "record_occurrence UPSERTS one occurrence fact per signature — appending a \
         fresh fact every cycle is exactly the self-amplification #4128 fixes; got {} facts",
        occurrences.len()
    );

    let value: serde_json::Value =
        serde_json::from_str(&occurrences[0].content).expect("a stored occurrence is JSON");
    let count = value
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .expect("the upserted occurrence carries a bounded count-in-content");
    assert!(
        count >= K as u64,
        "the count-in-content reflects every occurrence (bounded/saturating): {count} >= {K}"
    );
}
