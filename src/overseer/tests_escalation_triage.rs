//! TDD (RED) tests for the agentic **escalation-triage + course-correction**
//! surface (issue #4276). These are written BEFORE the implementation and
//! therefore reference API that does not exist yet — the crate test build will
//! FAIL to compile until the feature lands. That compile failure IS the red
//! state of red→green→refactor; every assertion below is the executable
//! contract the implementation must satisfy.
//!
//! Binding principle under test (Simard guideline **G3: agentic over brittle
//! heuristics**): a blocked-goal escalation must NOT stop at emitting a raw
//! machine marker (`🔒 [OODA-SAFEGUARD] … why=UNCLEAR-CRITERIA evidence=[…]`) to
//! a human. Instead, exactly like `self_diagnose.md` does for a StepFailure, a
//! THIN Rust trigger hands the escalation off to an agentic recipe
//! (`prompt_assets/simard/overseer/escalation_triage.md`) that:
//!
//! 1. restates the problem in PLAIN ENGLISH (no jargon tokens),
//! 2. recommends a concrete NEXT STEP,
//! 3. attempts a ROOT-CAUSE + COURSE-CORRECTION decision (rewrite an
//!    unmeasurable done-gate, complete a goal already delivered by a merged PR,
//!    or ask the operator ONE specific plain-English question), and
//! 4. emits a jargon-free Signal message per reasoning step.
//!
//! The Rust part stays a thin trigger: it launches the recipe through the SAME
//! `RecipeLauncher` seam `self_diagnose` uses and consumes ONLY the
//! `WorkstreamHandle` (never brittle-parses recipe stdout). The
//! escalate-vs-course-correct DECISION is owned by the recipe, not by a bare
//! integer threshold.
//!
//! Everything is hermetic: injected capability fakes + recording channels. No
//! network, no `~/.simard`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;

use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator,
    InFlightItem, IssueOutcome, MeetingHost, ObservedState, OrchestratorRunBrief, OverseerError,
    PrOps, RecipeBrief, RecipeLauncher, StatusReader, VerifyReport, WorkstreamHandle,
    WorkstreamStatus,
};
use crate::overseer::intervention::Intervention;
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::wiring::overseer_identity;
use crate::overseer::{ActOutcome, Capabilities, Overseer};

// ════════════════════════════════════════════════════════════════════════════
// Shared jargon tokens the operator must NEVER see in plain-English output.
// ════════════════════════════════════════════════════════════════════════════

/// Machine markers / internal tokens that the plain-English problem + next-step
/// (and the operator notification body) must be free of. This is the concrete
/// "no OODA-SAFEGUARD/UNCLEAR-CRITERIA jargon" contract from the issue.
const JARGON_TOKENS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "evidence=[",
    "why=",
    "\u{1F512}", // the 🔒 lock marker
];

fn assert_free_of_jargon(text: &str, context: &str) {
    for token in JARGON_TOKENS {
        assert!(
            !text.contains(token),
            "{context} must be plain English, but contains jargon token {token:?}: {text:?}"
        );
    }
}

// ─────────────────────────── capability fakes ──────────────────────────────

struct FakeStatus(ObservedState);
impl StatusReader for FakeStatus {
    fn snapshot(&self) -> Result<ObservedState, OverseerError> {
        Ok(self.0.clone())
    }
}

/// A `RecipeLauncher` that CAPTURES every launched brief so a test can prove the
/// escalation seam handed off to the agentic triage recipe (and passed the
/// structured context), and can count launches to prove in-flight dedup.
struct CapturingRecipes {
    launched: Arc<Mutex<Vec<RecipeBrief>>>,
}
impl CapturingRecipes {
    fn new() -> (Self, Arc<Mutex<Vec<RecipeBrief>>>) {
        let launched = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                launched: launched.clone(),
            },
            launched,
        )
    }
}
impl RecipeLauncher for CapturingRecipes {
    fn launch(&self, b: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        self.launched.lock().unwrap().push(b.clone());
        Ok(WorkstreamHandle {
            id: "ws-triage".to_string(),
        })
    }
    fn poll(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        // Still running ⇒ the in-flight dedup slot stays held, so a re-escalation
        // of the same goal within the same tick window is suppressed.
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

/// A goal store serving no blocked goals (the tests drive `act` directly).
struct FakeGoalStore;
impl GoalCurator for FakeGoalStore {
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

/// Capabilities wired with a capturing recipe launcher so the escalation seam's
/// hand-off to the agentic triage recipe is observable. Returns the shared
/// capture log alongside the capabilities.
fn caps_capturing() -> (Capabilities, Arc<Mutex<Vec<RecipeBrief>>>) {
    let (recipes, launched) = CapturingRecipes::new();
    let caps = Capabilities {
        status: Box::new(FakeStatus(ObservedState::default())),
        recipes: Box::new(recipes),
        prs: Box::new(FakePrs),
        deployer: Box::new(FakeDeployer),
        meetings: Box::new(FakeMeetings),
        issues: Box::new(FakeIssues),
        goals: Box::new(FakeGoalStore),
        auditor: Box::new(FakeAuditor),
        memory: Box::new(crate::overseer::capabilities::InertMemoryRecall),
    };
    (caps, launched)
}

// ─────────────────────────── sample builders ───────────────────────────────

/// A representative blocked-goal escalation intervention carrying the NEW
/// plain-English fields the feature adds: a jargon-free `problem`, a concrete
/// `next_step`, and an optional `link` to the tracking issue that already holds
/// the detail (fail-open: `None` is legal).
fn sample_escalation() -> Intervention {
    Intervention::EscalateBlockedGoal {
        goal_id: "feature-x".to_string(),
        reason: no_progress_blocked_reason(4),
        why: "brain-failure safeguard tripped 4× — reasoner regression".to_string(),
        problem: "Goal \"feature-x\" has been stuck for 4 cycles because its \
                  done-criteria can't be measured automatically, so Simard can't \
                  tell when it is finished."
            .to_string(),
        next_step: "Rewrite the goal's done-criteria as a machine-checkable gate \
                    (e.g. \"all tests in suite Y pass\"), or confirm it was already \
                    delivered by a merged PR."
            .to_string(),
        link: Some("https://github.com/rysweet/Simard/issues/4001".to_string()),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Section A — Intervention::EscalateBlockedGoal carries plain-English fields
// ════════════════════════════════════════════════════════════════════════════

/// The escalation intervention gains a plain-English `problem`, a concrete
/// `next_step`, and an optional tracking-issue `link` — the payload the operator
/// ultimately sees — WITHOUT changing its stable label.
#[test]
fn escalate_blocked_goal_carries_problem_next_step_and_link() {
    let iv = sample_escalation();
    match &iv {
        Intervention::EscalateBlockedGoal {
            problem,
            next_step,
            link,
            ..
        } => {
            assert!(!problem.is_empty(), "a plain-English problem is carried");
            assert!(!next_step.is_empty(), "a concrete next step is carried");
            assert_free_of_jargon(problem, "escalation problem");
            assert_free_of_jargon(next_step, "escalation next_step");
            assert_eq!(
                link.as_deref(),
                Some("https://github.com/rysweet/Simard/issues/4001"),
                "the tracking-issue link is threaded through the intervention"
            );
        }
        other => panic!("expected EscalateBlockedGoal, got {other:?}"),
    }
    // The stable label used for gating/telemetry/dedup is unchanged.
    assert_eq!(iv.label(), "escalate_blocked_goal");
}

// ════════════════════════════════════════════════════════════════════════════
// Section B — OperatorNotification: plain-English body, next step, no bad heading
// ════════════════════════════════════════════════════════════════════════════

/// A blocked-goal / escalation notification must NEVER render under the
/// merge/deploy "Problem solved:" heading — that template is wrong for an
/// unresolved, action-needed escalation (the reported bug). It uses an accurate
/// heading and surfaces the recommended NEXT STEP in the body.
#[test]
fn goal_blocked_notification_never_renders_problem_solved() {
    let n = OperatorNotification::goal_blocked_triaged(
        "feature-x",
        "Goal \"feature-x\" is stuck: its finish condition can't be checked automatically.",
        "Rewrite the finish condition so a test can confirm it, or close the goal if a \
         merged PR already delivered it.",
        Some("https://github.com/rysweet/Simard/issues/4001"),
    );
    let body = n.plain_text();
    assert!(
        !body.contains("Problem solved:"),
        "an unresolved escalation must not claim the problem is solved: {body:?}"
    );
    assert!(
        body.to_lowercase().contains("action needed") || body.to_lowercase().contains("blocked"),
        "the heading must accurately signal a blocked goal needing action: {body:?}"
    );
    // The recommended next step reaches the operator in the body.
    assert!(
        body.contains("Rewrite the finish condition"),
        "the recommended next step is surfaced to the operator: {body:?}"
    );
    // The tracking-issue link that holds the detail is present.
    assert!(
        body.contains("https://github.com/rysweet/Simard/issues/4001"),
        "the tracking-issue link is surfaced: {body:?}"
    );
    assert_free_of_jargon(&body, "goal-blocked notification body");
}

/// Regression guard: the heading fix must NOT change the merge notification,
/// whose "Problem solved:" heading is correct (the PR genuinely solved it).
#[test]
fn merge_notification_still_renders_problem_solved() {
    let merge = OperatorNotification {
        kind: "merge",
        headline: "fix(x): thing".to_string(),
        problem: "the flaky retry loop".to_string(),
        next_step: String::new(),
        link: Some("https://github.com/rysweet/Simard/pull/9".to_string()),
        repo: "rysweet/Simard".to_string(),
        autonomous: true,
    };
    assert!(
        merge.plain_text().contains("Problem solved:"),
        "a merge still legitimately reports the problem as solved"
    );
}

/// Regression guard: the deploy notification (which shares the generic template)
/// keeps its "Problem solved:" heading too.
#[test]
fn deploy_notification_still_renders_problem_solved() {
    let deploy = OperatorNotification::deploy(
        "abcdef1234567890",
        "0011223344556677",
        "rysweet/Simard",
        "canary green (4/4 gates)",
    );
    assert!(
        deploy.plain_text().contains("Problem solved:"),
        "a deploy still legitimately reports the problem as solved"
    );
}

/// The plain-English builder threads the operator-facing next step onto the
/// notification's dedicated `next_step` field (not smuggled inside `problem`),
/// and sets `link` to the tracking issue.
#[test]
fn goal_blocked_triaged_populates_next_step_field_and_link() {
    let n = OperatorNotification::goal_blocked_triaged(
        "research",
        "Goal \"research\" keeps getting parked because it never ships a bounded chunk.",
        "Ask Simard to carve one shippable sub-goal with a testable finish line.",
        None,
    );
    assert_eq!(
        n.next_step,
        "Ask Simard to carve one shippable sub-goal with a testable finish line."
    );
    assert!(
        n.link.is_none(),
        "a fail-open None link is legal — the escalation still fires"
    );
    assert_free_of_jargon(&n.problem, "triaged problem");
    assert_free_of_jargon(&n.next_step, "triaged next_step");
}

// ════════════════════════════════════════════════════════════════════════════
// Section C — the act seam is a THIN agentic trigger, not a Rust decider
// ════════════════════════════════════════════════════════════════════════════

/// On escalation the Overseer hands off to the agentic triage recipe through the
/// SAME `RecipeLauncher` seam `self_diagnose` uses — it does not stop at a
/// notify-and-count. The launched brief references `escalation_triage.md` and
/// carries the structured context (goal id + plain-English problem) so the agent
/// can reason; the Overseer consumes only the `WorkstreamHandle`.
#[test]
fn act_escalate_blocked_goal_launches_the_triage_recipe() {
    let (caps, launched) = caps_capturing();
    let mut ov = Overseer::new(caps)
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let out = ov.act(&sample_escalation());
    assert!(out.is_ok(), "the escalation act succeeds: {out:?}");

    let briefs = launched.lock().unwrap();
    assert_eq!(
        briefs.len(),
        1,
        "the escalation launched exactly one agentic triage workstream"
    );
    let brief = &briefs[0];
    assert!(
        brief.task_description.contains("escalation_triage.md"),
        "the brief points the agent at the escalation-triage recipe: {:?}",
        brief.task_description
    );
    assert!(
        brief.task_description.contains("feature-x"),
        "the goal id is passed as structured context to the recipe: {:?}",
        brief.task_description
    );
    assert_eq!(
        brief.target_repo, "rysweet/Simard",
        "the triage runs against the Simard repo"
    );
}

/// The recursion / distinct-identity guard still fails CLOSED: with no configured
/// steward identity the escalation refuses to run and launches NOTHING (no recipe,
/// no notification) — the anti-recursion invariant survives the rewrite.
#[test]
fn act_escalate_blocked_goal_fails_closed_without_identity() {
    let (caps, launched) = caps_capturing();
    let (notifier, email_log, signal_log) = dual_recording_notifier();
    // No `.with_identity(...)` ⇒ the default guard is unconfigured.
    let mut ov = Overseer::new(caps)
        .with_goal_health_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let out = ov.act(&sample_escalation());
    assert!(
        matches!(out, Err(OverseerError::Recursion { .. })),
        "an unconfigured identity refuses the escalation (fail closed): {out:?}"
    );
    assert!(
        launched.lock().unwrap().is_empty(),
        "no triage recipe was launched"
    );
    assert!(
        email_log.lock().unwrap().is_empty() && signal_log.lock().unwrap().is_empty(),
        "no notification reached the operator"
    );
}

/// A re-escalation of the SAME goal while its triage is still in flight must not
/// launch a second workstream — the in-flight dedup set (the same mechanism the
/// recipe-launch path already uses) holds the duplicate. This proves the
/// escalate-vs-course-correct authority moved to the recipe WITHOUT losing the
/// dedup guard that keeps a flapping goal from spawning triage every cycle.
#[test]
fn act_escalate_blocked_goal_deduped_while_triage_in_flight() {
    let (caps, launched) = caps_capturing();
    let mut ov = Overseer::new(caps)
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let _ = ov.act(&sample_escalation());
    let _ = ov.act(&sample_escalation());

    assert_eq!(
        launched.lock().unwrap().len(),
        1,
        "the second escalation of an already-in-flight goal launches no second triage"
    );
}

/// The operator's notification on escalation is PLAIN ENGLISH end-to-end: the
/// body carries the human-readable problem + recommended next step and is free
/// of every machine-marker token — never the raw `🔒 [OODA-SAFEGUARD]…` string.
#[test]
fn act_escalate_blocked_goal_notifies_operator_in_plain_english() {
    let (caps, _launched) = caps_capturing();
    let (notifier, email_log, signal_log) = dual_recording_notifier();
    let mut ov = Overseer::new(caps)
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true)
        .with_operator_notifier(Box::new(notifier));

    let out = ov.act(&sample_escalation());
    assert!(out.is_ok(), "the escalation act succeeds: {out:?}");

    for (chan, log) in [("email", &email_log), ("signal", &signal_log)] {
        let seen = log.lock().unwrap();
        assert_eq!(
            seen.len(),
            1,
            "the {chan} channel received the plain-English escalation"
        );
        let n = &seen[0];
        let body = n.plain_text();
        assert!(
            !body.contains("Problem solved:"),
            "the {chan} escalation must not claim the problem is solved: {body:?}"
        );
        assert!(
            body.contains("done-criteria") || body.contains("stuck") || body.contains("finished"),
            "the {chan} body restates the problem in plain English: {body:?}"
        );
        assert_free_of_jargon(&body, &format!("{chan} escalation body"));
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Section D — the escalation outcome is not a bare integer decision
// ════════════════════════════════════════════════════════════════════════════

/// Whatever `act` returns for an escalation, it must be the agentic-launch
/// outcome — the recipe now owns the escalate-vs-course-correct decision, so the
/// Overseer's act is a launch, not a terminal "escalated & counted" no-op.
#[test]
fn escalation_act_yields_an_agentic_launch_outcome() {
    let (caps, _launched) = caps_capturing();
    let mut ov = Overseer::new(caps)
        .with_identity(overseer_identity())
        .with_goal_health_enabled(true);

    let out = ov
        .act(&sample_escalation())
        .expect("the escalation act succeeds");
    assert!(
        matches!(out, ActOutcome::Launched(_)),
        "the escalation launches an agentic triage workstream (the recipe decides \
         escalate-vs-course-correct), rather than a bare notify-and-count: {out:?}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section E — the agentic recipe asset exists and mirrors self_diagnose.md
// ════════════════════════════════════════════════════════════════════════════

fn read_asset(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected asset {rel} to exist and be readable: {e}"))
}

/// The escalation-triage recipe asset exists, mirrors `self_diagnose.md`'s
/// structure (ROLE / INPUTS / OUTPUT), and — as agentic reasoning steps —
/// instructs the agent to produce (a) a plain-English problem, (b) a recommended
/// next step, (c) a root-cause + course-correction decision offering the three
/// concrete options, and (d) a jargon-free Signal message per step.
#[test]
fn escalation_triage_asset_exists_and_specifies_the_agentic_contract() {
    let asset = read_asset("prompt_assets/simard/overseer/escalation_triage.md");
    let lower = asset.to_lowercase();

    // Mirrors the self_diagnose structure.
    for section in ["## role", "## inputs", "## output"] {
        assert!(
            lower.contains(section),
            "the recipe must have a {section:?} section like self_diagnose.md"
        );
    }

    // (a) plain-English problem statement, (b) recommended next step.
    assert!(
        lower.contains("plain english") || lower.contains("plain-english"),
        "the recipe must demand a PLAIN-ENGLISH problem statement"
    );
    assert!(
        lower.contains("next step"),
        "the recipe must demand a concrete recommended NEXT STEP"
    );

    // (c) root-cause + the three course-correction options.
    assert!(
        lower.contains("root cause") || lower.contains("root-cause"),
        "the recipe must attempt a ROOT CAUSE"
    );
    assert!(
        lower.contains("done") && (lower.contains("machine") || lower.contains("measurable")),
        "the recipe must offer 'rewrite an unmeasurable done-gate to be machine-checkable'"
    );
    assert!(
        lower.contains("merged pr")
            || lower.contains("merged-pr")
            || lower.contains("already delivered"),
        "the recipe must offer 'complete a goal already delivered by a merged PR'"
    );
    assert!(
        lower.contains("one") && lower.contains("question"),
        "the recipe must offer 'ask the operator ONE specific plain-English question'"
    );

    // (d) a jargon-free Signal message per reasoning step.
    assert!(
        lower.contains("signal"),
        "the recipe must instruct a per-step plain-English Signal message"
    );

    // The recipe must forbid the anti-patterns the issue calls out.
    assert!(
        lower.contains("bridge"),
        "the recipe must carry the standing 'no Bridge naming' rule (mirrors self_diagnose)"
    );
}

/// The recipe must NOT tell the agent to emit the raw machine markers verbatim —
/// it exists precisely to TRANSLATE them. It may name a token only to say
/// "translate/strip it", so we assert the translation intent is present rather
/// than banning the token outright.
#[test]
fn escalation_triage_asset_demands_translation_not_marker_passthrough() {
    let asset = read_asset("prompt_assets/simard/overseer/escalation_triage.md");
    let lower = asset.to_lowercase();
    assert!(
        lower.contains("translate")
            || lower.contains("plain english")
            || lower.contains("plain-english")
            || lower.contains("no jargon")
            || lower.contains("jargon-free"),
        "the recipe must direct the agent to translate machine markers into plain English"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Section F — durable code-atlas documentation
// ════════════════════════════════════════════════════════════════════════════

/// The durable escalation-flow atlas exists and carries BOTH required Mermaid
/// diagrams (the escalation data-flow and the overseer-tick recipe-vs-code view).
/// It must be a durable architecture doc, not a point-in-time report.
#[test]
fn escalation_flow_atlas_has_both_mermaid_diagrams() {
    let atlas = read_asset("docs/atlas/escalation-flow/README.md");
    let mermaid_blocks = atlas.matches("```mermaid").count();
    assert!(
        mermaid_blocks >= 2,
        "the atlas must contain at least two Mermaid diagrams (data-flow + \
         overseer-tick recipe-vs-code), found {mermaid_blocks}"
    );
    let lower = atlas.to_lowercase();
    assert!(
        lower.contains("escalation_triage.md") || lower.contains("escalation triage"),
        "the atlas documents the new agentic triage recipe seam"
    );
    assert!(
        lower.contains("recipe") && lower.contains("overseer"),
        "the atlas documents the recipe-vs-code overseer-tick boundary"
    );
}
