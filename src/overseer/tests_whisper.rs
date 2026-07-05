//! TDD tests (written FIRST) for the **Simard Whisperer** — the Overseer's
//! lightweight steering channel (issue #2605).
//!
//! These tests specify the *contract* for a feature that does not yet exist;
//! they are the red half of the TDD cycle and pin down the public surface the
//! implementation must provide:
//!
//! - `overseer::whisper_ops`: [`WhisperUrgency`], [`WhisperRecord`],
//!   [`WhisperSink`], [`MeetingHandoffWhisperSink`], [`compose_whisper_note`],
//!   [`whisper_signature`].
//! - `Intervention::Whisper { note, urgency }` + `label()` + `classify` = Routine.
//! - `ProblemKind::{LoopDetected, DriftCorrection}` + matching `Signal`s and
//!   `signals_from` arms (loop threshold 2, strictly below the no-progress
//!   breaker threshold 3).
//! - `ObservedState.{consecutive_no_action, active_goal_id, drift_detail}`.
//! - `guardrails::{WhisperGate, WhisperDecision}` (dedup window + per-hour cap,
//!   injected clock).
//! - `config::{whisper_enabled_from, SIMARD_OVERSEER_WHISPER_ENV}` (opt-out gate,
//!   off whenever the Overseer itself is off).
//! - `Overseer::{with_whisper_sink, with_whisper_enabled}`, `act(Whisper)`,
//!   `ActOutcome::{Whispered, WhisperSuppressed}`, and `OverseerTickReport
//!   .{whispers, whispers_suppressed}`.
//! - `notify::OperatorNotification::whisper(...)`.
//! - OODA ingest: `ooda_loop::drain_overseer_whispers` folds the note into
//!   Simard's next-cycle context and marks the handoff processed; a whisper is
//!   never turned into a goal by `check_meeting_handoffs` (advisory only).
//!
//! Everything is exercised with injected fakes (observed-state, whisper sink,
//! clock, identity) and a `tempfile` inbox — no network, no `~/.simard`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::goal_curation::GoalBoard;
use crate::goal_curation::no_progress_breaker::NO_PROGRESS_BREAKER_THRESHOLD;
use crate::meeting_facilitator::load_meeting_handoff;

use crate::overseer::capabilities::{
    AuditReport, AuditScope, Auditor, DeployReport, Deployer, GoalBrief, GoalCurator, InFlightItem,
    IssueOutcome, MeetingHost, ObservedState, OrchestratorRunBrief, OverseerError, PrOps,
    RecipeBrief, RecipeLauncher, StatusReader, VerifyReport, WorkstreamHandle, WorkstreamStatus,
};
use crate::overseer::config::{
    SIMARD_OVERSEER_WHISPER_ENV, overseer_author_login, whisper_enabled_from,
};
use crate::overseer::guardrails::{
    AutonomyGate, RiskClass, WhisperDecision, WhisperGate, classify,
};
use crate::overseer::intervention::Intervention;
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::signal::{
    Priority, Problem, ProblemKind, Signal, WHISPER_LOOP_THRESHOLD, signals_from,
};
use crate::overseer::whisper_ops::{
    MeetingHandoffWhisperSink, WhisperRecord, WhisperSink, WhisperUrgency, compose_whisper_note,
    whisper_signature,
};
use crate::overseer::wiring::{overseer_identity, overseer_tick, run_overseer_tick_isolated};
use crate::overseer::{ActOutcome, Capabilities, Overseer, decide};

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

struct FakeGoals(Vec<InFlightItem>);
impl GoalCurator for FakeGoals {
    fn propose(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
        Ok(())
    }
    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
        Ok(self.0.clone())
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

/// Records every goal handed to the meeting host so escalation is observable
/// without a REPL or filesystem — the shared log is returned alongside so a test
/// can inspect it after the recorder is boxed into the Overseer.
struct RecordingMeetings {
    log: Arc<Mutex<Vec<GoalBrief>>>,
}
impl RecordingMeetings {
    fn new() -> (Self, Arc<Mutex<Vec<GoalBrief>>>) {
        let log: Arc<Mutex<Vec<GoalBrief>>> = Arc::new(Mutex::new(vec![]));
        (Self { log: log.clone() }, log)
    }
}
impl MeetingHost for RecordingMeetings {
    fn transfer_goal(&self, g: &GoalBrief) -> Result<(), OverseerError> {
        self.log.lock().unwrap().push(g.clone());
        Ok(())
    }
}

/// Captures delivered whisper records; can be told to fail or panic so the
/// panic-isolated tick can be proven to survive a bad whisper capability.
struct RecordingWhisperSink {
    log: Arc<Mutex<Vec<WhisperRecord>>>,
    fail: bool,
    panic: bool,
}
impl RecordingWhisperSink {
    fn new() -> (Self, Arc<Mutex<Vec<WhisperRecord>>>) {
        let log: Arc<Mutex<Vec<WhisperRecord>>> = Arc::new(Mutex::new(vec![]));
        (
            Self {
                log: log.clone(),
                fail: false,
                panic: false,
            },
            log,
        )
    }
    fn failing() -> Self {
        Self {
            log: Arc::new(Mutex::new(vec![])),
            fail: true,
            panic: false,
        }
    }
    fn panicking() -> Self {
        Self {
            log: Arc::new(Mutex::new(vec![])),
            fail: false,
            panic: true,
        }
    }
}
impl WhisperSink for RecordingWhisperSink {
    fn deliver(&self, rec: &WhisperRecord) -> Result<PathBuf, OverseerError> {
        if self.panic {
            panic!("boom: whisper sink blew up mid-deliver");
        }
        if self.fail {
            return Err(OverseerError::Capability {
                what: "whisper.deliver",
                detail: "inbox unwritable".to_string(),
            });
        }
        let mut log = self.log.lock().unwrap();
        log.push(rec.clone());
        Ok(PathBuf::from(format!("/tmp/whisper-{}.json", log.len())))
    }
}

/// A notify channel that records every notification and always reports `Sent`.
struct RecordingChannel {
    name: String,
    seen: Mutex<Vec<OperatorNotification>>,
}
impl RecordingChannel {
    fn sent(name: &str) -> Self {
        Self {
            name: name.to_string(),
            seen: Mutex::new(vec![]),
        }
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

fn caps(observed: ObservedState, meetings: Box<dyn MeetingHost>) -> Capabilities {
    Capabilities {
        status: Box::new(FakeStatus(observed)),
        recipes: Box::new(FakeRecipes),
        prs: Box::new(FakePrs),
        deployer: Box::new(FakeDeployer),
        meetings,
        issues: Box::new(FakeIssues),
        goals: Box::new(FakeGoals(vec![])),
        auditor: Box::new(FakeAuditor),
    }
}

/// An `ObservedState` with a live goal looping for `n` consecutive no-action
/// cycles — the primary whisper trigger.
fn looping(n: u32) -> ObservedState {
    ObservedState {
        consecutive_no_action: Some(n),
        active_goal_id: Some("g1".to_string()),
        ..ObservedState::default()
    }
}

// ─────────────────── 1. Signal derivation (Observe → Signal) ────────────────

#[test]
fn loop_signal_fires_at_and_above_the_whisper_threshold_only() {
    // The whisper must intervene BEFORE Simard's no-progress breaker trips. This
    // is a compile-time relationship between two constants; the assertion
    // documents and pins the invariant (clippy would otherwise flag it as a
    // constant assertion).
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(
            WHISPER_LOOP_THRESHOLD < NO_PROGRESS_BREAKER_THRESHOLD,
            "the whisper loop threshold ({WHISPER_LOOP_THRESHOLD}) must sit strictly below the breaker ({NO_PROGRESS_BREAKER_THRESHOLD})"
        );
    }

    // One below the fence: no loop signal.
    let below = looping(1);
    assert!(
        !signals_from(&below)
            .iter()
            .any(|s| matches!(s, Signal::LoopDetected { .. })),
        "one no-action cycle is below the whisper fence"
    );

    // Exactly at the fence: a loop signal carrying the goal + count.
    let at = looping(2);
    let found = signals_from(&at).into_iter().find_map(|s| match s {
        Signal::LoopDetected {
            goal_id,
            consecutive_no_action,
        } => Some((goal_id, consecutive_no_action)),
        _ => None,
    });
    assert_eq!(found, Some(("g1".to_string(), 2)));
}

#[test]
fn loop_signal_needs_an_active_goal() {
    // No active goal ⇒ nothing to steer, so no loop whisper even if the counter
    // is high (idle churn is not a goal loop).
    let idle = ObservedState {
        consecutive_no_action: Some(5),
        active_goal_id: None,
        ..ObservedState::default()
    };
    assert!(
        !signals_from(&idle)
            .iter()
            .any(|s| matches!(s, Signal::LoopDetected { .. }))
    );
}

#[test]
fn drift_signal_fires_when_drift_detail_present() {
    let drifting = ObservedState {
        drift_detail: Some("editing unrelated module Y".to_string()),
        active_goal_id: Some("g1".to_string()),
        ..ObservedState::default()
    };
    let found = signals_from(&drifting).into_iter().find_map(|s| match s {
        Signal::DriftCorrection { goal_id, detail } => Some((goal_id, detail)),
        _ => None,
    });
    let (goal_id, detail) = found.expect("a drift signal must fire");
    assert_eq!(goal_id, "g1");
    assert!(detail.contains("unrelated"));
}

#[test]
fn a_default_snapshot_emits_no_whisper_signals() {
    let sigs = signals_from(&ObservedState::default());
    assert!(
        !sigs.iter().any(|s| matches!(
            s,
            Signal::LoopDetected { .. } | Signal::DriftCorrection { .. }
        )),
        "an idle Overseer never whispers"
    );
}

// ─────────────────── 2. Decide routing: whisper by default ──────────────────

fn loop_problem(n: u32) -> Problem {
    Problem {
        kind: ProblemKind::LoopDetected,
        priority: Priority::High,
        dedup_key: "loop:g1".to_string(),
        summary: format!("no action for {n} cycles on g1"),
        evidence: vec![Signal::LoopDetected {
            goal_id: "g1".to_string(),
            consecutive_no_action: n,
        }],
    }
}

#[test]
fn decide_routes_a_mild_loop_to_a_lightweight_whisper() {
    // Default steward action: a whisper, not a meeting, not a launch.
    match decide(&loop_problem(2)) {
        Intervention::Whisper { note, urgency } => {
            assert!(!note.is_empty(), "the whisper must carry a steering note");
            assert_eq!(
                urgency,
                WhisperUrgency::Normal,
                "a mild loop is a Normal-urgency whisper"
            );
        }
        other => panic!("expected a Whisper for a mild loop, got {other:?}"),
    }
}

#[test]
fn decide_routes_drift_to_a_lightweight_whisper() {
    let drift = Problem {
        kind: ProblemKind::DriftCorrection,
        priority: Priority::Normal,
        dedup_key: "drift:g1".to_string(),
        summary: "work drifting from goal g1".to_string(),
        evidence: vec![Signal::DriftCorrection {
            goal_id: "g1".to_string(),
            detail: "editing unrelated module Y".to_string(),
        }],
    };
    assert!(
        matches!(decide(&drift), Intervention::Whisper { .. }),
        "drift correction is delivered as an advisory whisper"
    );
}

#[test]
fn decide_escalates_an_acute_repeated_loop_to_a_meeting_transfer() {
    // Repeated/urgent: once the loop reaches the breaker threshold the whisper
    // was insufficient, so the Overseer escalates via the existing meeting path
    // (TransferGoal → MeetingHost), NOT another lightweight whisper.
    let acute = loop_problem(NO_PROGRESS_BREAKER_THRESHOLD + 1);
    assert!(
        matches!(decide(&acute), Intervention::TransferGoal { .. }),
        "an acute/repeated loop escalates to a meeting transfer"
    );
}

// ───────────────── 3. Intervention identity, label, risk class ──────────────

#[test]
fn whisper_intervention_has_a_stable_label() {
    let iv = Intervention::Whisper {
        note: "steer".to_string(),
        urgency: WhisperUrgency::Normal,
    };
    assert_eq!(
        iv.label(),
        "whisper",
        "the stable label the gate/tracing/dedup depend on"
    );
}

#[test]
fn a_whisper_is_routine_and_admitted_by_the_default_autonomy_gate() {
    let iv = Intervention::Whisper {
        note: "steer".to_string(),
        urgency: WhisperUrgency::Normal,
    };
    // A whisper takes no action on Simard's behalf and spends no budget, so it
    // is Routine: no HIGH-RISK / merge opt-in required.
    assert_eq!(classify(&iv), RiskClass::Routine);
    assert!(
        AutonomyGate::default().admit(&iv).is_ok(),
        "a whisper is admitted by the default gate (its own dedup/identity gates apply elsewhere)"
    );
}

// ─────────────────── 4. WhisperGate: dedup window + per-hour cap ────────────

#[test]
fn whisper_gate_suppresses_an_identical_whisper_within_the_window() {
    // 900s dedup window, generous cap.
    let mut gate = WhisperGate::new(900, 5);
    assert_eq!(gate.admit("sig-a", 0), WhisperDecision::Deliver);
    assert_eq!(
        gate.admit("sig-a", 300),
        WhisperDecision::SuppressDuplicate,
        "same signature 300s later is a duplicate"
    );
    assert_eq!(
        gate.admit("sig-a", 899),
        WhisperDecision::SuppressDuplicate,
        "still inside the 900s window"
    );
    assert_eq!(
        gate.admit("sig-a", 901),
        WhisperDecision::Deliver,
        "past the window the same signature may be re-delivered"
    );
    // A different signature is never a duplicate of the first.
    assert_eq!(gate.admit("sig-b", 902), WhisperDecision::Deliver);
}

#[test]
fn whisper_gate_caps_whispers_per_rolling_hour() {
    // Distinct signatures (so dedup never fires) exercise the per-hour cap of 3.
    let mut gate = WhisperGate::new(900, 3);
    assert_eq!(gate.admit("s1", 0), WhisperDecision::Deliver);
    assert_eq!(gate.admit("s2", 10), WhisperDecision::Deliver);
    assert_eq!(gate.admit("s3", 20), WhisperDecision::Deliver);
    assert_eq!(
        gate.admit("s4", 30),
        WhisperDecision::SuppressCapReached,
        "the 4th whisper within the hour is capped"
    );
    // After the rolling hour fully elapses, the budget frees.
    assert_eq!(
        gate.admit("s5", 7200),
        WhisperDecision::Deliver,
        "a new hour re-opens the whisper budget"
    );
}

// ─────────────────── 5. whisper_ops helpers: note + signature ───────────────

#[test]
fn whisper_signature_is_stable_under_trivial_note_differences() {
    let a = whisper_signature(
        ProblemKind::LoopDetected,
        Some("g1"),
        "Steer   TOWARD the goal intent",
    );
    let b = whisper_signature(
        ProblemKind::LoopDetected,
        Some("g1"),
        "steer toward the goal intent",
    );
    assert_eq!(a, b, "case + whitespace normalise to one dedup signature");

    // A different goal or a different problem kind is a different whisper.
    let other_goal = whisper_signature(
        ProblemKind::LoopDetected,
        Some("g2"),
        "steer toward the goal intent",
    );
    let other_kind = whisper_signature(
        ProblemKind::DriftCorrection,
        Some("g1"),
        "steer toward the goal intent",
    );
    assert_ne!(a, other_goal, "goal id discriminates the signature");
    assert_ne!(a, other_kind, "problem kind discriminates the signature");
}

#[test]
fn compose_whisper_note_is_deterministic_and_references_the_goal() {
    let problem = loop_problem(2);
    let state = looping(2);
    let note = compose_whisper_note(&problem, &state);
    assert!(!note.is_empty());
    assert!(
        note.contains("g1"),
        "a steering note identifies the goal it is steering: {note:?}"
    );
    assert_eq!(
        note,
        compose_whisper_note(&problem, &state),
        "note composition is pure/deterministic"
    );
}

// ─────────── 6. MeetingHandoffWhisperSink: advisory handoff on the inbox ─────

#[test]
fn whisper_sink_writes_an_advisory_handoff_on_the_shared_inbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = MeetingHandoffWhisperSink::new(dir.path().to_path_buf());
    let note = "Focus on the failing assertion in module X before broadening scope.";
    let rec = WhisperRecord {
        note: note.to_string(),
        urgency: WhisperUrgency::Normal,
        problem: ProblemKind::LoopDetected,
        goal_id: Some("g1".to_string()),
        author: overseer_author_login(),
        signature: whisper_signature(ProblemKind::LoopDetected, Some("g1"), note),
    };

    let path = sink.deliver(&rec).expect("deliver a whisper");
    assert!(path.exists(), "the handoff file is written");
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("handoff-") && name.ends_with(".json"),
        "delivered through the same handoff-*.json channel observe.rs scans: {name}"
    );
    assert!(
        path.starts_with(dir.path()),
        "written under the injected inbox dir, never elsewhere"
    );

    // The handoff is shaped so Simard's curate step can NEVER promote it into a
    // goal or backlog item — it is advisory context only.
    let h = load_meeting_handoff(dir.path())
        .expect("load handoff")
        .expect("a handoff exists");
    assert!(
        h.decisions.is_empty(),
        "no decisions ⇒ a whisper can never become a goal"
    );
    assert!(
        h.action_items.is_empty(),
        "no action items ⇒ a whisper can never become a backlog item"
    );
    assert!(
        h.open_questions
            .iter()
            .any(|q| q.text.contains("Focus on the failing assertion")),
        "the steering note rides in a non-promoting field"
    );
    assert!(
        h.themes.iter().any(|t| t == "overseer-whisper"),
        "tagged for recognition (and self-whisper skip)"
    );
    assert!(
        !h.processed,
        "delivered unprocessed so the OODA inbox scan folds it in"
    );
    assert!(
        h.participants.iter().any(|p| p == &overseer_author_login()),
        "authored under the Overseer's DISTINCT steward identity"
    );
}

// ─────────── 7. Overseer.act(Whisper): deliver, dedup, fail-closed ──────────

#[test]
fn act_delivers_a_whisper_then_dedups_an_identical_one() {
    let (sink, log) = RecordingWhisperSink::new();
    let (meetings, _t) = RecordingMeetings::new();
    let mut ov = Overseer::new(caps(ObservedState::default(), Box::new(meetings)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(sink));

    let iv = Intervention::Whisper {
        note: "Re-read the goal intent, then narrow to one file.".to_string(),
        urgency: WhisperUrgency::Normal,
    };

    let first = ov.act(&iv).expect("first act");
    assert!(
        matches!(first, ActOutcome::Whispered { .. }),
        "the first whisper is delivered"
    );

    let second = ov.act(&iv).expect("second act");
    assert!(
        matches!(second, ActOutcome::WhisperSuppressed { .. }),
        "an identical whisper within the window is suppressed, not re-injected"
    );

    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "the sink is touched exactly once for a deduped whisper"
    );
}

#[test]
fn act_fails_closed_when_the_steward_identity_is_unconfigured() {
    // No `.with_identity(...)` ⇒ the default RecursionGuard is unconfigured, so
    // the whisper must be REFUSED (fail closed) and the sink never called — the
    // Overseer can never whisper without a distinct steward identity.
    let (sink, log) = RecordingWhisperSink::new();
    let (meetings, _t) = RecordingMeetings::new();
    let mut ov = Overseer::new(caps(ObservedState::default(), Box::new(meetings)))
        .with_whisper_sink(Box::new(sink));

    let iv = Intervention::Whisper {
        note: "steer".to_string(),
        urgency: WhisperUrgency::Normal,
    };
    let out = ov.act(&iv);
    assert!(
        matches!(out, Err(OverseerError::Recursion { .. })),
        "unconfigured identity refuses the whisper (anti-recursion, fail closed): {out:?}"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "a refused whisper never reaches the sink"
    );
}

// ───────────────── 8. Tick: emit, dedup, escalate, config, isolate ──────────

#[test]
fn a_loop_condition_delivers_exactly_one_whisper_this_tick() {
    let (sink, log) = RecordingWhisperSink::new();
    let (meetings, transfers) = RecordingMeetings::new();
    let mut ov = Overseer::new(caps(looping(2), Box::new(meetings)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(sink))
        .with_whisper_enabled(true);

    let report = overseer_tick(&mut ov);
    assert_eq!(report.whispers, 1, "one advisory whisper delivered");
    assert_eq!(report.whispers_suppressed, 0);
    assert_eq!(report.errors, 0);
    assert!(!report.panicked);

    let delivered = log.lock().unwrap();
    assert_eq!(delivered.len(), 1);
    assert!(
        delivered[0].note.contains("g1"),
        "the delivered note steers the looping goal"
    );
    assert!(
        transfers.lock().unwrap().is_empty(),
        "the default path is a lightweight whisper, not a meeting"
    );
}

#[test]
fn an_identical_whisper_is_suppressed_on_the_next_tick() {
    let (sink, log) = RecordingWhisperSink::new();
    let (meetings, _t) = RecordingMeetings::new();
    let mut ov = Overseer::new(caps(looping(2), Box::new(meetings)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(sink))
        .with_whisper_enabled(true);

    let first = overseer_tick(&mut ov);
    assert_eq!(first.whispers, 1);

    // Same observed condition ⇒ same whisper ⇒ suppressed by the dedup window.
    let second = overseer_tick(&mut ov);
    assert_eq!(second.whispers, 0, "no re-injection every cycle");
    assert_eq!(second.whispers_suppressed, 1, "the duplicate is counted");

    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "delivered once across two ticks"
    );
}

#[test]
fn an_acute_loop_escalates_to_a_meeting_instead_of_whispering() {
    let observed = looping(NO_PROGRESS_BREAKER_THRESHOLD + 1);
    let (sink, log) = RecordingWhisperSink::new();
    let (meetings, transfers) = RecordingMeetings::new();
    let mut ov = Overseer::new(caps(observed, Box::new(meetings)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(sink))
        .with_whisper_enabled(true);

    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.whispers, 0,
        "an acute/repeated loop escalates rather than whispering"
    );
    assert_eq!(
        transfers.lock().unwrap().len(),
        1,
        "escalation transfers a goal via the existing meeting host"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "no lightweight whisper is delivered when escalating"
    );
}

#[test]
fn a_disabled_whisperer_emits_nothing_even_when_the_condition_holds() {
    let (sink, log) = RecordingWhisperSink::new();
    let (meetings, _t) = RecordingMeetings::new();
    let mut ov = Overseer::new(caps(looping(2), Box::new(meetings)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(sink))
        .with_whisper_enabled(false);

    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.whispers, 0,
        "SIMARD_OVERSEER_WHISPER disabled ⇒ no whisper emitted"
    );
    assert!(log.lock().unwrap().is_empty(), "the sink is never called");
}

#[test]
fn a_panicking_whisper_sink_is_isolated_and_the_overseer_survives() {
    let mut ov = Overseer::new(caps(looping(2), Box::new(RecordingMeetings::new().0)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(RecordingWhisperSink::panicking()))
        .with_whisper_enabled(true);

    let report = run_overseer_tick_isolated(&mut ov);
    assert!(
        report.panicked,
        "a panic in the whisper capability is caught by the isolated tick"
    );
    // The daemon keeps ticking: a second isolated tick also survives.
    let report2 = run_overseer_tick_isolated(&mut ov);
    assert!(report2.panicked);
}

#[test]
fn a_whisper_sink_error_is_counted_not_fatal() {
    let mut ov = Overseer::new(caps(looping(2), Box::new(RecordingMeetings::new().0)))
        .with_identity(overseer_identity())
        .with_whisper_sink(Box::new(RecordingWhisperSink::failing()))
        .with_whisper_enabled(true);

    let report = overseer_tick(&mut ov);
    assert!(!report.panicked, "an error is not a panic");
    assert!(
        report.errors >= 1,
        "a failed whisper delivery is counted, not propagated"
    );
    assert_eq!(report.whispers, 0, "nothing was delivered");
}

// ─────────────────── 9. Config: opt-out gate, off when Overseer off ─────────

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

#[test]
fn whisper_enabled_by_default_when_the_overseer_is_enabled() {
    // Unset everything: the acting Overseer defaults ON, so the whisperer does too.
    assert!(whisper_enabled_from(env_of(&[])));
    // Explicit truthy / garbage / empty leaves it ON (opt-out, not opt-in).
    for v in ["1", "true", "yes", "on", "", "maybe"] {
        assert!(
            whisper_enabled_from(env_of(&[(SIMARD_OVERSEER_WHISPER_ENV, v)])),
            "{v:?} must leave the whisperer enabled"
        );
    }
}

#[test]
fn whisper_disabled_by_explicit_falsey_flag() {
    for v in ["0", "false", "no", "off", "  off  "] {
        assert!(
            !whisper_enabled_from(env_of(&[(SIMARD_OVERSEER_WHISPER_ENV, v)])),
            "{v:?} must disable the whisperer"
        );
    }
}

#[test]
fn whisper_disabled_whenever_the_overseer_itself_is_disabled() {
    // Whispering only makes sense while the Overseer runs: an explicitly-disabled
    // Overseer forces the whisperer off regardless of the whisper flag.
    let disabled_overseer = env_of(&[
        ("SIMARD_OVERSEER_ENABLED", "false"),
        (SIMARD_OVERSEER_WHISPER_ENV, "true"),
    ]);
    assert!(
        !whisper_enabled_from(disabled_overseer),
        "no Overseer ⇒ no whisperer, even with the whisper flag on"
    );
}

// ─────────────────── 10. Operator notification (observability) ──────────────

#[test]
fn a_delivered_whisper_is_surfaced_to_the_operator() {
    let n = OperatorNotification::whisper(
        "Re-read the goal intent before broadening scope.",
        "loop_detected",
        WhisperUrgency::Normal,
        "g1",
    );
    assert_eq!(n.kind, "whisper", "a distinct notification kind");
    assert!(
        n.problem.contains("Re-read the goal intent"),
        "the steering note is carried to the operator"
    );
    assert!(
        n.subject().contains("whisper"),
        "the subject names the whisper: {}",
        n.subject()
    );
    assert!(n.plain_text().contains("Re-read the goal intent"));

    // Surfaced through the MANDATORY dual channel — never a hidden side-channel.
    let notifier = DualChannelNotifier::new(vec![
        Box::new(RecordingChannel::sent("email")),
        Box::new(RecordingChannel::sent("signal")),
    ]);
    let report = notifier.notify(&n);
    assert!(report.dispatched(), "the whisper notification fires");
    assert!(report.all_sent());
}

// ─────────── 11. Integration: the whisper reaches Simard's next cycle ───────

#[test]
fn a_delivered_whisper_is_folded_into_the_next_ooda_cycle_once() {
    // Deliver a real whisper handoff onto a tempfile inbox …
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = MeetingHandoffWhisperSink::new(dir.path().to_path_buf());
    let note = "Prefer the smallest reproduction; stop re-running the full suite.";
    let rec = WhisperRecord {
        note: note.to_string(),
        urgency: WhisperUrgency::Normal,
        problem: ProblemKind::LoopDetected,
        goal_id: Some("g1".to_string()),
        author: overseer_author_login(),
        signature: whisper_signature(ProblemKind::LoopDetected, Some("g1"), note),
    };
    sink.deliver(&rec).expect("deliver");

    // … the OODA cycle-start ingest folds the note into the reasoner-facing
    // context (the inbox observe.rs scans) and marks it processed.
    let notes = crate::ooda_loop::drain_overseer_whispers(dir.path()).expect("drain whispers");
    assert_eq!(notes.len(), 1, "the note reaches the next cycle's context");
    assert!(notes[0].contains("smallest reproduction"));

    // A whisper is folded exactly once — never re-injected every cycle.
    let again = crate::ooda_loop::drain_overseer_whispers(dir.path()).expect("drain again");
    assert!(
        again.is_empty(),
        "the whisper is processed after ingest, not re-injected"
    );
}

#[test]
fn a_whisper_is_advisory_and_never_becomes_a_goal() {
    // Curate must never promote a whisper into a goal or backlog item: reasoners
    // still decide; the whisper is only additional context.
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = MeetingHandoffWhisperSink::new(dir.path().to_path_buf());
    let note = "Consider whether the goal is already satisfied by PR #42.";
    let rec = WhisperRecord {
        note: note.to_string(),
        urgency: WhisperUrgency::Normal,
        problem: ProblemKind::DriftCorrection,
        goal_id: Some("g1".to_string()),
        author: overseer_author_login(),
        signature: whisper_signature(ProblemKind::DriftCorrection, Some("g1"), note),
    };
    sink.deliver(&rec).expect("deliver");

    let mut board = GoalBoard::new();
    let created = crate::ooda_loop::check_meeting_handoffs(&mut board, dir.path(), dir.path())
        .expect("curate handoffs");
    assert_eq!(created, 0, "a whisper fabricates no goal / backlog item");
    assert!(
        board.active.is_empty(),
        "no active goal created from a whisper"
    );
    assert!(
        board.backlog.is_empty(),
        "no backlog item created from a whisper"
    );
}
