//! TDD contract tests for the Overseer's **cognitive-memory recall** surface
//! (issue #2628). These are written **first** (red): they pin the not-yet-built
//! contract from the design/reference docs so the implementation has an exact,
//! executable specification. They MUST fail (compile-fail on the missing API,
//! then assert-fail) until the feature is implemented, and MUST pass once it is.
//!
//! Contract under test (see `docs/reference/overseer-memory-recall-api.md`):
//!
//! - A single additive [`MemoryRecall`] capability trait + owned-`String` result
//!   types (`RecallKeys`, `RecallBudget`, `MemorySnapshot`, `RecalledFact` /
//!   `RecalledEpisode` / `RecalledProcedure` / `RecalledProspective`,
//!   `ObservationEpisode`, `RecordOutcome`) live in `capabilities.rs`, alongside
//!   the `sanitize_recalled` egress helper.
//! - The Overseer USES recall in Observe/Orient: recall populates
//!   [`ObservedState::recall`], and ≥2 recalled episodes sharing a
//!   `failure_signature` raise a new structural [`Signal::RecurringSignature`]
//!   that promotes the problem to `Priority::High`.
//! - The Overseer WRITES its observation back as one **de-duplicated** episodic
//!   memory (reused `WhisperGate`-pattern gate, 900 s window).
//! - **No silent fallback**: a memory error is surfaced on
//!   [`ObservedState::recall_error`] (recall stays `None` — never a partial or
//!   empty snapshot) and counted in `OverseerTickReport.memory_errors`; the tick
//!   still completes.
//! - The whole path is **additive and opt-out** (`SIMARD_OVERSEER_MEMORY_RECALL`,
//!   default ON) and reuses the daemon's single `Arc<dyn CognitiveMemoryOps>`.
//!
//! Everything is exercised with injected fakes — no network, no `~/.simard`, no
//! second cognitive store.

use std::sync::{Arc, Mutex};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use crate::overseer::capabilities::{
    Auditor, Deployer, GoalCurator, IssueFiler, MeetingHost, ObservedState, OverseerError, PrOps,
    RecipeLauncher, StatusReader,
};
// ── The NEW capability seam + result/input types (do not exist yet: red). ──
use crate::overseer::capabilities::{
    AuditReport, AuditScope, CiFailure, DeployReport, GoalBrief, InFlightItem, IssueOutcome,
    OrchestratorRunBrief, RecipeBrief, VerifyReport, WorkstreamHandle, WorkstreamStatus,
};
use crate::overseer::capabilities::{
    MemoryRecall, MemorySnapshot, ObservationEpisode, RecallBudget, RecallKeys, RecalledEpisode,
    RecalledFact, RecalledProcedure, RecalledProspective, RecordOutcome, sanitize_recalled,
};
use crate::overseer::config::{
    OVERSEER_ENABLED_ENV, SIMARD_OVERSEER_MEMORY_RECALL_ENV, memory_recall_enabled_from,
};
use crate::overseer::signal::{Priority, Signal, signals_from};
use crate::overseer::wiring::{
    MemoryRecallOps, overseer_identity, overseer_tick, run_overseer_tick_isolated,
};
use crate::overseer::{Capabilities, Overseer, orient};

// ─────────────────────────── base capability fakes ─────────────────────────
// The eight pre-existing capabilities the Overseer already needs. Canned/inert
// so each recall test is isolated to the memory seam.

struct FakeStatus(ObservedState);
impl StatusReader for FakeStatus {
    fn snapshot(&self) -> Result<ObservedState, OverseerError> {
        Ok(self.0.clone())
    }
}

/// A status reader that yields a scripted sequence of `ObservedState`s (one per
/// tick), repeating the last once the script is exhausted. Lets a single
/// persistent Overseer see two *different* observations across two ticks.
struct SeqStatus(Mutex<Vec<ObservedState>>);
impl StatusReader for SeqStatus {
    fn snapshot(&self) -> Result<ObservedState, OverseerError> {
        let mut v = self.0.lock().unwrap();
        if v.len() > 1 {
            Ok(v.remove(0))
        } else {
            Ok(v[0].clone())
        }
    }
}

struct FakeRecipes;
impl RecipeLauncher for FakeRecipes {
    fn launch(&self, _brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
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
    fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
        Ok(VerifyReport {
            ready: false,
            checks: vec![],
        })
    }
    fn merge(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
        Ok(())
    }
    fn resolve_conflict(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
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
    fn transfer_goal(&self, _goal: &GoalBrief) -> Result<(), OverseerError> {
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
    fn propose(&self, _goal: &GoalBrief) -> Result<(), OverseerError> {
        Ok(())
    }
    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
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

// ─────────────────────────── memory-recall fake ────────────────────────────

/// Which recall/write sub-calls the fake should fail (to prove fail-closed,
/// whole-pass error surfacing).
#[derive(Clone, Copy, Default)]
struct RecallFailure {
    semantic: bool,
    episodic: bool,
    procedural: bool,
    prospective: bool,
    record: bool,
}

/// A `MemoryRecall` double: returns canned recall results (or injected errors)
/// and records every write-back so tests can assert de-dup and provenance.
#[derive(Clone)]
struct FakeMemoryRecall {
    facts: Vec<RecalledFact>,
    episodes: Vec<RecalledEpisode>,
    procedures: Vec<RecalledProcedure>,
    prospectives: Vec<RecalledProspective>,
    fail: RecallFailure,
    /// Every `record_observation` payload the Overseer actually dispatched.
    recorded: Arc<Mutex<Vec<ObservationEpisode>>>,
    /// Every `keys` seen by a recall call (for key-derivation assertions).
    seen_keys: Arc<Mutex<Vec<RecallKeys>>>,
}

impl FakeMemoryRecall {
    fn new() -> Self {
        Self {
            facts: Vec::new(),
            episodes: Vec::new(),
            procedures: Vec::new(),
            prospectives: Vec::new(),
            fail: RecallFailure::default(),
            recorded: Arc::new(Mutex::new(Vec::new())),
            seen_keys: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn recorded(&self) -> Arc<Mutex<Vec<ObservationEpisode>>> {
        Arc::clone(&self.recorded)
    }
}

fn cap_err(what: &'static str) -> OverseerError {
    OverseerError::Capability {
        what,
        detail: "injected memory failure".to_string(),
    }
}

impl MemoryRecall for FakeMemoryRecall {
    fn recall_semantic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledFact>, OverseerError> {
        self.seen_keys.lock().unwrap().push(keys.clone());
        if self.fail.semantic {
            return Err(cap_err("memory-recall"));
        }
        Ok(self.facts.iter().take(limit as usize).cloned().collect())
    }

    fn recall_episodic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledEpisode>, OverseerError> {
        self.seen_keys.lock().unwrap().push(keys.clone());
        if self.fail.episodic {
            return Err(cap_err("memory-recall"));
        }
        Ok(self.episodes.iter().take(limit as usize).cloned().collect())
    }

    fn recall_procedural(
        &self,
        _keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProcedure>, OverseerError> {
        if self.fail.procedural {
            return Err(cap_err("memory-recall"));
        }
        Ok(self
            .procedures
            .iter()
            .take(limit as usize)
            .cloned()
            .collect())
    }

    fn recall_prospective(
        &self,
        _keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProspective>, OverseerError> {
        if self.fail.prospective {
            return Err(cap_err("memory-recall"));
        }
        Ok(self
            .prospectives
            .iter()
            .take(limit as usize)
            .cloned()
            .collect())
    }

    fn record_observation(
        &self,
        episode: &ObservationEpisode,
    ) -> Result<RecordOutcome, OverseerError> {
        if self.fail.record {
            return Err(cap_err("memory-recall"));
        }
        let mut rec = self.recorded.lock().unwrap();
        rec.push(episode.clone());
        Ok(RecordOutcome::Stored {
            node_id: format!("ep-{}", rec.len()),
        })
    }
}

// ─────────────────────────── construction helpers ──────────────────────────

fn base_caps(status: Box<dyn StatusReader>, memory: Box<dyn MemoryRecall>) -> Capabilities {
    Capabilities {
        status,
        recipes: Box::new(FakeRecipes),
        prs: Box::new(FakePrs),
        deployer: Box::new(FakeDeployer),
        meetings: Box::new(FakeMeetings),
        issues: Box::new(FakeIssues),
        goals: Box::new(FakeGoals),
        auditor: Box::new(FakeAuditor),
        memory,
    }
}

/// Build an acting Overseer with recall enabled/disabled and a given memory fake.
fn overseer_with(
    observed: ObservedState,
    memory: FakeMemoryRecall,
    recall_enabled: bool,
) -> Overseer {
    let caps = base_caps(Box::new(FakeStatus(observed)), Box::new(memory));
    Overseer::new(caps)
        .with_identity(overseer_identity())
        .with_memory_recall_enabled(recall_enabled)
}

/// An `ObservedState` that reliably derives at least one Signal/Problem so a
/// recall pass has non-empty keys and a write-back has something to record.
fn observed_with_process_health() -> ObservedState {
    ObservedState {
        distill_fail_pct: Some(62.0),
        ..ObservedState::default()
    }
}

fn episode(id: &str, summary: &str, sig: Option<&str>) -> RecalledEpisode {
    RecalledEpisode {
        id: id.to_string(),
        summary: summary.to_string(),
        failure_signature: sig.map(str::to_string),
        score: 1.0,
    }
}

// ═══════════════════════════ config: opt-out flag ══════════════════════════

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

#[test]
fn memory_recall_enabled_by_default_when_unset() {
    // Opt-OUT: an unset var leaves recall ON (mirrors goal-health/whisper).
    assert!(memory_recall_enabled_from(env(&[])));
}

#[test]
fn memory_recall_disabled_only_on_explicit_falsey_values() {
    for falsey in ["0", "false", "FALSE", "False", "no", "off", "  off  "] {
        assert!(
            !memory_recall_enabled_from(env(&[(SIMARD_OVERSEER_MEMORY_RECALL_ENV, falsey)])),
            "{falsey:?} should DISABLE memory recall"
        );
    }
}

#[test]
fn memory_recall_stays_on_for_truthy_empty_or_garbage_values() {
    for on in ["1", "true", "yes", "on", "", "  ", "maybe", "2", "enabled"] {
        assert!(
            memory_recall_enabled_from(env(&[(SIMARD_OVERSEER_MEMORY_RECALL_ENV, on)])),
            "{on:?} must leave memory recall ON (default)"
        );
    }
}

#[test]
fn memory_recall_forced_off_when_overseer_disabled() {
    // A disabled Overseer forces recall off regardless of the recall flag.
    assert!(!memory_recall_enabled_from(env(&[
        (OVERSEER_ENABLED_ENV, "0"),
        (SIMARD_OVERSEER_MEMORY_RECALL_ENV, "1"),
    ])));
}

// ═══════════════════════════ sanitize_recalled ═════════════════════════════

#[test]
fn sanitize_recalled_strips_crlf_and_control_chars() {
    let dirty = "line one\r\nDROP TABLE\ttab\u{0007}bell\u{001b}[31m";
    let clean = sanitize_recalled(dirty);
    assert!(
        !clean.contains('\n') && !clean.contains('\r'),
        "newlines must be stripped/escaped (log/notification injection): {clean:?}"
    );
    assert!(
        !clean.chars().any(|c| c.is_control()),
        "no raw control characters may survive sanitization: {clean:?}"
    );
}

#[test]
fn sanitize_recalled_caps_length() {
    let long = "a".repeat(100_000);
    let clean = sanitize_recalled(&long);
    assert!(
        clean.len() < long.len(),
        "recalled text must be length-capped before egress"
    );
    assert!(
        clean.len() <= 8192,
        "length cap should bound egress to a sane size (got {})",
        clean.len()
    );
}

#[test]
fn sanitize_recalled_preserves_plain_text() {
    let plain = "distillation parse-failure rate 62% (process:distill_fail)";
    assert_eq!(sanitize_recalled(plain), plain);
}

// ═══════════════════════════ RecallBudget / RecallKeys ═════════════════════

#[test]
fn recall_budget_default_is_5_5_3_5() {
    let b = RecallBudget::default();
    assert_eq!(b.semantic, 5);
    assert_eq!(b.episodic, 5);
    assert_eq!(b.procedural, 3);
    assert_eq!(b.prospective, 5);
}

#[test]
fn recall_keys_empty_for_no_signals_or_problems() {
    let keys = RecallKeys::from_signals(&[], &[]);
    assert!(keys.keywords.is_empty(), "no signals ⇒ no keywords");
    assert!(keys.signatures.is_empty(), "no problems ⇒ no signatures");
}

#[test]
fn recall_keys_are_derived_and_deterministic() {
    let signals = signals_from(&observed_with_process_health());
    let problems = orient(&signals, &[]);
    assert!(!problems.is_empty(), "guard: fixture must derive a problem");

    let a = RecallKeys::from_signals(&signals, &problems);
    let b = RecallKeys::from_signals(&signals, &problems);

    // Keys are derived from the cycle's signals/problems (never a full scan)…
    assert!(
        !a.keywords.is_empty(),
        "keywords must be derived from the detected signals"
    );
    assert!(
        !a.signatures.is_empty(),
        "one failure-signature-style key per problem"
    );
    // …and deterministic so recall is reproducible.
    assert_eq!(a.keywords, b.keywords);
    assert_eq!(a.signatures, b.signatures);
}

// ═══════════════════ RecurringSignature signal derivation ══════════════════

fn snapshot_with_episodes(eps: Vec<RecalledEpisode>) -> MemorySnapshot {
    MemorySnapshot {
        facts: vec![],
        episodes: eps,
        procedures: vec![],
        prospectives: vec![],
    }
}

#[test]
fn recurring_signature_emitted_when_two_episodes_share_signature() {
    let state = ObservedState {
        recall: Some(snapshot_with_episodes(vec![
            episode("e1", "prior distill failure", Some("process:distill_fail")),
            episode(
                "e2",
                "another distill failure",
                Some("process:distill_fail"),
            ),
        ])),
        ..ObservedState::default()
    };
    let sigs = signals_from(&state);
    assert!(
        sigs.contains(&Signal::RecurringSignature {
            signature: "process:distill_fail".to_string(),
            occurrences: 2,
        }),
        "≥2 recalled episodes sharing a signature must raise RecurringSignature; got {sigs:?}"
    );
}

#[test]
fn recurring_signature_not_emitted_for_single_occurrence() {
    let state = ObservedState {
        recall: Some(snapshot_with_episodes(vec![episode(
            "e1",
            "one-off",
            Some("process:distill_fail"),
        )])),
        ..ObservedState::default()
    };
    let has_recurring = signals_from(&state)
        .iter()
        .any(|s| matches!(s, Signal::RecurringSignature { .. }));
    assert!(!has_recurring, "a single occurrence is not recurring");
}

#[test]
fn recurring_signature_ignores_episodes_without_signature() {
    let state = ObservedState {
        recall: Some(snapshot_with_episodes(vec![
            episode("e1", "no sig", None),
            episode("e2", "no sig", None),
        ])),
        ..ObservedState::default()
    };
    let has_recurring = signals_from(&state)
        .iter()
        .any(|s| matches!(s, Signal::RecurringSignature { .. }));
    assert!(
        !has_recurring,
        "episodes lacking a failure_signature must not count toward recurrence"
    );
}

#[test]
fn no_recall_yields_no_recurring_signature() {
    // Additive/no-op: with recall disabled/absent the signal set is unchanged.
    let state = ObservedState {
        recall: None,
        ..observed_with_process_health()
    };
    let has_recurring = signals_from(&state)
        .iter()
        .any(|s| matches!(s, Signal::RecurringSignature { .. }));
    assert!(!has_recurring);
}

#[test]
fn recurring_signature_is_additive_not_replacing() {
    // Base state derives DistillFailureRate; recall adds RecurringSignature on
    // top — the existing signal must still be present (recall never removes).
    let mut state = observed_with_process_health();
    state.recall = Some(snapshot_with_episodes(vec![
        episode("e1", "prior", Some("process:distill_fail")),
        episode("e2", "prior", Some("process:distill_fail")),
    ]));
    let sigs = signals_from(&state);
    assert!(
        sigs.contains(&Signal::DistillFailureRate { pct: 62.0 }),
        "recall must be additive: the pre-existing signal survives"
    );
    assert!(
        sigs.iter()
            .any(|s| matches!(s, Signal::RecurringSignature { .. })),
        "and RecurringSignature is added alongside it"
    );
}

// ═══════════════════════ orient: priority promotion ════════════════════════

#[test]
fn orient_raises_recurring_signature_to_high_priority() {
    let signals = vec![Signal::RecurringSignature {
        signature: "process:distill_fail".to_string(),
        occurrences: 3,
    }];
    let problems = orient(&signals, &[]);
    assert!(
        !problems.is_empty(),
        "a recurring signature yields a problem"
    );
    assert!(
        problems.iter().any(|p| p.priority == Priority::High),
        "a recurring signature promotes the problem to High priority; got {:?}",
        problems.iter().map(|p| p.priority).collect::<Vec<_>>()
    );
}

#[test]
fn recurring_signature_problem_summary_is_sanitized() {
    // A hostile signature carrying CR/LF must never reach a Problem.summary raw
    // (that summary can egress to an operator notification).
    let signals = vec![Signal::RecurringSignature {
        signature: "evil\r\nSUBJECT: spoofed".to_string(),
        occurrences: 2,
    }];
    for p in orient(&signals, &[]) {
        assert!(
            !p.summary.contains('\n') && !p.summary.contains('\r'),
            "recalled/derived text must be sanitized before entering Problem.summary: {:?}",
            p.summary
        );
    }
}

// ═════════════════ run_cycle: recall USE + error surfacing ═════════════════

#[test]
fn run_cycle_populates_recall_snapshot_when_enabled() {
    let mut mem = FakeMemoryRecall::new();
    mem.facts = vec![RecalledFact {
        id: "f1".to_string(),
        content: "distillation flakiness root-cause".to_string(),
        score: 0.9,
    }];
    mem.episodes = vec![episode("e1", "prior fix", Some("process:distill_fail"))];

    let mut ov = overseer_with(observed_with_process_health(), mem, true);
    let report = ov
        .run_cycle()
        .expect("cycle must not error on a healthy recall");

    let snap = report
        .observed
        .recall
        .as_ref()
        .expect("recall enabled + memory OK ⇒ Some(snapshot)");
    assert_eq!(
        snap.facts.len(),
        1,
        "recalled facts flow onto ObservedState"
    );
    assert_eq!(snap.episodes.len(), 1);
    assert!(
        report.observed.recall_error.is_none(),
        "a successful recall leaves recall_error clear"
    );
}

#[test]
fn run_cycle_leaves_recall_none_when_disabled() {
    // Disabled ⇒ the graph is never queried and recall stays None.
    let mem = FakeMemoryRecall {
        facts: vec![RecalledFact {
            id: "f1".to_string(),
            content: "should never be read".to_string(),
            score: 1.0,
        }],
        ..FakeMemoryRecall::new()
    };
    let seen_keys = Arc::clone(&mem.seen_keys);

    let mut ov = overseer_with(observed_with_process_health(), mem, false);
    let report = ov.run_cycle().expect("cycle");

    assert!(report.observed.recall.is_none());
    assert!(report.observed.recall_error.is_none());
    assert!(
        seen_keys.lock().unwrap().is_empty(),
        "recall must not touch memory when disabled"
    );
}

#[test]
fn run_cycle_surfaces_error_and_leaves_recall_none_on_memory_failure() {
    let mem = FakeMemoryRecall {
        fail: RecallFailure {
            episodic: true,
            ..RecallFailure::default()
        },
        ..FakeMemoryRecall::new()
    };
    let mut ov = overseer_with(observed_with_process_health(), mem, true);

    // No silent fallback: the tick still completes (Ok), but the error is
    // surfaced — NOT swallowed into an empty snapshot.
    let report = ov
        .run_cycle()
        .expect("recall failure must not abort the cycle");
    assert!(
        report.observed.recall.is_none(),
        "a recall error must NOT be replaced by an empty snapshot"
    );
    assert!(
        report.observed.recall_error.is_some(),
        "the recall error must be surfaced on recall_error (no silent fallback)"
    );
}

#[test]
fn recall_is_fail_closed_whole_pass_discards_partial_reads() {
    // Semantic succeeds, episodic fails: the WHOLE pass fails closed. The
    // successful reads are discarded (recall == None), never a partial snapshot.
    let mut mem = FakeMemoryRecall::new();
    mem.facts = vec![RecalledFact {
        id: "f1".to_string(),
        content: "ok semantic read".to_string(),
        score: 1.0,
    }];
    mem.fail = RecallFailure {
        episodic: true,
        ..RecallFailure::default()
    };

    let mut ov = overseer_with(observed_with_process_health(), mem, true);
    let report = ov.run_cycle().expect("cycle");
    assert!(
        report.observed.recall.is_none(),
        "partial success must not produce a partial snapshot"
    );
    assert!(report.observed.recall_error.is_some());
}

// ═══════════════ overseer_tick: counters + no-silent-fallback ═══════════════

#[test]
fn tick_counts_one_memory_recall_on_success() {
    let mem = FakeMemoryRecall::new(); // empty graph is a valid success
    let mut ov = overseer_with(observed_with_process_health(), mem, true);
    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.memory_recalls, 1,
        "a completed whole recall pass increments memory_recalls exactly once"
    );
    assert_eq!(report.memory_errors, 0, "a clean pass reports no errors");
}

#[test]
fn tick_counts_memory_error_and_not_a_recall_on_failure() {
    let mem = FakeMemoryRecall {
        fail: RecallFailure {
            semantic: true,
            ..RecallFailure::default()
        },
        ..FakeMemoryRecall::new()
    };
    let mut ov = overseer_with(observed_with_process_health(), mem, true);
    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.memory_recalls, 0,
        "a failed pass never counts as a completed recall"
    );
    assert!(
        report.memory_errors >= 1,
        "the surfaced recall failure is counted in memory_errors"
    );
}

#[test]
fn disabled_recall_keeps_all_memory_counters_zero() {
    let mem = FakeMemoryRecall::new();
    let recorded = mem.recorded();
    let mut ov = overseer_with(observed_with_process_health(), mem, false);
    let report = overseer_tick(&mut ov);
    assert_eq!(report.memory_recalls, 0);
    assert_eq!(report.memory_writes, 0);
    assert_eq!(report.memory_errors, 0);
    assert!(
        recorded.lock().unwrap().is_empty(),
        "no write-back when recall is disabled"
    );
}

#[test]
fn recall_failure_never_panics_the_tick() {
    // A recall error is surfaced, not a panic: the panic-isolated tick reports
    // NOT-panicked and still completes (duration recorded).
    let mem = FakeMemoryRecall {
        fail: RecallFailure {
            semantic: true,
            episodic: true,
            procedural: true,
            prospective: true,
            record: true,
        },
        ..FakeMemoryRecall::new()
    };
    let mut ov = overseer_with(observed_with_process_health(), mem, true);
    let report = run_overseer_tick_isolated(&mut ov);
    assert!(!report.panicked, "memory errors must surface, never panic");
    assert!(report.memory_errors >= 1);
}

// ═══════════════════ write-back: deliberate + de-duplicated ═════════════════

#[test]
fn tick_writes_observation_back_once() {
    let mem = FakeMemoryRecall::new();
    let recorded = mem.recorded();
    let mut ov = overseer_with(observed_with_process_health(), mem, true);

    let report = overseer_tick(&mut ov);
    assert_eq!(
        report.memory_writes, 1,
        "the Overseer records its observation back exactly once"
    );
    assert_eq!(
        recorded.lock().unwrap().len(),
        1,
        "one episodic observation persisted"
    );
}

#[test]
fn write_back_is_deduplicated_within_window() {
    // Two identical ticks (same observed state ⇒ same signature) within the
    // 900 s window: the second write-back is de-duplicated (nothing persisted).
    let mem = FakeMemoryRecall::new();
    let recorded = mem.recorded();
    let mut ov = overseer_with(observed_with_process_health(), mem, true);

    let t1 = overseer_tick(&mut ov);
    let t2 = overseer_tick(&mut ov);

    assert_eq!(t1.memory_writes, 1, "first observation is stored");
    assert_eq!(
        t2.memory_writes, 0,
        "an identical-signature observation within the window is suppressed"
    );
    assert_eq!(
        recorded.lock().unwrap().len(),
        1,
        "exactly one episode persisted across two identical ticks"
    );
}

#[test]
fn write_back_persists_again_for_a_distinct_signature() {
    // Two DIFFERENT observations ⇒ two distinct signatures ⇒ both are recorded.
    let states = vec![
        ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        },
        ObservedState {
            ci_failures: vec![CiFailure {
                repo: "rysweet/Simard".to_string(),
                failing: 4,
            }],
            ..ObservedState::default()
        },
    ];
    let mem = FakeMemoryRecall::new();
    let recorded = mem.recorded();
    let caps = base_caps(Box::new(SeqStatus(Mutex::new(states))), Box::new(mem));
    let mut ov = Overseer::new(caps)
        .with_identity(overseer_identity())
        .with_memory_recall_enabled(true);

    let t1 = overseer_tick(&mut ov);
    let t2 = overseer_tick(&mut ov);
    assert_eq!(t1.memory_writes, 1);
    assert_eq!(
        t2.memory_writes, 1,
        "a distinct observation is recorded too"
    );
    assert_eq!(recorded.lock().unwrap().len(), 2);
}

// ══════════════ MemoryRecallOps adapter over the shared handle ══════════════
// Proves the adapter reuses ONE CognitiveMemoryOps handle (single-source),
// pins the write-back provenance (source_label = "overseer"), and maps every
// underlying Err to OverseerError::Capability.

/// A recording `CognitiveMemoryOps` double. Captures `store_episode` provenance
/// and the `check_triggers` probe; returns canned recall data or injected Errs.
#[derive(Default)]
struct RecordingCogMem {
    fail: bool,
    facts: Vec<CognitiveFact>,
    episodes: Vec<CognitiveEpisode>,
    /// (content, source_label, had_metadata) of each store_episode.
    stored_episodes: Arc<Mutex<Vec<(String, String, bool)>>>,
    /// Each `check_triggers` probe string.
    trigger_probes: Arc<Mutex<Vec<String>>>,
}

fn integrity_err() -> SimardError {
    SimardError::MemoryIntegrityError {
        path: std::path::PathBuf::from("<fake>"),
        reason: "injected".to_string(),
    }
}

impl CognitiveMemoryOps for RecordingCogMem {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("s".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("w".to_string())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        if self.fail {
            return Err(integrity_err());
        }
        self.stored_episodes.lock().unwrap().push((
            content.to_string(),
            source_label.to_string(),
            metadata.is_some(),
        ));
        Ok("ep-1".to_string())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _c: &str,
        _content: &str,
        _cf: f64,
        _t: &[String],
        _s: &str,
    ) -> SimardResult<String> {
        Ok("f".to_string())
    }
    fn search_facts(
        &self,
        _query: &str,
        limit: u32,
        _min_conf: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        if self.fail {
            return Err(integrity_err());
        }
        Ok(self.facts.iter().take(limit as usize).cloned().collect())
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("p".to_string())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pr".to_string())
    }
    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        self.trigger_probes
            .lock()
            .unwrap()
            .push(content.to_string());
        Ok(vec![])
    }
    fn search_episodes_by_keywords(
        &self,
        _keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        if self.fail {
            return Err(integrity_err());
        }
        Ok(self.episodes.iter().take(limit as usize).cloned().collect())
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

fn keys(keywords: &[&str], signatures: &[&str]) -> RecallKeys {
    RecallKeys {
        keywords: keywords.iter().map(|s| s.to_string()).collect(),
        signatures: signatures.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn adapter_write_back_uses_fixed_overseer_source_label() {
    let backing = Arc::new(RecordingCogMem::default());
    let stored = Arc::clone(&backing.stored_episodes);
    let adapter = MemoryRecallOps::new(backing as Arc<dyn CognitiveMemoryOps>);

    let out = adapter
        .record_observation(&ObservationEpisode {
            content: "overseer observed distill flakiness".to_string(),
            signature: "process:distill_fail".to_string(),
        })
        .expect("write-back");
    assert!(matches!(out, RecordOutcome::Stored { .. }));

    let rows = stored.lock().unwrap();
    assert_eq!(rows.len(), 1, "one episode stored via the shared handle");
    assert_eq!(
        rows[0].1, "overseer",
        "provenance is fixed: source_label must be \"overseer\" (never caller-chosen)"
    );
    assert!(
        rows[0].2,
        "typed metadata carrying the signature is attached"
    );
}

#[test]
fn adapter_maps_underlying_error_to_capability() {
    let backing = Arc::new(RecordingCogMem {
        fail: true,
        ..RecordingCogMem::default()
    });
    let adapter = MemoryRecallOps::new(backing as Arc<dyn CognitiveMemoryOps>);

    let err = adapter
        .recall_semantic(&keys(&["distill"], &["process:distill_fail"]), 5)
        .expect_err("a backing error must surface, never an empty Ok");
    match err {
        OverseerError::Capability { what, .. } => {
            assert_eq!(what, "memory-recall", "errors map to the recall capability")
        }
        other => panic!("expected OverseerError::Capability, got {other:?}"),
    }
}

#[test]
fn adapter_recall_semantic_projects_underlying_facts() {
    let backing = Arc::new(RecordingCogMem {
        facts: vec![CognitiveFact {
            node_id: "n1".to_string(),
            concept: "distill".to_string(),
            content: "flaky distiller".to_string(),
            confidence: 0.8,
            source_id: "src".to_string(),
            tags: vec![],
            usage_count: 0,
            last_accessed_at: None,
        }],
        ..RecordingCogMem::default()
    });
    let adapter = MemoryRecallOps::new(backing as Arc<dyn CognitiveMemoryOps>);

    let facts = adapter
        .recall_semantic(&keys(&["distill"], &[]), 5)
        .expect("recall");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].id, "n1");
    assert_eq!(facts[0].content, "flaky distiller");
}

#[test]
fn adapter_prospective_joins_keys_into_one_probe() {
    // check_triggers takes a single &str: the adapter must join keys into one
    // deterministic probe rather than fanning out per key.
    let backing = Arc::new(RecordingCogMem::default());
    let probes = Arc::clone(&backing.trigger_probes);
    let adapter = MemoryRecallOps::new(backing as Arc<dyn CognitiveMemoryOps>);

    adapter
        .recall_prospective(&keys(&["alpha", "beta"], &["sig-1"]), 5)
        .expect("prospective recall");

    let probes = probes.lock().unwrap();
    assert_eq!(probes.len(), 1, "exactly one joined check_triggers probe");
    assert!(
        probes[0].contains("alpha") && probes[0].contains("beta"),
        "the single probe carries all keys: {:?}",
        probes[0]
    );
}
