//! Tests for the Overseer's recurring **workstream gap-scan** — the additive
//! Observe→Orient→Act step that answers "WHAT WORKSTREAMS ARE WE MISSING?" on
//! each (or every Nth) tick. These pin the contract that:
//!
//! - the pure detector [`detect_workstream_gaps`] surveys the WHOLE work
//!   picture (goal board + high-signal open issues + live anomalies), correlates
//!   it against the coverage set (in-flight refs ∪ open PRs), and flags ONLY
//!   genuine, uncovered work — a gap ⟺ no active workstream AND no open PR AND
//!   (for anomalies) no fix in flight;
//! - blocked / "needs human review" goals are DELEGATED to `goal_health` and are
//!   never re-flagged as gaps (no double-notify);
//! - the Observe pass surfaces gaps into [`ObservedState::workstream_gaps`] and
//!   emits ONE consolidated [`Signal::WorkstreamGap`], which Orient classifies to
//!   a [`ProblemKind::WorkstreamCoverage`] problem;
//! - the act path notifies the operator on BOTH channels (email + Signal) with a
//!   provenance-labelled summary AND files a deduped issue per gap — through the
//!   SAME plumbing `goal_health` / M1 use, with NO new bypass — returning one
//!   [`ActOutcome::WorkstreamGapsFlagged`] that feeds DEDICATED tick counters
//!   (never the generic `issues_filed` / `escalations`);
//! - a recurring gap is deduped to AT MOST ONE notification + issue per signature
//!   (WhisperGate layer), so a fast cadence or a restart never floods the operator;
//! - the act FAILS CLOSED without a DISTINCT steward identity (anti-recursion);
//! - the `SIMARD_OVERSEER_GAP_SCAN` kill-switch holds the whole action, and
//!   `..._EVERY_N` clamps to a floor of 1;
//! - hostile external issue/PR text renders inert — a restricted-slug signature
//!   and bounded fields (V3/V4).
//!
//! Everything is exercised with injected fakes (goal/issue survey, notifier,
//! recording issue filer) and hand-built synthetic pictures — no network, no real
//! `gh`, no clock dependence.

use std::sync::{Arc, Mutex};

use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};

use crate::overseer::activity::humanize_tick;
use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator,
    InFlightItem, IssueFiler, IssueOutcome, MeetingHost, ObservedState, OrchestratorRunBrief,
    OverseerError, PrOps, RecipeBrief, RecipeLauncher, StatusReader, VerifyReport,
    WorkstreamHandle, WorkstreamStatus,
};
use crate::overseer::config::{
    OVERSEER_ENABLED_ENV, SIMARD_OVERSEER_GAP_SCAN_ENV, SIMARD_OVERSEER_GAP_SCAN_EVERY_N_ENV,
    gap_scan_enabled_from, gap_scan_every_n_from,
};
use crate::overseer::guardrails::{AutonomyGate, RiskClass, classify};
use crate::overseer::intervention::Intervention;
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::sensor::{
    MAX_GAP_FIELD_LEN, MAX_GAPS_PER_TICK, SurveyedIssue, detect_workstream_gaps,
};
use crate::overseer::signal::{
    GapCategory, GapItem, Priority, Problem, ProblemKind, Signal, signals_from,
};
use crate::overseer::wiring::{
    OverseerTickReport, overseer_identity, overseer_tick, run_overseer_tick_isolated,
};
use crate::overseer::{ActOutcome, Capabilities, Overseer, decide, orient};

// ─────────────────────────── sample gap helpers ────────────────────────────

/// A representative "uncovered high-priority goal" gap.
fn sample_goal_gap() -> GapItem {
    GapItem {
        category: GapCategory::GoalUncovered,
        ref_id: "g-hot".to_string(),
        title: "Ship the p1 launch blocker".to_string(),
        why_it_matters: "p1 goal with no engineer and no PR".to_string(),
        signature: "goal:g-hot".to_string(),
    }
}

/// A representative "unaddressed live anomaly" gap.
fn sample_anomaly_gap() -> GapItem {
    GapItem {
        category: GapCategory::AnomalyUnaddressed,
        ref_id: "distill parse-fail rate high".to_string(),
        title: "distill parse-fail rate high".to_string(),
        why_it_matters: "live anomaly with no fix in flight".to_string(),
        signature: "anomaly:distill_parse_fail_rate_high".to_string(),
    }
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

/// An `IssueFiler` that records every filed run so the "one deduped issue per
/// flagged gap, none on a suppressed repeat" contract is observable.
struct RecordingIssues {
    filed: Arc<Mutex<Vec<OrchestratorRunBrief>>>,
}
impl IssueFiler for RecordingIssues {
    fn file(&self, run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
        self.filed.lock().unwrap().push(run.clone());
        Ok(IssueOutcome::FiledNew {
            url: "https://example/issues/1".to_string(),
        })
    }
}

/// A goal-store fake whose `workstream_gaps` survey yields a fixed set of gaps —
/// the seam `run_cycle` reads to surface [`ObservedState::workstream_gaps`]
/// without a real board / GitHub survey.
struct FakeGapGoalStore {
    gaps: Vec<GapItem>,
}
impl GoalCurator for FakeGapGoalStore {
    fn propose(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
        Ok(())
    }
    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
        Ok(vec![])
    }
    fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
        Ok(vec![])
    }
    fn workstream_gaps(&self, _anomalies: &[String]) -> Result<Vec<GapItem>, OverseerError> {
        Ok(self.gaps.clone())
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

/// A `DualChannelNotifier` (the SAME mandatory notifier the merge / goal-health
/// paths use) wired with two recording channels so a test can prove BOTH
/// email + Signal fired exactly once.
fn dual_recording_notifier() -> NotifierAndLogs {
    let (email, email_log) = RecordingChannel::new("email");
    let (signal, signal_log) = RecordingChannel::new("signal");
    let notifier = DualChannelNotifier::new(vec![Box::new(email), Box::new(signal)]);
    (notifier, email_log, signal_log)
}

/// Build the Overseer's capabilities around a preset gap survey + a recording
/// issue filer; everything else is a canned fake yielding an otherwise-clean
/// picture (so the ONLY signals are the workstream-gap ones).
fn caps_for_gaps(gaps: Vec<GapItem>, filed: Arc<Mutex<Vec<OrchestratorRunBrief>>>) -> Capabilities {
    Capabilities {
        status: Box::new(FakeStatus(ObservedState::default())),
        recipes: Box::new(FakeRecipes),
        prs: Box::new(FakePrs),
        deployer: Box::new(FakeDeployer),
        meetings: Box::new(FakeMeetings),
        issues: Box::new(RecordingIssues { filed }),
        goals: Box::new(FakeGapGoalStore { gaps }),
        auditor: Box::new(FakeAuditor),
        memory: Box::new(crate::overseer::capabilities::InertMemoryRecall),
    }
}

/// The injectable env resolver used by the config tests (no `std::env` mutation).
fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| owned.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone())
}

// ═══════════════════ 1. Data model + signal emission ════════════════════════

#[test]
fn workstream_gap_signal_emitted_only_when_gaps_present() {
    // A clean board emits NO gap signal — no gap, no signal, no noise.
    let clean = ObservedState::default();
    assert!(
        !signals_from(&clean)
            .iter()
            .any(|s| matches!(s, Signal::WorkstreamGap { .. })),
        "a clean board emits no WorkstreamGap signal"
    );

    // A non-empty gap list emits exactly ONE consolidated signal carrying every
    // gap (unlike goal_health, which emits one signal per blocked goal).
    let observed = ObservedState {
        workstream_gaps: vec![sample_goal_gap(), sample_anomaly_gap()],
        ..ObservedState::default()
    };
    let sigs = signals_from(&observed);
    let gap_sigs: Vec<_> = sigs
        .iter()
        .filter(|s| matches!(s, Signal::WorkstreamGap { .. }))
        .collect();
    assert_eq!(
        gap_sigs.len(),
        1,
        "one consolidated WorkstreamGap signal per Observe pass"
    );
    match gap_sigs[0] {
        Signal::WorkstreamGap { gaps } => {
            assert_eq!(gaps.len(), 2, "the signal carries every gap verbatim");
        }
        other => panic!("expected WorkstreamGap, got {other:?}"),
    }
}

#[test]
fn workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority() {
    let observed = ObservedState {
        workstream_gaps: vec![sample_goal_gap(), sample_anomaly_gap()],
        ..ObservedState::default()
    };
    let signals = signals_from(&observed);
    let problems = orient(&signals, &[]);
    let problem = problems
        .iter()
        .find(|p| p.kind == ProblemKind::WorkstreamCoverage)
        .expect("a WorkstreamGap classifies to a WorkstreamCoverage problem");
    assert!(
        !problem.dedup_key.is_empty(),
        "the coverage problem carries a stable dedup key"
    );
    // An uncovered p1 goal / active anomaly ranks above a routine issue.
    // (Priority sorts ascending: Critical < High < Normal < Low.)
    assert!(
        problem.priority <= Priority::High,
        "an uncovered goal/anomaly gap ranks at least High: {:?}",
        problem.priority
    );
    // The evidence is the consolidated signal, so Act can render the gaps verbatim.
    assert!(
        problem
            .evidence
            .iter()
            .any(|s| matches!(s, Signal::WorkstreamGap { .. })),
        "the problem carries the WorkstreamGap evidence"
    );
}

// ═══════════════════ 2. Pure detector (hermetic) ════════════════════════════

#[test]
fn detects_uncovered_p1_goal_and_unaddressed_anomaly() {
    // THE core hermetic case: a picture with an uncovered p1 goal AND an
    // unaddressed anomaly yields exactly those two gaps, with their specifics.
    let mut board = GoalBoard::new();
    board.active = vec![
        // uncovered p1 goal: active, high-priority, no engineer, no PR ⇒ a gap.
        ActiveGoal::new("g-hot", "Ship the p1 launch blocker", 1),
        // a p2 goal WITH an assigned engineer ⇒ covered, not a gap.
        {
            let mut g = ActiveGoal::new("g-owned", "Owned p2 work", 2);
            g.assigned_to = Some("engineer-1".to_string());
            g
        },
        // a p3 goal ⇒ below the p1/p2 bar, not a gap.
        ActiveGoal::new("g-low", "p3 nice to have", 3),
    ];
    let anomalies = vec!["distill parse-fail rate 45% over window".to_string()];

    let gaps = detect_workstream_gaps(&board, &[], &anomalies, &[]);
    assert_eq!(
        gaps.len(),
        2,
        "only the uncovered p1 goal + the unaddressed anomaly are gaps: {gaps:?}"
    );

    let goal_gap = gaps
        .iter()
        .find(|g| g.category == GapCategory::GoalUncovered)
        .expect("the uncovered p1 goal is flagged");
    assert_eq!(goal_gap.ref_id, "g-hot", "the gap names the specific goal");
    assert_eq!(goal_gap.signature, "goal:g-hot", "goal signature grammar");
    assert!(
        goal_gap.title.contains("Ship the p1 launch blocker"),
        "the gap carries the goal title: {goal_gap:?}"
    );
    assert!(
        !goal_gap.why_it_matters.is_empty(),
        "the gap explains WHY an uncovered p1 goal matters"
    );

    let anomaly_gap = gaps
        .iter()
        .find(|g| g.category == GapCategory::AnomalyUnaddressed)
        .expect("the unaddressed anomaly is flagged");
    assert!(
        anomaly_gap.signature.starts_with("anomaly:"),
        "anomaly signature grammar: {anomaly_gap:?}"
    );
    assert!(
        !anomaly_gap.why_it_matters.is_empty(),
        "the anomaly gap explains why it matters"
    );
}

#[test]
fn ignores_goal_covered_by_pr_assignment_or_coverage_set() {
    // p1 goal carrying a PR work-in-progress ref ⇒ covered.
    let mut with_pr = ActiveGoal::new("g-pr", "p1 already has a PR", 1);
    with_pr.wip_refs = vec![WipRef {
        kind: "pr".to_string(),
        ref_id: "42".to_string(),
        label: "PR #42".to_string(),
        url: None,
    }];
    // p1 goal whose signature is already in the coverage set (an open PR /
    // in-flight workstream references it) ⇒ covered.
    let in_coverage = ActiveGoal::new("g-cov", "p1 already covered elsewhere", 1);

    let mut board = GoalBoard::new();
    board.active = vec![with_pr, in_coverage];
    let coverage = vec!["goal:g-cov".to_string()];

    let gaps = detect_workstream_gaps(&board, &[], &[], &coverage);
    assert!(
        gaps.is_empty(),
        "a goal that has a PR / is in the coverage set is not a gap: {gaps:?}"
    );
}

#[test]
fn delegates_blocked_goals_to_goal_health_and_never_reflags_them() {
    // A no-progress "needs human review" block is owned by goal_health.
    let mut needs_review = ActiveGoal::new("g-blocked", "p1 but blocked (needs review)", 1);
    needs_review.status = GoalProgress::Blocked(no_progress_blocked_reason(4));
    // A plain operator-set block is likewise not re-flagged by the gap-scan.
    let mut operator_block = ActiveGoal::new("g-b2", "p1 operator-blocked", 1);
    operator_block.status = GoalProgress::Blocked("waiting on infra".to_string());

    let mut board = GoalBoard::new();
    board.active = vec![needs_review, operator_block];

    let gaps = detect_workstream_gaps(&board, &[], &[], &[]);
    assert!(
        gaps.is_empty(),
        "blocked goals flow through goal_health, never re-flagged as gaps (no double-notify): {gaps:?}"
    );
}

#[test]
fn flags_high_signal_uncovered_issue_ignores_low_signal_and_covered() {
    let high = SurveyedIssue {
        repo: "rysweet/Simard".to_string(),
        number: 2630,
        title: "P1 daemon crash on startup".to_string(),
        labels: vec!["bug".to_string(), "P1".to_string()],
    };
    // Not a high-signal label ⇒ not a gap.
    let low = SurveyedIssue {
        repo: "rysweet/Simard".to_string(),
        number: 9,
        title: "typo in docs".to_string(),
        labels: vec!["documentation".to_string()],
    };
    // High-signal but already covered by an in-flight workstream ⇒ not a gap.
    let covered = SurveyedIssue {
        repo: "rysweet/Simard".to_string(),
        number: 100,
        title: "workflow:default run failing".to_string(),
        labels: vec!["workflow:default".to_string()],
    };
    let coverage = vec!["issue:rysweet/Simard#100".to_string()];

    let gaps = detect_workstream_gaps(&GoalBoard::new(), &[high, low, covered], &[], &coverage);
    assert_eq!(
        gaps.len(),
        1,
        "only the high-signal, uncovered issue is a gap: {gaps:?}"
    );
    let g = &gaps[0];
    assert_eq!(g.category, GapCategory::IssueUncovered);
    assert_eq!(g.ref_id, "rysweet/Simard#2630", "issue ref grammar");
    assert_eq!(
        g.signature, "issue:rysweet/Simard#2630",
        "issue signature grammar"
    );
}

#[test]
fn ignores_anomaly_with_a_fix_in_flight() {
    let anomalies = vec!["distill parse-fail rate high".to_string()];

    // Discover the signature the detector assigns this anomaly.
    let discovered = detect_workstream_gaps(&GoalBoard::new(), &[], &anomalies, &[]);
    assert_eq!(discovered.len(), 1, "the anomaly is a gap when uncovered");
    let sig = discovered[0].signature.clone();

    // Now mark that signature covered (a fix is in flight) ⇒ no gap.
    let gaps = detect_workstream_gaps(&GoalBoard::new(), &[], &anomalies, &[sig]);
    assert!(
        gaps.is_empty(),
        "an anomaly with a fix in flight is not a gap: {gaps:?}"
    );
}

#[test]
fn hostile_issue_title_yields_sanitized_signature_and_bounded_fields() {
    // A hostile issue title with shell/markup metacharacters + an enormous length.
    let mut nasty = String::from("`rm -rf /`; $(curl evil) \n\r\t <script>alert(1)</script> \"|&;");
    nasty.push_str(&"A".repeat(10_000));
    let issue = SurveyedIssue {
        repo: "rysweet/Simard".to_string(),
        number: 7,
        title: nasty,
        labels: vec!["bug".to_string()],
    };

    let gaps = detect_workstream_gaps(&GoalBoard::new(), &[issue], &[], &[]);
    assert_eq!(gaps.len(), 1);
    let g = &gaps[0];

    // V3: signature is a restricted slug — only [A-Za-z0-9_-#/:], no metachars,
    // built from the trusted repo + numeric issue id (never the raw title).
    assert!(
        g.signature
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-#/:".contains(c)),
        "signature must be a restricted slug: {:?}",
        g.signature
    );
    assert_eq!(g.signature, "issue:rysweet/Simard#7");

    // V4: every rendered field is bounded so a hostile title cannot inflate a
    // notification, an issue body, or a log line.
    assert!(
        g.title.chars().count() <= MAX_GAP_FIELD_LEN,
        "title is truncated to the field bound: {} chars",
        g.title.chars().count()
    );
    assert!(
        g.ref_id.chars().count() <= MAX_GAP_FIELD_LEN,
        "ref_id is bounded"
    );

    // The rendered notification body stays inert and bounded.
    let n = OperatorNotification::workstream_gap(1, &gaps);
    assert!(
        n.plain_text().len() < 4096,
        "the notification body is bounded even for a hostile title"
    );
}

#[test]
fn bounds_the_number_of_gaps_per_tick() {
    let mut board = GoalBoard::new();
    board.active = (0..(MAX_GAPS_PER_TICK + 25))
        .map(|i| ActiveGoal::new(format!("g-{i}"), format!("p1 uncovered work {i}"), 1))
        .collect();

    let gaps = detect_workstream_gaps(&board, &[], &[], &[]);
    assert!(
        gaps.len() <= MAX_GAPS_PER_TICK,
        "gaps per tick are bounded (V4): got {}",
        gaps.len()
    );
}

#[test]
fn degrades_to_empty_on_an_empty_picture() {
    let gaps = detect_workstream_gaps(&GoalBoard::new(), &[], &[], &[]);
    assert!(
        gaps.is_empty(),
        "a clean picture fabricates no gaps (degrade-to-empty): {gaps:?}"
    );
}

// ═══════════════════ 3. Act path integration ════════════════════════════════

#[test]
fn workstream_gaps_flagged_outcome_carries_batch_counts() {
    // The consolidated gap act returns ONE outcome carrying the batch counts;
    // a new `tally_outcome` arm sums them into the two DEDICATED counters (never
    // the generic issues_filed / escalations). This pins the variant shape.
    let outcome = ActOutcome::WorkstreamGapsFlagged {
        flagged: 2,
        suppressed: 1,
    };
    assert_eq!(
        outcome,
        ActOutcome::WorkstreamGapsFlagged {
            flagged: 2,
            suppressed: 1,
        }
    );
}

#[test]
fn flags_gaps_notifies_both_channels_files_once_then_dedupes_on_repeat() {
    let gaps = vec![sample_goal_gap(), sample_anomaly_gap()];
    let filed = Arc::new(Mutex::new(Vec::new()));
    let (notifier, email_log, signal_log) = dual_recording_notifier();

    let mut ov = Overseer::new(caps_for_gaps(gaps, filed.clone()))
        .with_identity(overseer_identity())
        .with_gap_scan_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    // First tick: both gaps are genuine ⇒ flagged, one consolidated notification
    // on BOTH channels, and one deduped issue per gap.
    let first = overseer_tick(&mut ov);
    assert_eq!(
        first.workstream_gaps_detected, 2,
        "both genuine gaps are flagged this tick"
    );
    assert_eq!(first.workstream_gaps_suppressed, 0);
    // Gap activity rides its DEDICATED counters, NOT the generic ones (one act
    // returns one outcome — it cannot bump both IssueFiled and Escalated).
    assert_eq!(
        first.issues_filed, 0,
        "gap issues ride the dedicated gap counter, not issues_filed"
    );
    assert_eq!(
        first.escalations, 0,
        "gaps do not ride the escalation counter"
    );
    assert_eq!(first.errors, 0);
    assert!(!first.panicked);

    // ONE consolidated notification per channel (not one per gap).
    assert_eq!(
        email_log.lock().unwrap().len(),
        1,
        "the email channel got one consolidated gap notification"
    );
    assert_eq!(
        signal_log.lock().unwrap().len(),
        1,
        "the Signal channel got one consolidated gap notification"
    );
    // One deduped issue filed per flagged gap.
    assert_eq!(
        filed.lock().unwrap().len(),
        2,
        "one deduped issue filed per flagged gap"
    );

    // The consolidated notification names the specifics.
    {
        let seen = email_log.lock().unwrap();
        let n = &seen[0];
        assert_eq!(n.kind, "workstream-gap");
        assert!(
            n.plain_text().contains("g-hot"),
            "the notification names the uncovered goal: {n:?}"
        );
    }

    // Second tick on the SAME picture: both signatures are still within the
    // dedup window ⇒ both suppressed (gate hit). No second notification, no
    // second issue — one deduped item per recurring signature, not per tick.
    let second = overseer_tick(&mut ov);
    assert_eq!(
        second.workstream_gaps_detected, 0,
        "a recurring gap is not re-flagged within the dedup window"
    );
    assert_eq!(
        second.workstream_gaps_suppressed, 2,
        "both recurring gaps are counted as suppressed"
    );
    assert_eq!(
        email_log.lock().unwrap().len(),
        1,
        "no second notification for a recurring gap"
    );
    assert_eq!(
        signal_log.lock().unwrap().len(),
        1,
        "no second Signal notification for a recurring gap"
    );
    assert_eq!(
        filed.lock().unwrap().len(),
        2,
        "no second issue for a recurring gap"
    );
}

#[test]
fn flagged_gap_brief_source_module_routes_to_default_repo() {
    use crate::stewardship::{TargetRepo, route_failure};

    // Regression for issue #2934: `act_flag_workstream_gaps` files each gap with
    // the bare source_module "overseer", which matches NO stewardship routing
    // keyword. Before the default-repo fallback, the real `StewardshipIssueFiler`
    // rejected this with `StewardshipRoutingAmbiguous`, so `flag_workstream_gaps`
    // failed every tick ("overseer intervention failed ...
    // intervention=flag_workstream_gaps error=... cannot route source-module
    // 'overseer'"). This test pins that the brief the gap path actually produces
    // routes to a real repo — the configured default (rysweet/Simard) — so the
    // issue gets filed instead of erroring.
    let gaps = vec![sample_goal_gap()];
    let filed = Arc::new(Mutex::new(Vec::new()));
    let (notifier, _email_log, _signal_log) = dual_recording_notifier();

    let mut ov = Overseer::new(caps_for_gaps(gaps, filed.clone()))
        .with_identity(overseer_identity())
        .with_gap_scan_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.workstream_gaps_detected, 1,
        "the genuine gap is flagged"
    );
    assert_eq!(report.errors, 0, "the gap intervention must not error");
    assert!(!report.panicked);

    let briefs = filed.lock().unwrap();
    assert_eq!(briefs.len(), 1, "one deduped issue filed for the gap");
    let src = &briefs[0].source_module;

    // The exact value the gap path emits must resolve — never ambiguous.
    let target = route_failure(src).unwrap_or_else(|e| {
        panic!("gap brief source_module {src:?} must route to a real repo, got {e:?}")
    });
    assert!(
        matches!(target, TargetRepo::Simard),
        "an overseer gap brief routes to the default repo (rysweet/Simard): {src:?}"
    );
    assert_eq!(target.slug(), "rysweet/Simard");
}

#[test]
fn disabled_gap_scan_holds_the_whole_action() {
    let gaps = vec![sample_goal_gap()];
    let filed = Arc::new(Mutex::new(Vec::new()));
    let (notifier, email_log, signal_log) = dual_recording_notifier();

    // Identity + notifier wired; only the gap-scan enable flag is OFF.
    let mut ov = Overseer::new(caps_for_gaps(gaps, filed.clone()))
        .with_identity(overseer_identity())
        .with_gap_scan_enabled(false)
        .with_operator_notifier(Box::new(notifier));

    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.workstream_gaps_detected, 0,
        "disabled ⇒ no gap is flagged"
    );
    assert!(
        report.held >= 1,
        "the gap-scan action is held when disabled: {report:?}"
    );
    assert!(
        email_log.lock().unwrap().is_empty() && signal_log.lock().unwrap().is_empty(),
        "disabled ⇒ no notification is dispatched"
    );
    assert!(
        filed.lock().unwrap().is_empty(),
        "disabled ⇒ no issue is filed"
    );
}

#[test]
fn gap_scan_fails_closed_without_a_distinct_identity() {
    let gaps = vec![sample_goal_gap()];
    let filed = Arc::new(Mutex::new(Vec::new()));
    let (notifier, email_log, signal_log) = dual_recording_notifier();

    // No `.with_identity(...)` ⇒ the default RecursionGuard is unconfigured, so
    // the gap act must be REFUSED (fail closed) — the Overseer can never notify /
    // file on behalf of a gap without a DISTINCT steward identity.
    let mut ov = Overseer::new(caps_for_gaps(gaps, filed.clone()))
        .with_gap_scan_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let direct = ov.act(&Intervention::FlagWorkstreamGaps {
        gaps: vec![sample_goal_gap()],
    });
    assert!(
        matches!(direct, Err(OverseerError::Recursion { .. })),
        "an unconfigured identity refuses the gap act (fail closed): {direct:?}"
    );

    // A full tick isolates that refusal: it is counted, never notifies/files.
    let report = run_overseer_tick_isolated(&mut ov);
    assert_eq!(report.workstream_gaps_detected, 0);
    assert!(
        !report.panicked,
        "a fail-closed refusal never panics the tick"
    );
    assert!(
        report.errors >= 1,
        "the fail-closed refusal is counted, isolated: {report:?}"
    );
    assert!(
        email_log.lock().unwrap().is_empty() && signal_log.lock().unwrap().is_empty(),
        "no notification reached the operator without a distinct identity"
    );
    assert!(
        filed.lock().unwrap().is_empty(),
        "no issue was filed without a distinct identity"
    );
}

// ═══════════════════ 4. Notification factory + rendering ═════════════════════

#[test]
fn workstream_gap_notification_kind_subject_and_body() {
    let gaps = vec![sample_goal_gap(), sample_anomaly_gap()];
    let n = OperatorNotification::workstream_gap(gaps.len(), &gaps);

    assert_eq!(n.kind, "workstream-gap");
    assert!(
        n.autonomous,
        "a gap notification is an autonomous Overseer action"
    );

    // Subject reuses the shared renderer unchanged:
    // "[Overseer] workstream-gap: N uncovered workstream(s)".
    let subject = n.subject();
    assert!(
        subject.starts_with("[Overseer] workstream-gap:"),
        "subject: {subject}"
    );
    assert!(
        subject.contains('2'),
        "subject carries the count: {subject}"
    );

    // Body is the consolidated, provenance-labelled list — each gap says WHAT is
    // uncovered and WHY it matters.
    let body = n.plain_text();
    assert!(
        body.contains("Uncovered work:"),
        "body has the fixed 'Uncovered work:' heading: {body}"
    );
    for g in &gaps {
        assert!(
            body.contains(&g.ref_id),
            "body names each uncovered ref_id ({}): {body}",
            g.ref_id
        );
        assert!(
            body.contains(&g.why_it_matters),
            "body explains why each gap matters ({}): {body}",
            g.why_it_matters
        );
    }
}

// ═══════════════════ 5. Config helpers (opt-out + clamp) ═════════════════════

#[test]
fn gap_scan_enabled_is_opt_out_and_off_when_overseer_off() {
    // Default (unset): ON — the gap-scan runs whenever the acting Overseer runs.
    assert!(gap_scan_enabled_from(env(&[])));
    // Explicit falsey on the gap-scan flag: OFF.
    for falsey in ["0", "false", "no", "off", "OFF"] {
        assert!(
            !gap_scan_enabled_from(env(&[(SIMARD_OVERSEER_GAP_SCAN_ENV, falsey)])),
            "{falsey:?} disables the gap-scan"
        );
    }
    // A disabled acting Overseer forces the gap-scan OFF regardless of the flag.
    assert!(!gap_scan_enabled_from(env(&[
        (OVERSEER_ENABLED_ENV, "false"),
        (SIMARD_OVERSEER_GAP_SCAN_ENV, "on"),
    ])));
}

#[test]
fn gap_scan_every_n_defaults_to_one_and_clamps_to_floor() {
    // Unset ⇒ every tick.
    assert_eq!(gap_scan_every_n_from(env(&[])), 1);
    // Explicit value above the floor is honoured.
    assert_eq!(
        gap_scan_every_n_from(env(&[(SIMARD_OVERSEER_GAP_SCAN_EVERY_N_ENV, "4")])),
        4
    );
    // 0 / empty / garbage / negative ⇒ clamped to the floor of 1 (never disables
    // the scan by stealth, never divides by zero).
    for bad in ["0", "", "  ", "abc", "-3"] {
        assert!(
            gap_scan_every_n_from(env(&[(SIMARD_OVERSEER_GAP_SCAN_EVERY_N_ENV, bad)])) >= 1,
            "{bad:?} clamps to the floor of 1"
        );
    }
    assert_eq!(
        gap_scan_every_n_from(env(&[(SIMARD_OVERSEER_GAP_SCAN_EVERY_N_ENV, "0")])),
        1,
        "0 clamps to the floor of 1"
    );
}

// ═══════════════════ 6. decide / classify routing ═══════════════════════════

#[test]
fn decide_routes_workstream_coverage_to_flag_gaps() {
    let problem = Problem {
        kind: ProblemKind::WorkstreamCoverage,
        priority: Priority::High,
        dedup_key: "workstream-gap".to_string(),
        summary: "2 uncovered workstreams".to_string(),
        evidence: vec![Signal::WorkstreamGap {
            gaps: vec![sample_goal_gap(), sample_anomaly_gap()],
        }],
        why: None,
    };
    match decide(&problem) {
        Intervention::FlagWorkstreamGaps { gaps } => {
            assert_eq!(gaps.len(), 2, "decide carries the specific gaps forward");
        }
        other => panic!("expected FlagWorkstreamGaps, got {other:?}"),
    }
}

#[test]
fn flag_workstream_gaps_is_routine_and_admitted_by_default_gate() {
    let iv = Intervention::FlagWorkstreamGaps {
        gaps: vec![sample_goal_gap()],
    };
    assert_eq!(
        classify(&iv),
        RiskClass::Routine,
        "flagging gaps (notify + deduped file) is a routine action"
    );
    assert!(
        AutonomyGate::default().admit(&iv).is_ok(),
        "the routine gap action is admitted; its own identity/dedup gates apply in act"
    );
    assert_eq!(iv.label(), "flag_workstream_gaps");
}

// ═══════════════════ 7. humanize_tick rendering ═════════════════════════════

#[test]
fn tick_render_shows_flagged_gap_clause_and_a_clean_scan_adds_none() {
    let flagged = OverseerTickReport {
        problems: 3,
        workstream_gaps_detected: 2,
        ..OverseerTickReport::default()
    };
    let line = humanize_tick(&flagged);
    assert!(
        line.contains("flagged 2 workstream gaps"),
        "a flagged-gap tick renders the dedicated clause: {line}"
    );

    // A clean scan adds NO clause — a clean board is the honest observing state,
    // never a fabricated "0 gaps" line.
    let clean = OverseerTickReport {
        problems: 0,
        ..OverseerTickReport::default()
    };
    let clean_line = humanize_tick(&clean);
    assert!(
        !clean_line.contains("workstream gap"),
        "a clean scan fabricates no gap clause: {clean_line}"
    );
    assert!(
        clean_line.contains("observing"),
        "a clean board is the honest 'observing' state: {clean_line}"
    );
}
