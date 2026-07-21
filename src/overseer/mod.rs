//! # Overseer — an autonomous operator/observer co-process (DESIGN SKETCH)
//!
//! The `Overseer` embeds the operator/observer role a human+Copilot pair has
//! performed over many sessions: it watches HOW Simard performs, spots problems,
//! and drives improvements **outside** Simard's own OODA workstreams. Simard's
//! OODA governs the external repos she stewards plus her own feature work; the
//! Overseer works at the **meta level** — improving Simard's own health/process
//! and driving cross-cutting initiatives.
//!
//! ## Status: design + scaffolding only
//!
//! This module is a **type/trait sketch**. It is additive, `#![allow(dead_code)]`,
//! and **not wired into `main`** or the daemon loop — nothing here is constructed
//! or scheduled at runtime. It exists to pin down the vocabulary (`Signal`,
//! `Problem`, `Intervention`), the capability seam (`capabilities`), and the
//! guardrails (`guardrails`), each annotated with the EXISTING Simard function it
//! reuses. See `docs/design/overseer.md` for the full architecture, the
//! co-process-vs-`CognitiveThread` decision, and the phased roadmap.
//!
//! ## Architecture (summary)
//!
//! The Overseer is a **sibling co-process**, not a `CognitiveThread`. A
//! `CognitiveThread` is given a least-authority `ThreadContext` and is explicitly
//! forbidden a "code path to self_deploy / self_relaunch / redeploy"
//! (`docs/howto/add-a-new-cognitive-thread.md`); the Overseer needs guarded
//! deploy authority and launches long-running recipe/merge work, so it runs as
//! its own supervised task holding capability handles behind guardrails. A thin,
//! read-only `impl CognitiveThread` **sensor** (observe → signals → report → file
//! issue) is a valid M1 packaging and is described in the design doc; the acting
//! Overseer (M2+) is a co-process.
//!
//! ## Meta-OODA loop
//!
//! `run_cycle` implements one turn of the Overseer's OWN OODA, distinct from
//! Simard's repo-facing OODA:
//!
//! - **Observe** — `StatusReader::snapshot` (wraps `crate::status::assemble`) plus
//!   PR/CI/goal state, folded into `ObservedState`, then `signal::signals_from`.
//! - **Orient** — `orient`: classify + prioritise + **dedup against Simard's
//!   in-flight work** (`GoalCurator::in_flight`).
//! - **Decide** — `decide`: choose one `Intervention` per `Problem`.
//! - **Act** — gate (`guardrails`) then dispatch via the reused capability
//!   (`act`). `run_cycle` only PLANS; execution of admitted interventions is the
//!   M2+ seam.

#![allow(dead_code)]

pub mod activity;
pub mod audit;
pub mod capabilities;
pub mod claim_reaper;
pub mod config;
pub mod conflict;
pub mod deploy;
pub mod deploy_trigger;
pub mod diagnosis;
pub mod ecosystem_observe;
pub mod failure_sink;
pub mod guardrails;
pub mod health_review;
pub mod intervention;
pub mod launch;
pub mod meeting_ops;
pub mod merge_ops;
pub mod merge_queue_observe;
pub mod notify;
pub mod observer;
pub mod pr_verify;
pub mod root_cause;
pub mod sensor;
pub mod signal;
pub mod tuning;
pub mod whisper_ops;
pub mod wiring;

#[cfg(test)]
mod tests_deploy_drift;
#[cfg(test)]
mod tests_diagnosis;
#[cfg(test)]
mod tests_escalation_triage;
#[cfg(test)]
mod tests_gap_scan;
#[cfg(test)]
mod tests_goal_health;
#[cfg(test)]
mod tests_m1;
#[cfg(test)]
mod tests_m2;
#[cfg(test)]
mod tests_memory_recall;
#[cfg(test)]
mod tests_merge_queue_reasoning;
#[cfg(test)]
mod tests_root_cause;
#[cfg(test)]
mod tests_self_healing;
#[cfg(test)]
mod tests_selfmerge_fix;
#[cfg(test)]
mod tests_whisper;

pub use capabilities::{
    Auditor, BlockedGoal, Deployer, GoalCurator, IssueFiler, IssuePriority, IssueReadiness,
    MeetingHost, MemoryRecall, MemorySnapshot, MergeReasoningStatus, ObservationEpisode,
    ObservedState, OrchestratorRunBrief, OverseerError, PrDisposition, PrOps, PrRef, ReasonedPr,
    RecallBudget, RecallKeys, RecalledEpisode, RecalledFact, RecalledProcedure,
    RecalledProspective, RecipeBrief, RecipeLauncher, RecordOutcome, StatusReader, TriagedIssue,
    sanitize_recalled,
};
pub use config::{
    daily_budget_usd, gap_scan_enabled, gap_scan_every_n, goal_health_enabled,
    health_review_enabled, health_review_service_unit, memory_recall_enabled,
    overseer_acting_enabled, overseer_author_login, overseer_enabled,
};
pub use diagnosis::{FailureCause, FailureDiagnosis, classify_terminal_failure};
pub use guardrails::{
    AutonomyGate, BackoffDecision, BackoffGate, BudgetGate, ConflictSequencer, RecursionGuard,
    RiskClass, Subject, WhisperDecision, WhisperGate, classify,
};
pub use intervention::{Intervention, PlannedIntervention, Remediation, RemediationClass};
pub use observer::{StewardshipIssueFiler, decide_read_only, is_m1_permitted};
pub use sensor::{
    ObserverReport, OverseerSensorThread, SnapshotSource, SnapshotStatusReader,
    blocked_goals_from_board, in_flight_from_board, observed_from_snapshot, run_observer_cycle,
};
pub use signal::{
    CauseCandidate, CauseSource, Confidence, GapCategory, GapItem, Likelihood, Priority, Problem,
    ProblemKind, RootCause, Signal, signals_from,
};
pub use whisper_ops::{
    MeetingHandoffWhisperSink, WhisperRecord, WhisperSink, WhisperUrgency, compose_whisper_note,
    note_signature, whisper_signature,
};
pub use wiring::{
    BoardGoalCurator, MemoryRecallOps, OverseerCadence, OverseerTickReport, RefuseDeployer,
    assemble_capabilities, build_overseer, overseer_identity, overseer_tick,
    overseer_tick_detailed, overseer_tick_interval_secs, run_overseer_tick_isolated,
    run_overseer_tick_isolated_detailed,
};

pub use activity::ProblemEntry;
pub use root_cause::{PriorOccurrence, RECURRENCE_ESCALATION_THRESHOLD, root_cause_signature};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, is_no_progress_marker,
};
use crate::overseer::notify::{OperatorNotification, OperatorNotifier};
use crate::stewardship::PrSnapshot;
use crate::stewardship::merge_authority::evaluate_objective_gates;
use capabilities::{
    DeployReport, GoalBrief, InFlightItem, IssueOutcome, WorkstreamHandle, WorkstreamStatus,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The capability handles the Overseer acts through — one per reused Simard
/// subsystem. Grouping them keeps the `Overseer` constructor to a single
/// argument and makes every external dependency explicit and injectable (fakes
/// in tests, real adapters in the daemon).
pub struct Capabilities {
    pub status: Box<dyn StatusReader>,
    pub recipes: Box<dyn RecipeLauncher>,
    pub prs: Box<dyn PrOps>,
    pub deployer: Box<dyn Deployer>,
    pub meetings: Box<dyn MeetingHost>,
    pub issues: Box<dyn IssueFiler>,
    pub goals: Box<dyn GoalCurator>,
    pub auditor: Box<dyn Auditor>,
    /// Bounded read/write access to Simard's cognitive-memory graph (issue
    /// #2628). A thin adapter over the daemon's single shared
    /// [`CognitiveMemoryOps`](crate::cognitive_memory::CognitiveMemoryOps)
    /// handle — the Overseer never opens a second store.
    pub memory: Box<dyn MemoryRecall>,
}

/// The Overseer co-process. Holds its capability handles plus its guardrails.
pub struct Overseer {
    caps: Capabilities,
    autonomy: AutonomyGate,
    recursion: RecursionGuard,
    budget: BudgetGate,
    sequencer: ConflictSequencer,
    /// Cap on how many cost-bearing launches one cycle may plan (concurrency
    /// bound layered on top of the AIMD engineer cap the launcher already obeys).
    max_launches_per_cycle: usize,
    /// The Simard Whisperer's delivery seam (advisory steering notes onto the
    /// meeting-handoff inbox). `None` until wired; whispers require it.
    whisper_sink: Option<Box<dyn WhisperSink>>,
    /// Dedup + rate-limit gate for whispers (shared across ticks).
    whisper_gate: WhisperGate,
    /// Whether the whisperer is enabled (config opt-out). When off, whispers are
    /// held (never delivered) even if the observed condition holds.
    whisper_enabled: bool,
    /// The operator-notification seam (email + Signal) the ESCALATE-blocked-goal
    /// path fires through so a "needs human review" marker reaches a human.
    /// `None` until wired; escalation requires it.
    notifier: Option<Box<dyn OperatorNotifier>>,
    /// Dedup + rate-limit gate for goal-board self-heal / escalation actions, so
    /// the same blocked goal is not re-unblocked or re-escalated every tick.
    blocked_goal_gate: WhisperGate,
    /// Whether goal-board health handling (self-heal + escalate) is enabled
    /// (config opt-out). When off, both actions are held (never taken).
    goal_health_enabled: bool,
    /// Whether cognitive-memory recall (issue #2628) is enabled (config opt-out).
    /// When off, the graph is never queried, `ObservedState.recall` stays `None`,
    /// no observation is written back, and the memory counters stay `0`.
    memory_recall_enabled: bool,
    /// Dedup + rate-limit gate for the deliberate episodic write-back, so the
    /// same observation signature is not re-recorded every tick. Reuses the
    /// [`WhisperGate`] primitive (900 s window) keyed by observation signature.
    write_back_gate: WhisperGate,
    /// Whether the recurring backlog-coverage gap-scan is enabled (config
    /// opt-out). When off, the gap-scan action is held (never notifies/files)
    /// even though gaps were observed.
    gap_scan_enabled: bool,
    /// Dedup gate for the gap-scan, keyed on each gap's signature, so a recurring
    /// gap is notified + filed at most once per window (never every tick). Kept
    /// distinct from `blocked_goal_gate` so goal-health and gap-scan dedup never
    /// interfere.
    gap_gate: WhisperGate,
    /// The cognitive-memory handle (amplihack-memory-lib, G2) the root-cause
    /// analysis recalls prior occurrences from and records new ones into. `None`
    /// until wired: the analysis then degrades gracefully to telemetry-only WHYs
    /// with zero recurrence (never a silent failure — the WHY is still produced
    /// and honestly labelled `source = Telemetry`). Distinct from the observe-loop
    /// memory-recall capability (`caps.memory`): this is the occurrence-signature
    /// recall/store seam that drives recurrence-based root-cause escalation.
    memory: Option<Arc<dyn CognitiveMemoryOps>>,
    /// In-flight recipe-investigation dedup set (live defect 2026-07-15): keyed by
    /// each launched investigation's [`recipe_dedup_key`], holding the workstream
    /// handle so the set can self-reconcile. A goal / recurring-signature that
    /// already has a recipe-runner investigation RUNNING is never launched a
    /// second time — the concrete defect was two recipe-runner PIDs (1074394 and
    /// 1095553) investigating the identical `overseer-obs:goal:blocked:…`
    /// signature at once. Populated in [`Overseer::act`] on a successful launch
    /// and reconciled at the top of [`Overseer::run_cycle`] (a workstream that is
    /// no longer `Running` frees its slot), so the guard is "at most one IN
    /// FLIGHT", never a permanent one-shot.
    inflight_investigations: HashMap<String, WorkstreamHandle>,
    /// Whether the periodic stale-engineer-claim reaper (issue #4099) is enabled
    /// (config opt-out via `SIMARD_CLAIM_REAP_ENABLED`). ON by default in the
    /// daemon; OFF in the bare constructor. When off, the sweep is a no-op.
    claim_reap_enabled: bool,
    /// Idle-staleness threshold (seconds) the reaper applies to a worktree's
    /// newest-file age. Generous (default 1800s) so a long-but-alive engineer is
    /// never reaped (no wall-clock kill).
    claim_reap_stale_secs: u64,
    /// The reaper's three injected seams: the ledger it sweeps + reclaims through
    /// (the shared release chokepoint), the liveness probe, and the orphan
    /// worktree cleanup. `None` until wired by `build_overseer`; when absent the
    /// tick simply skips the sweep.
    claim_reaper: Option<ClaimReaperSeams>,
    /// Self-improvement interventions the stale-engineer reaper's investigations
    /// surfaced this tick (issue #4400), staged for dispatch through the SAME
    /// gated Act path health-review uses. Populated by `reap_stale_engineer_claims`
    /// at the top of the tick and drained into the plan after `health_review`, so
    /// a stale engineer's investigation feeds self-improvement rather than a
    /// silent reclaim. Empty between ticks.
    pending_reaper_interventions: Vec<Intervention>,
    /// The live agentic ecosystem-observe rail (issue #2419): the thin
    /// [`ecosystem_observe::EcosystemObserver`] seam that invokes the
    /// `ecosystem-observe` recipe on the Overseer cadence and forwards its
    /// OPAQUE semantic brief into the gated launch machinery. `None` until wired
    /// by `build_overseer`; when absent (bare constructor / tests) the pass is
    /// skipped and the Overseer behaves exactly as before. This REPLACES the
    /// retired single-repo Rust gap-scan survey as the cross-repo observation
    /// SOURCE — Rust never queries or parses a repo; the observation lives in the
    /// agent's reasoning and is handed forward as an opaque string.
    ecosystem_observer: Option<Box<dyn ecosystem_observe::EcosystemObserver>>,
    /// The stewarded roster (validated `owner/name` slugs) handed to the
    /// ecosystem-observe recipe. Loaded once from `ecosystem_repos.toml` by
    /// `build_overseer`; empty (and the pass skipped) until wired.
    ecosystem_roster: Vec<String>,
    /// Every-N cadence for the ecosystem-observe pass (reuses the gap-scan
    /// cadence knob). Clamped to a floor of 1 by [`ecosystem_observe::should_observe`].
    ecosystem_every_n: u64,
    /// Monotonic tick counter driving the ecosystem-observe every-N cadence.
    ecosystem_tick: u64,
    /// Injected wall-clock seam (Unix seconds) the gap-scan dedup backoff reads.
    /// The daemon uses real wall-clock ([`now_secs`]); tests inject a virtual
    /// clock via [`Overseer::with_clock`] so the backoff window is deterministic.
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    /// Exponential-backoff dedup gate on the `WorkstreamCoverage` launch path
    /// (Problem 1 / issue #4186). The in-flight guard only holds a duplicate
    /// WHILE the covering workstream runs; once it COMPLETES, an unchanged,
    /// still-recurring gap would re-launch every tick. This gate suppresses an
    /// equivalent relaunch (keyed by the same [`recipe_dedup_key`]) within a
    /// growing, bounded window even after completion — never permanently.
    coverage_backoff: BackoffGate,
    /// The agentic observe/orient merge-queue reasoner rail (#4097). When wired
    /// (`build_overseer` with a resolved recipe-runner), each due tick invokes the
    /// `observe-merge-queue` recipe — an AGENT surveys open PRs + issues across the
    /// governed roster and REASONS to a bounded brief — and the rail parses it
    /// FAIL-CLOSED into `ObservedState.reasoned_prs` / `triaged_issues`. Absent by
    /// default (bare constructor / tests) so the pass is skipped and the Overseer
    /// behaves exactly as before. The reasoning is BROAD; merge AUTHORIZATION stays
    /// narrow behind [`project_ready_prs`] + the downstream merge-authority gate.
    merge_queue_reasoner: Option<Box<dyn merge_queue_observe::MergeQueueReasoner>>,
    /// The governed reasoning roster (validated `owner/name` slugs) handed to the
    /// `observe-merge-queue` recipe as the DEFAULT scope. Loaded once from
    /// `ecosystem_repos.toml` by `build_overseer`; the resolved scope
    /// ([`config::merge_reasoning_scope`]) is default-ON over this roster and only
    /// an EXPLICIT operator disable turns it off (loud). Empty (pass skipped) until
    /// wired.
    merge_queue_roster: Vec<String>,
    /// Every-N cadence for the merge-queue observe pass (reuses the gap-scan
    /// cadence knob, like the ecosystem pass). Clamped to a floor of 1.
    merge_queue_every_n: u64,
    /// Monotonic tick counter driving the merge-queue observe every-N cadence.
    merge_queue_tick: u64,
    /// One-time latch for the R3 "merge reasoning DISABLED" operator notification
    /// (#4097): an EXPLICIT operator disable is surfaced LOUD once (WARN log +
    /// status field + a single dual-channel NotifyOperator), never re-sent every
    /// tick while it stays disabled.
    merge_reasoning_disabled_notified: bool,
    /// The agentic **health-review** rail ([standing]): the thin
    /// [`health_review::HealthReviewer`] seam that invokes the
    /// `overseer-health-review` recipe on the Overseer cadence — an AGENT reads
    /// the OODA journal + `simard status` + `simard goal list`, detects
    /// crash-loops / shared failure signatures, and reasons to typed remediation
    /// DECISIONS — and routes each into the SAME gated `LaunchRecipe` /
    /// `EscalateBlockedGoal` path every other action uses. `None` until wired by
    /// `build_overseer`; when absent (bare constructor / tests) the pass is
    /// skipped and the Overseer behaves exactly as before. Rust never reads the
    /// journal, counts a failure, or encodes a threshold — the observation and
    /// the remediation judgment live entirely in the agent's reasoning.
    health_reviewer: Option<Box<dyn health_review::HealthReviewer>>,
    /// Whether the agentic health-review rail is enabled (dedicated config
    /// opt-out `SIMARD_OVERSEER_HEALTH_REVIEW`, on top of the shared
    /// `SIMARD_OVERSEER_GAP_SCAN` scan throttle). Off in the bare constructor;
    /// `build_overseer` sets it from [`config::health_review_enabled`].
    health_review_enabled: bool,
    /// Every-N cadence for the health-review pass (reuses the gap-scan cadence
    /// knob, like the ecosystem/merge-queue passes). Clamped to a floor of 1 by
    /// [`ecosystem_observe::should_observe`].
    health_review_every_n: u64,
    /// Monotonic tick counter driving the health-review every-N cadence.
    health_review_tick: u64,
    /// The autonomous self-deploy drift sensor (issue #2590). `None` in the bare
    /// constructor and in tests that do not exercise the rail; `build_overseer`
    /// wires the production [`deploy_trigger::GitDeployDriftObserver`]. When
    /// present AND autonomous deploy is enabled AND the process-global throttle
    /// admits this tick, `run_cycle` observes drift (fail-safe) and surfaces a
    /// `Signal::DeployDriftDetected` so Decide can emit a guarded deploy.
    deploy_drift_observer: Option<Box<dyn deploy_trigger::DeployDriftObserver>>,
}

/// The reaper's injected dependencies, bundled so the `Overseer` carries one
/// optional field. Wired in `build_overseer`; faked in tests via the pure
/// [`claim_reaper::reap_stale_claims`] entry point.
struct ClaimReaperSeams {
    ledger: Box<dyn claim_reaper::ClaimLedger>,
    probe: Box<dyn claim_reaper::ClaimLivenessProbe>,
    cleanup: Box<dyn claim_reaper::OrphanWorktreeCleanup>,
    investigator: Box<dyn claim_reaper::StaleEngineerInvestigator>,
}

/// The result of one meta-OODA turn. Side-effect free: it reports what was
/// observed and what WOULD be done. Act (M2+) executes only the admitted items.
#[derive(Clone, Debug, PartialEq)]
pub struct CycleReport {
    pub observed: ObservedState,
    pub signals: Vec<Signal>,
    pub problems: Vec<Problem>,
    pub plan: Vec<PlannedIntervention>,
    /// Per-problem feed entries (issue #2635): problem + WHY + action +
    /// root/symptom, one per problem, index-aligned with `problems`/`plan`.
    pub entries: Vec<ProblemEntry>,
}

/// Outcome of dispatching one intervention through its capability. Returned by
/// `act` (the M2+ execution seam), exercised here only in tests with fakes.
#[derive(Clone, Debug, PartialEq)]
pub enum ActOutcome {
    Launched(WorkstreamHandle),
    Merged,
    ConflictResolved,
    Deployed(DeployReport),
    IssueFiled(IssueOutcome),
    GoalTransferred,
    Reported,
    Audited,
    Escalated,
    /// A lightweight advisory whisper was delivered into Simard's OODA inbox.
    Whispered {
        path: PathBuf,
        signature: String,
    },
    /// A whisper was suppressed by the [`WhisperGate`] (duplicate within the
    /// dedup window, or the per-hour cap was reached) — not re-injected.
    WhisperSuppressed {
        reason: &'static str,
    },
    /// A false-parked standing/perpetual goal was auto-unblocked + reactivated
    /// (the `simard goal unblock` operation) by the self-heal path.
    GoalUnblocked {
        goal_id: String,
    },
    /// A genuinely-blocked "needs human review" goal was escalated to the
    /// operator on both channels (email + Signal).
    GoalEscalated {
        goal_id: String,
    },
    /// A goal-board self-heal / escalation was suppressed by the dedup gate
    /// (duplicate within the window, or the per-hour cap was reached).
    GoalHealthSuppressed {
        reason: &'static str,
    },
    /// The recurring backlog-coverage gap-scan flagged (and/or suppressed)
    /// backlog gaps this act. `flagged` gaps got the consolidated operator
    /// notification + one deduped issue each; `suppressed` gaps were within the
    /// per-gap dedup window (a recurring gap, not re-notified/re-filed). Feeds the
    /// DEDICATED tick counters — never the generic `issues_filed`/`escalations`.
    WorkstreamGapsFlagged {
        flagged: usize,
        suppressed: usize,
    },
    /// An open PR judged STALE by the agentic merge-queue reasoner (#4097) was
    /// flagged with a `gh pr comment` (never a merge/close).
    StalePrFlagged {
        repo: String,
        pr: u32,
    },
    /// An open PR judged a DUPLICATE by the agentic reasoner (#4097) was closed
    /// with `gh pr close`, referencing the original.
    DuplicatePrClosed {
        repo: String,
        pr: u32,
        duplicate_of: u32,
    },
}

/// The `target_repo` marker attached to ecosystem-observe launches. The
/// AUTHORITATIVE repo for each surfaced Problem is named in the agent's brief
/// prose (per `observe.md` / `problem_to_brief.md`); this stable
/// observation-origin slug only feeds the launcher's dedup signature and hint,
/// never a `cd`/clone. Keeping it constant lets `recipe_dedup_key` dedup off the
/// brief content rather than a churning per-repo target.
const ECOSYSTEM_OBSERVE_TARGET: &str = "rysweet/Simard";

impl Overseer {
    /// Construct with default guardrails (HIGH-RISK gated, default daily budget).
    pub fn new(caps: Capabilities) -> Self {
        Self {
            caps,
            autonomy: AutonomyGate::default(),
            recursion: RecursionGuard::default(),
            budget: BudgetGate::default(),
            sequencer: ConflictSequencer::default(),
            max_launches_per_cycle: 2,
            whisper_sink: None,
            // 15-minute dedup window; at most 5 whispers per rolling hour.
            whisper_gate: WhisperGate::new(900, 5),
            whisper_enabled: false,
            notifier: None,
            // Blocked-goal escalation uses EXPONENTIAL BACKOFF (issue #4255): a
            // persistently-blocked goal is escalated once, then its per-goal
            // signature is suppressed for a window that DOUBLES each re-fire
            // (15m → 30m → 1h → … capped at 4h) instead of re-escalating every
            // ~15-minute tick. A change in the blocked-goal SET presents as a new
            // per-goal signature that fires immediately. A generous per-hour cap
            // covers many distinct goals without flooding.
            blocked_goal_gate: WhisperGate::with_backoff(900, 14_400, 20),
            goal_health_enabled: false,
            // Off by default in the bare constructor; the daemon enables it from
            // `config::memory_recall_enabled`. 15-minute dedup window so a
            // persistent observation signature is written back at most once per
            // window (never per-tick), with a generous per-hour cap.
            memory_recall_enabled: false,
            write_back_gate: WhisperGate::new(900, 5),
            gap_scan_enabled: false,
            // 15-minute dedup window so a recurring gap notifies/files once per
            // window; a generous per-hour cap covers a maxed-out tick's distinct
            // gaps (bounded by `sensor::MAX_GAPS_PER_TICK`) without flooding.
            gap_gate: WhisperGate::new(900, 200),
            memory: None,
            inflight_investigations: HashMap::new(),
            claim_reap_enabled: false,
            claim_reap_stale_secs: config::DEFAULT_CLAIM_REAP_STALE_SECS,
            claim_reaper: None,
            pending_reaper_interventions: Vec::new(),
            ecosystem_observer: None,
            ecosystem_roster: Vec::new(),
            ecosystem_every_n: 1,
            ecosystem_tick: 0,
            clock: Box::new(now_secs),
            coverage_backoff: BackoffGate::new(
                config::overseer_backoff_base_secs(),
                config::overseer_backoff_multiplier(),
                config::overseer_backoff_max_secs(),
            ),
            merge_queue_reasoner: None,
            merge_queue_roster: Vec::new(),
            merge_queue_every_n: 1,
            merge_queue_tick: 0,
            merge_reasoning_disabled_notified: false,
            health_reviewer: None,
            health_review_enabled: false,
            health_review_every_n: 1,
            health_review_tick: 0,
            deploy_drift_observer: None,
        }
    }

    /// Wire the Simard Whisperer's delivery seam (advisory steering notes).
    pub fn with_whisper_sink(mut self, sink: Box<dyn WhisperSink>) -> Self {
        self.whisper_sink = Some(sink);
        self
    }

    /// Wire the operator-notification seam (email + Signal) used by the
    /// ESCALATE-blocked-goal path. `None` until wired; escalation requires it.
    pub fn with_operator_notifier(mut self, notifier: Box<dyn OperatorNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Enable/disable goal-board health handling (self-heal + escalate). Off by
    /// default; the daemon sets this from [`config::goal_health_enabled`].
    pub fn with_goal_health_enabled(mut self, enabled: bool) -> Self {
        self.goal_health_enabled = enabled;
        self
    }

    /// Enable/disable cognitive-memory recall (issue #2628). Off by default; the
    /// daemon sets this from [`config::memory_recall_enabled`]. When off the
    /// Overseer behaves exactly as before — no read, no write-back, counters `0`.
    pub fn with_memory_recall_enabled(mut self, enabled: bool) -> Self {
        self.memory_recall_enabled = enabled;
        self
    }

    /// Enable/disable the recurring backlog-coverage gap-scan. Off by default;
    /// the daemon sets this from [`config::gap_scan_enabled`] (and its every-N
    /// cadence). When off, observed gaps are held (never notified/filed).
    pub fn with_gap_scan_enabled(mut self, enabled: bool) -> Self {
        self.gap_scan_enabled = enabled;
        self
    }

    /// Wire the live agentic ecosystem-observe rail (issue #2419): the stewarded
    /// `roster` the OBSERVE agent scans, the [`ecosystem_observe::EcosystemObserver`]
    /// seam that invokes the recipe, and the every-N cadence. Absent by default;
    /// `build_overseer` wires it with the committed roster + a production
    /// recipe-runner. Gated by [`Self::with_gap_scan_enabled`] — it REPLACES the
    /// retired single-repo gap-scan survey as the observation source, so the same
    /// `SIMARD_OVERSEER_GAP_SCAN` opt-out disables it and the same
    /// `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` sets its cadence.
    pub fn with_ecosystem_observer(
        mut self,
        roster: Vec<String>,
        observer: Box<dyn ecosystem_observe::EcosystemObserver>,
        every_n: u64,
    ) -> Self {
        self.ecosystem_roster = roster;
        self.ecosystem_observer = Some(observer);
        self.ecosystem_every_n = every_n;
        self
    }

    /// Wire the live agentic observe/orient merge-queue reasoner rail (#4097):
    /// the governed `roster` the reasoning agent surveys, the
    /// [`merge_queue_observe::MergeQueueReasoner`] seam that invokes the
    /// `observe-merge-queue` recipe, and the every-N cadence. Absent by default;
    /// `build_overseer` wires it with the committed roster + a production
    /// recipe-runner. Gated by [`Self::with_gap_scan_enabled`] (the same opt-out
    /// family as the ecosystem pass); the resolved reasoning SCOPE is default-ON
    /// over the roster and only an EXPLICIT `SIMARD_MERGE_REASONING_SCOPE` disable
    /// turns it off (loud).
    pub fn with_merge_queue_reasoner(
        mut self,
        roster: Vec<String>,
        reasoner: Box<dyn merge_queue_observe::MergeQueueReasoner>,
        every_n: u64,
    ) -> Self {
        self.merge_queue_roster = roster;
        self.merge_queue_reasoner = Some(reasoner);
        self.merge_queue_every_n = every_n;
        self
    }

    /// Enable/disable the whisperer (config opt-out). Off by default; the daemon
    /// sets this from [`config::whisper_enabled`].
    pub fn with_whisper_enabled(mut self, enabled: bool) -> Self {
        self.whisper_enabled = enabled;
        self
    }

    /// Wire the live agentic health-review rail ([standing]): the thin
    /// [`health_review::HealthReviewer`] seam that invokes the
    /// `overseer-health-review` recipe and the every-N cadence. Absent by
    /// default; `build_overseer` wires it with a production recipe-runner. Gated
    /// by both the dedicated [`Self::with_health_review_enabled`] opt-out and the
    /// shared [`Self::with_gap_scan_enabled`] scan throttle.
    pub fn with_health_reviewer(
        mut self,
        reviewer: Box<dyn health_review::HealthReviewer>,
        every_n: u64,
    ) -> Self {
        self.health_reviewer = Some(reviewer);
        self.health_review_every_n = every_n;
        self
    }

    /// Enable/disable the agentic health-review rail (dedicated config opt-out
    /// `SIMARD_OVERSEER_HEALTH_REVIEW`). Off by default; the daemon sets this from
    /// [`config::health_review_enabled`]. When off, the pass is skipped even while
    /// the shared gap-scan throttle is on.
    pub fn with_health_review_enabled(mut self, enabled: bool) -> Self {
        self.health_review_enabled = enabled;
        self
    }

    /// Opt into autonomous HIGH-RISK execution (deploy / conflict-resolution).
    /// Off by default: those interventions escalate instead.
    pub fn with_high_risk_autonomy(mut self, allow: bool) -> Self {
        self.autonomy.allow_high_risk = allow;
        self
    }

    /// Opt into autonomous PR verify-and-merge (crusty risk #1). Off by default:
    /// `VerifyAndMergePr` escalates until the operator explicitly enables it,
    /// once M1's signal quality is proven. Independent of HIGH-RISK autonomy.
    pub fn with_verify_merge_autonomy(mut self, allow: bool) -> Self {
        self.autonomy.allow_verify_merge = allow;
        self
    }

    /// Set the Overseer's own identity so anti-recursion can refuse its own work.
    pub fn with_identity(mut self, guard: RecursionGuard) -> Self {
        self.recursion = guard;
        self
    }

    /// Inject the wall-clock seam (Unix seconds) the gap-scan dedup backoff reads
    /// (Problem 1 / issue #4186). Defaults to real wall-clock; tests inject a
    /// deterministic virtual clock so the exponential backoff window is testable.
    pub fn with_clock(mut self, clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    /// Wire the cognitive-memory handle (amplihack-memory-lib, G2) used by the
    /// root-cause analysis to recall prior occurrences of a problem's root cause
    /// and record new ones. `None` until wired: the analysis degrades gracefully
    /// to telemetry-only WHYs (never a silent failure).
    pub fn with_memory(mut self, memory: Arc<dyn CognitiveMemoryOps>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Wire the periodic stale-engineer-claim reaper (issue #4099): the ledger it
    /// sweeps + reclaims through (the shared release chokepoint), the liveness
    /// probe (worktree presence + newest-file mtime), the orphan-worktree
    /// cleanup, and the resolved config (enabled + staleness threshold). Absent
    /// by default; `build_overseer` wires the production seams. When wired,
    /// [`Overseer::run_cycle`] sweeps every `engineer_claims` row each tick and
    /// reclaims those whose engineer is provably dead — INDEPENDENT of per-goal
    /// polling — closing the within-incarnation claim leak.
    pub fn with_claim_reaper(
        mut self,
        ledger: Box<dyn claim_reaper::ClaimLedger>,
        probe: Box<dyn claim_reaper::ClaimLivenessProbe>,
        cleanup: Box<dyn claim_reaper::OrphanWorktreeCleanup>,
        investigator: Box<dyn claim_reaper::StaleEngineerInvestigator>,
        enabled: bool,
        stale_secs: u64,
    ) -> Self {
        self.claim_reap_enabled = enabled;
        self.claim_reap_stale_secs = stale_secs;
        self.claim_reaper = Some(ClaimReaperSeams {
            ledger,
            probe,
            cleanup,
            investigator,
        });
        self
    }

    /// Wire the autonomous self-deploy drift sensor (issue #2590). `build_overseer`
    /// injects the production [`deploy_trigger::GitDeployDriftObserver`]; tests
    /// inject a fake so the OBSERVE→DECIDE→ACT rail can be exercised without a
    /// live git checkout. Absent by default (the rail is inert, exactly as before
    /// the wiring).
    pub fn with_deploy_drift_observer(
        mut self,
        observer: Box<dyn deploy_trigger::DeployDriftObserver>,
    ) -> Self {
        self.deploy_drift_observer = Some(observer);
        self
    }

    /// OBSERVE the autonomous self-deploy drift signal (issue #2590), enriching
    /// `observed.deploy_drift` when the running binary is behind merged `main`.
    ///
    /// This is the thin deterministic OBSERVE rail that connects the already-built
    /// drift detector to the already-built guarded deployer. It is layered with
    /// four fail-closed guards, in order:
    ///
    ///   1. The documented opt-OUT env (`SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0`)
    ///      lets an operator PIN the daemon.
    ///   2. The rail is inert unless a [`DeployDriftObserver`] is wired.
    ///   3. The PROCESS-GLOBAL anti-thrash throttle admits at most one deploy
    ///      attempt per min-interval — essential because the daemon rebuilds this
    ///      `Overseer` every tick, so a drift re-observed each tick (until the
    ///      swap lands, or forever if the gate keeps refusing) can never redeploy
    ///      or re-notify every cycle.
    ///   4. A LIVE crash-loop guard: never deploy into a restart storm. Reuses the
    ///      observed restart churn and the SAME threshold the deploy gate uses.
    ///
    /// Fail-SAFE throughout: the observer maps any git/source error to "no drift",
    /// so an error surfaces no signal (never a panic, never a blind deploy).
    ///
    /// [`DeployDriftObserver`]: deploy_trigger::DeployDriftObserver
    fn observe_deploy_drift(&mut self, observed: &mut ObservedState) {
        // 1) Operator opt-out pins the daemon.
        if !deploy_trigger::autonomous_deploy_enabled() {
            return;
        }
        // 2) Inert until the sensor is wired.
        let Some(observer) = self.deploy_drift_observer.as_ref() else {
            return;
        };
        // 3) Anti-thrash: at most one probe+attempt per min-interval, process
        //    -global so the per-tick-rebuilt Overseer cannot reset it.
        let now = crate::self_quality_audit::now_epoch_secs();
        if !deploy_trigger::global_deploy_throttle_allow(
            now,
            deploy_trigger::deploy_min_interval_secs(),
        ) {
            return;
        }
        // 4) LIVE crash-loop guard: deploying into an unstable, churning process
        //    makes it worse (Bainbridge's irony). Skip when restart churn is at or
        //    above the deploy gate's crash-loop threshold.
        if let Some(churn) = observed.restart_churn
            && churn >= deploy::CRASH_LOOP_CHURN_THRESHOLD
        {
            return;
        }
        // Fail-safe drift probe: `None` on current / unresolved / any git error.
        if let Some(drift) = observer.observe() {
            observed.deploy_drift = Some(drift);
        }
    }

    /// Run one meta-OODA turn. Observe → Orient → Decide → plan+gate. Does NOT
    /// execute side effects; returns the plan for M2+ Act to run.
    pub fn run_cycle(&mut self) -> Result<CycleReport, OverseerError> {
        // Reconcile the in-flight investigation dedup set FIRST: poll each
        // launched recipe-runner investigation and free the slot of any that is
        // no longer `Running`, so a genuinely-new recurrence can be investigated
        // later. A poll error leaves the entry in place (fail closed: better to
        // skip a duplicate launch than to double-launch on a transient error) —
        // exactly the "two PIDs on one signature" defect this guard closes.
        self.reconcile_inflight_investigations();

        // The conflict sequencer serialises the coverage/sweep launches PLANNED
        // within THIS cycle (analogous to the per-cycle `launches` cap reset
        // below). Reset it here so a `sequence_group` admitted in a prior cycle
        // does not permanently lock out later coverage of that group — cross
        // -cycle dedup of an identical recurring launch is the in-flight guard's
        // and the coverage backoff's job, not the sequencer's.
        self.sequencer.reset();

        // Periodic stale-engineer-claim reaper (issue #4099): a synchronous,
        // thread-less sweep alongside the reconcile above. Reclaims every
        // `engineer_claims` row whose engineer is provably dead (no worktree, or
        // an idle worktree stale beyond the threshold), INDEPENDENT of whether
        // that goal is being advanced — the gap PR #4095's per-collision /
        // per-goal reclaim paths never close. Fail-closed, fail-visible; a
        // per-claim error is contained so it can never abort the tick.
        self.reap_stale_engineer_claims();

        // Observe.
        let mut observed = self.caps.status.snapshot()?;
        // Enrich with goal-board health AND the in-flight dedup set from a
        // SINGLE board read: the goals currently BLOCKED on Simard's board
        // (false-parks + genuine "needs human review" blocks) plus Simard's
        // in-flight work. Read-only; a board-read failure degrades both to
        // empty (never aborts the cycle), and reading them together avoids
        // deserializing the same snapshot twice this tick.
        let (blocked_goals, in_flight) = self.caps.goals.observe_board().unwrap_or_default();
        observed.blocked_goals = blocked_goals;

        // Enrich with the recurring backlog-coverage gap-scan: important work
        // that SHOULD have an active workstream but does not, already correlated
        // against in-flight workstreams / open PRs so only GENUINE gaps appear.
        // Read-only; a survey failure degrades to "no gaps", never aborts. The
        // gap-scan action itself is gated (held) when the scan is disabled.
        observed.workstream_gaps = self
            .caps
            .goals
            .workstream_gaps(&observed.anomalies)
            .unwrap_or_default();

        // Enrich with the autonomous-self-merge candidate survey (#4097): the
        // thin deterministic sensor rail that LISTS Simard's OWN green +
        // MERGEABLE PRs in the explicit `SIMARD_AUTOMERGE_REPOS` allowlist. This
        // is the dead-wire fix — populating `ready_prs` re-activates the already
        // -built `PrReadyToMerge`/`VerifyAndMergePr` merge machinery. The
        // allowlist is EMPTY by default => no candidates => autonomous merge
        // stays OFF until an operator canary-enables one repo. The rail only
        // LISTS candidates; the authoritative six-criteria gate stays downstream
        // in `merge_authority`. A survey failure degrades to empty inside the
        // ops layer (fail-visible), never aborts the cycle.
        observed.ready_prs = self.caps.prs.survey_ready_prs(&config::automerge_repos());

        // Agentic observe/orient merge-queue + issue reasoning (#4097). This is
        // the fix for the dead-wire allowlist above: the imperative
        // `survey_ready_prs` rail is EMPTY when `SIMARD_AUTOMERGE_*` are unset, so
        // it reasoned about ZERO open PRs. This pass instead REASONS agentically
        // over the governed roster (default-ON) each due tick — surveying open
        // PRs (CI/mergeable/review/staleness/duplication) and issues (priority/
        // readiness/next-action) — and populates `reasoned_prs` / `triaged_issues`
        // + a re-narrowed `ready_prs` projection. Reasoning is BROAD; merge
        // AUTHORIZATION stays narrow behind `project_ready_prs` + the downstream
        // merge-authority gate. Placed BEFORE `signals_from` so the reasoned state
        // flows through the SAME signals → orient → decide → gate pipeline every
        // other observation uses. Fail-closed: an unwired rail / off cadence /
        // empty scope / degraded recipe leaves the observation unchanged.
        self.observe_merge_queue(&mut observed, &in_flight);

        // Drain diagnosed step failures (#2640, PART 2) from the process-global
        // failure sink into this Observe pass, so a caught decision-cycle /
        // engineer / terminal-shell failure surfaces as a corrective
        // `Signal::StepFailureDiagnosed` the Orient/Decide loop acts on — a fix,
        // not a silent log line. Draining here (not in the pure
        // `observed_from_snapshot` projection) keeps that projection side-effect
        // free; the sink is bounded so this is O(capacity).
        observed.recent_step_failures = failure_sink::drain_recent();

        // Autonomous self-deploy OBSERVE rail (issue #2590): when the running
        // binary is behind merged `main`, enrich `observed.deploy_drift` so
        // `signals_from` raises a `Signal::DeployDriftDetected` that Decide maps
        // to a guarded `Intervention::Deploy`. Opt-out, anti-thrash-throttled,
        // crash-loop-guarded, and fail-safe (see `observe_deploy_drift`); inert
        // until `build_overseer` wires the production drift sensor.
        self.observe_deploy_drift(&mut observed);

        // Cognitive-memory recall (#2628) — the USE step. Key off the pre-recall
        // signals/problems (never a full-graph scan), then run ONE whole-pass,
        // fail-closed recall on the shared handle. Best-effort: the calls are
        // count-bounded and run on this panic-isolated tick thread, so recall can
        // never stall or crash the loop. A memory error is SURFACED on
        // `recall_error` (recall stays `None` — no silent fallback), never
        // swallowed into an empty snapshot.
        if self.memory_recall_enabled {
            let pre_signals = signals_from(&observed);
            let pre_problems = orient(&pre_signals, &in_flight);
            let keys = RecallKeys::from_signals(&pre_signals, &pre_problems);
            match self.recall_pass(&keys) {
                Ok(snapshot) => observed.recall = Some(snapshot),
                Err(e) => {
                    tracing::warn!(
                        target: "overseer::memory",
                        error = %e,
                        "overseer memory recall failed — surfaced, loop continues (no silent fallback)"
                    );
                    observed.recall_error = Some(e.to_string());
                }
            }
        }

        // Signals (now including any recall-derived RecurringSignature) + Orient.
        let signals = signals_from(&observed);

        // Orient (dedup against Simard's in-flight work; a board read failure
        // above already degraded `in_flight` to empty, i.e. "no dedup", never
        // aborting the cycle). Orient stays PURE — every problem it emits has
        // `why = None`.
        let mut problems = orient(&signals, &in_flight);

        // MANDATORY ROOT-CAUSE enrichment (issue #2635): for EVERY problem,
        // determine WHY before deciding. Best-effort read-only recall of prior
        // same-signature occurrences (amplihack-memory-lib, G2) feeds the
        // structured analyzer; when memory is unavailable the WHY degrades to a
        // telemetry-only analysis (never a silent failure — a WHY is always
        // produced and honestly labelled).
        for problem in &mut problems {
            let recall = self.recall_occurrences(&problem.dedup_key);
            let why = root_cause::analyze(problem, &observed, &recall);
            problem.why = Some(why);
        }

        // Decide + gate + classify each action's remediation, and build the
        // per-problem feed entries (problem + WHY + action + root/symptom).
        let mut plan = Vec::with_capacity(problems.len());
        let mut entries = Vec::with_capacity(problems.len());
        let mut launches = 0usize;
        for problem in &problems {
            let iv = decide(problem);
            let mut planned = self.gate(&iv, &observed, &mut launches);
            let why = problem.why.clone().unwrap_or_else(RootCause::unknown);
            let remediation = remediation_for(&iv, &why);
            planned.remediation = remediation.clone();
            entries.push(ProblemEntry {
                key: problem.dedup_key.clone(),
                summary: problem.summary.clone(),
                why,
                action: iv.label().to_string(),
                remediation,
            });
            plan.push(planned);
        }

        // Live agentic ecosystem-observe (issue #2419): on the Overseer cadence,
        // the thin rail invokes the `ecosystem-observe` recipe — an AGENT scans
        // the stewarded roster with `gh` and REASONS to a prioritized, deduped
        // Problem list, then briefs each into a `smart-orchestrator`
        // task_description. Rust never queries or parses a repo; it only schedules
        // the recipe and routes the agent's OPAQUE semantic brief into the SAME
        // gated launch path every fix uses. Replaces the retired single-repo Rust
        // gap-scan survey as the cross-repo observation SOURCE.
        self.observe_ecosystem(&observed, &in_flight, &mut launches, &mut plan);

        // Live agentic health-review ([standing]): on the Overseer cadence, the
        // thin rail invokes the `overseer-health-review` recipe — an AGENT reads
        // the OODA journal + `simard status` + `simard goal list`, detects
        // crash-loops / shared failure signatures (systemic-vs-per-goal root
        // cause), and REASONS to typed remediation DECISIONS. Rust never reads the
        // journal, counts a failure, or encodes a threshold; it only schedules the
        // recipe and routes each parsed `LaunchRecipe` / `EscalateBlockedGoal`
        // through the SAME gate every other action uses.
        self.health_review(&observed, &mut launches, &mut plan);

        // Drain the stale-engineer reaper's staged investigation interventions
        // (issue #4400) into the SAME gated plan — a stale engineer's death feeds
        // self-improvement (issue/escalation/fix) instead of a silent reclaim.
        self.dispatch_reaper_interventions(&observed, &mut launches, &mut plan);

        Ok(CycleReport {
            observed,
            signals,
            problems,
            plan,
            entries,
        })
    }

    /// One live agentic ecosystem-observe pass (issue #2419), appended to the
    /// cycle plan when the rail is wired and due on the cadence.
    ///
    /// The observation itself is entirely agentic: the thin rail invokes the
    /// `ecosystem-observe` recipe, whose OBSERVE agent runs `gh` across the
    /// stewarded roster and REASONS to a deduped Problem list, and whose BRIEF
    /// agent turns each Problem into a `smart-orchestrator` `task_description`.
    /// Rust never queries or parses a repo — it receives only the agent's OPAQUE
    /// semantic brief and routes it VERBATIM into a gated [`Intervention::LaunchRecipe`]
    /// (the SAME budget / launch-cap / sequencer / in-flight-dedup / recursion
    /// guard as every other launch).
    ///
    /// Fail-closed: an unwired observer, an off cadence, an empty roster, or a
    /// "nothing actionable" / failed recipe run all leave the plan unchanged —
    /// never a fabricated launch.
    fn observe_ecosystem(
        &mut self,
        observed: &ObservedState,
        in_flight: &[InFlightItem],
        launches: &mut usize,
        plan: &mut Vec<PlannedIntervention>,
    ) {
        if self.ecosystem_observer.is_none() {
            return;
        }
        // Cadence: reuse the gap-scan enable + every-N gate (this pass replaced
        // the gap-scan survey as the observation source). Advance the tick FIRST
        // so a disabled/held pass still keeps the counter monotonic.
        let tick = self.ecosystem_tick;
        self.ecosystem_tick = self.ecosystem_tick.wrapping_add(1);
        if !ecosystem_observe::should_observe(self.gap_scan_enabled, self.ecosystem_every_n, tick) {
            return;
        }
        // Flatten Simard's in-flight OODA refs so the agent dedups against work
        // an engineer already owns and never duplicates her OODA.
        let inflight_refs: Vec<String> = in_flight
            .iter()
            .flat_map(|item| item.refs.iter().cloned())
            .collect();
        // Borrow the observer only for the call; the returned brief is owned, so
        // the immutable borrow of `self` ends before the mutable `gate` below.
        let outcome = {
            let observer = self
                .ecosystem_observer
                .as_ref()
                .expect("ecosystem_observer presence checked above");
            observer.observe(&self.ecosystem_roster, &inflight_refs)
        };
        let brief = match outcome {
            Ok(Some(brief)) => brief,
            Ok(None) => return, // nothing actionable this pass — fabricate nothing
            Err(e) => {
                tracing::warn!(
                    target: "overseer::ecosystem_observe",
                    error = %e,
                    "ecosystem-observe pass failed — degrading to no observation (no problems fabricated)"
                );
                return;
            }
        };
        // Route the agent's OPAQUE brief into the SAME gated launch path. The
        // brief prose names its own target repo (per `observe.md`); the
        // launcher's `target_repo` stays the stable observation-origin marker,
        // and per-launch dedup keys off the brief content via `recipe_dedup_key`.
        let iv = Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: brief,
                target_repo: ECOSYSTEM_OBSERVE_TARGET.to_string(),
                sequence_group: None,
            },
        };
        let planned = self.gate(&iv, observed, launches);
        plan.push(planned);
    }

    /// One live agentic health-review pass ([standing]), appended to the cycle
    /// plan when the rail is wired and due on the cadence.
    ///
    /// The observation AND the remediation judgment are entirely agentic: the
    /// thin rail invokes the `overseer-health-review` recipe, whose agent reads
    /// the OODA journal (`journalctl --user -u <unit>`), `simard status`, and
    /// `simard goal list`, detects crash-loops and clusters a shared failure
    /// signature across goals into a systemic-vs-per-goal root cause, and REASONS
    /// to typed remediation DECISIONS — `LaunchRecipe` for a fix it can drive, or
    /// `EscalateBlockedGoal` to notify the operator in plain English. Rust never
    /// reads the journal, counts a failure, or encodes an N-identical-failure
    /// threshold; it only schedules the recipe and routes each parsed
    /// [`Intervention`] through the SAME gate (budget / launch-cap / sequencer /
    /// in-flight-dedup / recursion guard) every other action uses.
    ///
    /// Fail-closed: an unwired reviewer, a disabled rail, an off cadence, a
    /// `HEALTHY` verdict, or a degraded/failed recipe run all leave the plan
    /// unchanged — never a fabricated launch or escalation.
    fn health_review(
        &mut self,
        observed: &ObservedState,
        launches: &mut usize,
        plan: &mut Vec<PlannedIntervention>,
    ) {
        if self.health_reviewer.is_none() || !self.health_review_enabled {
            return;
        }
        // Cadence: reuse the shared gap-scan enable throttle + the dedicated
        // every-N knob (same family as the ecosystem/merge-queue passes). Advance
        // the tick FIRST so a disabled/held pass still keeps the counter monotonic.
        let tick = self.health_review_tick;
        self.health_review_tick = self.health_review_tick.wrapping_add(1);
        if !ecosystem_observe::should_observe(
            self.gap_scan_enabled,
            self.health_review_every_n,
            tick,
        ) {
            return;
        }
        // Borrow the reviewer only for the call; the returned Vec is owned, so the
        // immutable borrow of `self` ends before the mutable `gate` below.
        let outcome = {
            let reviewer = self
                .health_reviewer
                .as_ref()
                .expect("health_reviewer presence checked above");
            reviewer.review()
        };
        let interventions = match outcome {
            Ok(ivs) => ivs,
            Err(e) => {
                tracing::warn!(
                    target: "overseer::health_review",
                    error = %e,
                    "health-review pass failed — degrading to no review (no interventions fabricated)"
                );
                return;
            }
        };
        // Route each parsed DECISION (LaunchRecipe / EscalateBlockedGoal) through
        // the SAME gate every other action uses. A `HEALTHY` verdict parses to an
        // empty Vec, so this loop simply adds nothing.
        for iv in interventions {
            let planned = self.gate(&iv, observed, launches);
            plan.push(planned);
        }
    }
    /// enriching `observed` with `reasoned_prs` / `triaged_issues` /
    /// `merge_reasoning_status` and a re-narrowed `ready_prs` projection when the
    /// rail is wired and due on the cadence.
    ///
    /// The reasoning itself is entirely agentic: the thin rail invokes the
    /// `observe-merge-queue` recipe, whose agent runs `gh` across the governed
    /// roster and REASONS to a bounded brief. Rust never queries or parses a repo
    /// here — it receives the agent's OPAQUE brief, parses it FAIL-CLOSED against
    /// the roster trust boundary ([`merge_queue_observe::parse_merge_queue_brief`]),
    /// and re-narrows the `ReadyForMerge` proposals through the DETERMINISTIC rail
    /// ([`PrOps::project_reasoned_ready_prs`] → [`project_ready_prs`]) before any
    /// PR reaches the merge gate. Reasoning is BROAD; authorization is NARROW.
    ///
    /// Scope resolution (R2/R3): default-ON over the governed roster when
    /// `SIMARD_MERGE_REASONING_SCOPE` is unset; narrowed (still on) when it names
    /// an explicit subset; and LOUD-disabled (WARN log + status field + a one-time
    /// dual-channel NotifyOperator) only when it is EXPLICITLY set to a disable
    /// value. Unset is never a silent hard-OFF.
    ///
    /// Fail-closed: an unwired reasoner, an off cadence, an empty scope, or a
    /// degraded recipe run leaves `observed` unchanged (no fabricated reasoning).
    fn observe_merge_queue(&mut self, observed: &mut ObservedState, in_flight: &[InFlightItem]) {
        if self.merge_queue_reasoner.is_none() {
            return;
        }
        // Cadence: reuse the gap-scan enable + every-N gate (same opt-out family
        // as the ecosystem pass). Advance the tick FIRST so a disabled/held pass
        // still keeps the counter monotonic.
        let tick = self.merge_queue_tick;
        self.merge_queue_tick = self.merge_queue_tick.wrapping_add(1);
        // Resource opt-out (SIMARD_OVERSEER_GAP_SCAN): the shared throttle for ALL
        // agentic overseer scans. Honor it, but make it VISIBLE — #4097 exists to
        // kill silent hard-OFFs, so surface WHY reasoning stopped in the status
        // rather than leaving it a silent `Unknown`. (No one-time NotifyOperator
        // here — that is reserved for the R3 explicit scope-disable; the operator
        // set this opt-out themselves, so the status breadcrumb is enough.)
        if !self.gap_scan_enabled {
            observed.merge_reasoning_status = MergeReasoningStatus::Disabled {
                reason: format!(
                    "{} opt-out disables all agentic overseer scans (incl. merge-queue reasoning)",
                    config::SIMARD_OVERSEER_GAP_SCAN_ENV
                ),
            };
            return;
        }
        // Cadence only: a non-due tick is NOT disabled — skip silently and leave
        // the last due tick's status untouched.
        if !ecosystem_observe::should_observe(true, self.merge_queue_every_n, tick) {
            return;
        }

        // Resolve the reasoning SCOPE (R2/R3). Default-ON over the roster; an
        // EXPLICIT operator disable is LOUD, never a silent hard-OFF.
        let scope = config::merge_reasoning_scope(&self.merge_queue_roster);
        let scope_repos: Vec<String> = match scope {
            config::MergeReasoningScope::Disabled => {
                let reason = format!(
                    "{} explicitly set to a disable value",
                    config::SIMARD_MERGE_REASONING_SCOPE_ENV
                );
                observed.merge_reasoning_status = MergeReasoningStatus::Disabled {
                    reason: reason.clone(),
                };
                tracing::warn!(
                    target: "overseer::merge",
                    reason = %reason,
                    "observe-merge-queue: agentic merge-queue reasoning DISABLED by explicit \
                     operator configuration — no open PRs/issues will be reasoned about this tick \
                     (loud, not a silent hard-OFF)"
                );
                self.notify_merge_reasoning_disabled_once(&reason);
                return;
            }
            config::MergeReasoningScope::Roster => {
                observed.merge_reasoning_status = MergeReasoningStatus::RosterWide;
                self.merge_queue_roster.clone()
            }
            config::MergeReasoningScope::Explicit(list) => {
                observed.merge_reasoning_status = MergeReasoningStatus::Narrowed {
                    repos: list.clone(),
                };
                list
            }
        };
        // A configured-but-empty scope is nothing to reason about; the rail's own
        // empty-scope guard also fails closed, but returning here avoids spawning
        // the recipe for no repos.
        if scope_repos.is_empty() {
            return;
        }

        // Flatten Simard's in-flight OODA refs so the agent dedups against work an
        // engineer already owns and never re-proposes it.
        let inflight_refs: Vec<String> = in_flight
            .iter()
            .flat_map(|item| item.refs.iter().cloned())
            .collect();

        let request = merge_queue_observe::MergeQueueObserveRequest {
            scope: scope_repos.clone(),
            inflight_refs,
            escalation_note: String::new(),
        };
        // Borrow the reasoner only for the call; the returned brief is owned, so
        // the immutable borrow of `self` ends before the mutable projection below.
        let outcome = {
            let reasoner = self
                .merge_queue_reasoner
                .as_ref()
                .expect("merge_queue_reasoner presence checked above");
            reasoner.observe(request)
        };
        let brief = match outcome {
            Ok(Some(brief)) => brief,
            Ok(None) => return, // nothing actionable this pass — fabricate nothing
            Err(e) => {
                tracing::warn!(
                    target: "overseer::merge",
                    error = %e,
                    "observe-merge-queue pass failed — degrading to no reasoning (no PRs/issues fabricated)"
                );
                return;
            }
        };

        // Parse the OPAQUE brief FAIL-CLOSED against the roster trust boundary.
        let parsed = merge_queue_observe::parse_merge_queue_brief(&brief, &scope_repos);
        observed.reasoned_prs = parsed.reasoned_prs;
        observed.triaged_issues = parsed.triaged_issues;

        // Re-narrow the BROAD `ReadyForMerge` proposals into the NARROW set of PRs
        // authorized to merge, against AUTHORITATIVE `gh` facts (#4097). This is
        // the deterministic safety rail — it can only ever REMOVE candidates. The
        // result becomes a DERIVED view of `ready_prs` (unioned with the existing
        // deterministic survey), the ONLY thing that drives `PrReadyToMerge` into
        // the gated merge chain. An agent can never widen merge authorization.
        let overseer_login = self.recursion.author_login.clone();
        let projected = self
            .caps
            .prs
            .project_reasoned_ready_prs(&observed.reasoned_prs, &overseer_login);
        for pr in projected {
            if !observed.ready_prs.contains(&pr) {
                observed.ready_prs.push(pr);
            }
        }
    }

    /// Send the R3 "merge reasoning DISABLED" alert to the operator on BOTH
    /// channels exactly ONCE (#4097), latched so a persistent disable is surfaced
    /// loud a single time rather than re-notified every tick. No-op when no
    /// notifier is wired (the WARN log + status field still surface it).
    fn notify_merge_reasoning_disabled_once(&mut self, reason: &str) {
        if self.merge_reasoning_disabled_notified {
            return;
        }
        if let Some(notifier) = self.notifier.as_ref() {
            let notification = OperatorNotification::merge_reasoning_disabled(reason);
            let report = notifier.notify(&notification);
            // Latch regardless of per-channel delivery result: the WARN log and
            // status field already carry it, and re-sending every tick would spam
            // the operator. A failed channel is recorded in the report/logs.
            let _ = report;
        }
        self.merge_reasoning_disabled_notified = true;
    }
    /// bounded semantic + episodic + procedural + prospective reads keyed off the
    /// cycle's `keys`. The `?` on each sub-read makes the pass **atomic**: if any
    /// one read errors, the whole pass returns `Err` and the successful reads are
    /// discarded — the Overseer never orients on a partially-recalled view of
    /// memory. Every result is size-bounded by [`RecallBudget`] so a slow or huge
    /// graph degrades to "bounded read", never a stall.
    fn recall_pass(&self, keys: &RecallKeys) -> Result<MemorySnapshot, OverseerError> {
        let budget = RecallBudget::default();
        let facts = self.caps.memory.recall_semantic(keys, budget.semantic)?;
        let episodes = self.caps.memory.recall_episodic(keys, budget.episodic)?;
        let procedures = self
            .caps
            .memory
            .recall_procedural(keys, budget.procedural)?;
        let prospectives = self
            .caps
            .memory
            .recall_prospective(keys, budget.prospective)?;
        Ok(MemorySnapshot {
            facts,
            episodes,
            procedures,
            prospectives,
        })
    }

    /// Deliberately write the Overseer's own observation back into cognitive
    /// memory as ONE episodic memory (issue #2628), so its stewardship activity
    /// becomes part of the graph the rest of Simard can recall. De-duplicated via
    /// the reused [`WhisperGate`] primitive keyed by the observation signature, so
    /// a persistent condition is recorded at most once per window (never every
    /// tick). Provenance is fixed by the adapter (`source_label = "overseer"`).
    ///
    /// Returns:
    /// - `Ok(Some(RecordOutcome::Stored { .. }))` — a new episode was persisted,
    /// - `Ok(None)` — nothing to record (recall disabled, no problems) or the
    ///   write-back was de-duplicated within the window,
    /// - `Err(..)` — the backing store errored; the caller surfaces it (no silent
    ///   fallback) and the tick still completes.
    ///
    /// Only ever called when recall is enabled; the dedup slot is consumed only
    /// after a successful store so a failed write never suppresses a later one.
    pub fn write_back_observation(
        &mut self,
        problems: &[Problem],
    ) -> Result<Option<RecordOutcome>, OverseerError> {
        if !self.memory_recall_enabled {
            return Ok(None);
        }
        // Deliberate, not chatty: only record a tick that actually observed a
        // problem. A clean tick writes nothing (nothing worth recalling later).
        if problems.is_empty() {
            return Ok(None);
        }
        // D1 (issue #4128): never record an observation OF our own observation.
        // Recall-derived `overseer-obs:*` problems are the Overseer reading its
        // own prior write-back back out of cognitive memory; folding them into a
        // fresh write-back is the self-referential loop that self-amplifies the
        // recurrence counter (the live "recurring signature seen 2×" incident).
        // If a tick's ONLY problems are these self-observations, there is nothing
        // first-order to record — record nothing and break the loop.
        if !problems
            .iter()
            .any(|p| !is_recall_derived_self_observation(p))
        {
            return Ok(None);
        }
        let signature = observation_signature(problems);
        let now = now_secs();
        match self.write_back_gate.peek(&signature, now) {
            WhisperDecision::Deliver => {
                let episode = ObservationEpisode {
                    content: observation_content(problems),
                    signature: signature.clone(),
                };
                let outcome = self.caps.memory.record_observation(&episode)?;
                // Consume the dedup slot only after a successful store.
                self.write_back_gate.commit(&signature, now);
                Ok(Some(outcome))
            }
            // Duplicate within the window / per-hour cap reached: never reaches
            // the backend, nothing persisted this tick.
            _ => Ok(None),
        }
    }

    /// Free the dedup slot of every in-flight investigation whose recipe-runner
    /// is no longer `Running`, so a genuinely-new recurrence of the same
    /// signature can be investigated on a later cycle. A poll error leaves the
    /// entry in place (fail closed — never double-launch on a transient error).
    fn reconcile_inflight_investigations(&mut self) {
        if self.inflight_investigations.is_empty() {
            return;
        }
        let done: Vec<String> = self
            .inflight_investigations
            .iter()
            .filter_map(|(key, handle)| match self.caps.recipes.poll(handle) {
                Ok(WorkstreamStatus::Running) | Err(_) => None,
                Ok(_) => Some(key.clone()),
            })
            .collect();
        for key in done {
            self.inflight_investigations.remove(&key);
        }
    }

    /// Run one claim-reaper sweep if the reaper is wired (issue #4099). Delegates
    /// to the pure [`claim_reaper::reap_stale_claims`] over the injected seams so
    /// all policy + containment lives in one tested place. A no-op when the
    /// reaper is absent (bare constructor / tests) or disabled by config.
    fn reap_stale_engineer_claims(&mut self) {
        let Some(reaper) = self.claim_reaper.as_ref() else {
            return;
        };
        let summary = claim_reaper::reap_stale_claims(
            reaper.ledger.as_ref(),
            reaper.probe.as_ref(),
            reaper.cleanup.as_ref(),
            reaper.investigator.as_ref(),
            self.claim_reap_enabled,
            self.claim_reap_stale_secs,
        );
        // Stage the investigations' self-improvement interventions (issue #4400)
        // for dispatch through the SAME gated Act path health-review uses; they
        // are drained into the plan after `health_review` this tick. A stale
        // engineer's death becomes a signal, never a silent reclaim.
        if !summary.pending_interventions.is_empty() {
            self.pending_reaper_interventions
                .extend(summary.pending_interventions);
        }
        if !summary.reclaimed.is_empty() || summary.errors > 0 {
            tracing::info!(
                target: "simard::claim_reaper",
                reclaimed = summary.reclaimed.len(),
                skipped = summary.skipped,
                errors = summary.errors,
                "[simard] claim-reaper sweep complete",
            );
        }
    }

    /// Drain the stale-engineer reaper's staged investigation interventions (issue
    /// #4400) into the gated plan — through the SAME gate + push loop
    /// [`Overseer::health_review`] uses (no parallel plumbing). Each surfaced
    /// `LaunchRecipe` / `FileIssue` / `EscalateBlockedGoal` is gated and pushed so
    /// a stale engineer's investigation feeds self-improvement. A no-op when the
    /// sweep surfaced nothing.
    fn dispatch_reaper_interventions(
        &mut self,
        observed: &ObservedState,
        launches: &mut usize,
        plan: &mut Vec<PlannedIntervention>,
    ) {
        if self.pending_reaper_interventions.is_empty() {
            return;
        }
        let interventions = std::mem::take(&mut self.pending_reaper_interventions);
        for iv in interventions {
            let planned = self.gate(&iv, observed, launches);
            plan.push(planned);
        }
    }

    /// a `PlannedIntervention` (admitted or held-with-reason). The attached
    /// `remediation` is a from-intervention default; `run_cycle` overrides it with
    /// the WHY-aware classification once the problem's root cause is known.
    fn gate(
        &mut self,
        iv: &Intervention,
        observed: &ObservedState,
        launches: &mut usize,
    ) -> PlannedIntervention {
        // Whisper opt-out: when the whisperer is disabled, hold the whisper (it
        // is never delivered) even though the observed condition holds.
        if matches!(iv, Intervention::Whisper { .. }) && !self.whisper_enabled {
            return held_plan(iv, "held: whisper disabled (SIMARD_OVERSEER_WHISPER)");
        }

        // Goal-board health opt-out: when disabled, hold the self-heal /
        // escalation (no action taken) even though a blocked goal was observed.
        if matches!(
            iv,
            Intervention::UnblockGoal { .. } | Intervention::EscalateBlockedGoal { .. }
        ) && !self.goal_health_enabled
        {
            return held_plan(
                iv,
                "held: goal-board health disabled (SIMARD_OVERSEER_GOAL_HEALTH)",
            );
        }

        // Gap-scan opt-out: when disabled, hold the coverage action even though
        // gaps were observed. The closing edge is now a `LaunchRecipe` tagged with
        // `WORKSTREAM_COVERAGE_GROUP` (issue #4128, D3b), so the opt-out matches
        // that marker; the legacy notify-only `FlagWorkstreamGaps` is still held
        // too (defence in depth for any direct construction).
        if !self.gap_scan_enabled
            && (matches!(iv, Intervention::FlagWorkstreamGaps { .. })
                || matches!(
                    iv,
                    Intervention::LaunchRecipe { brief }
                        if brief.sequence_group.as_deref() == Some(WORKSTREAM_COVERAGE_GROUP)
                ))
        {
            return held_plan(iv, "held: gap-scan disabled (SIMARD_OVERSEER_GAP_SCAN)");
        }

        // In-flight investigation dedup (live defect 2026-07-15): never launch a
        // SECOND recipe-runner investigation for a signature that already has one
        // running. The observed failure was two recipe-runner processes (PIDs
        // 1074394 and 1095553) investigating the identical
        // `overseer-obs:goal:blocked:…` signature at once, because a recurring
        // signature re-observed each cycle re-launched a fresh recipe while the
        // prior one was still in flight (`sequence_group` is `None` for these, so
        // the conflict sequencer never dedups them). Checked BEFORE the cost gate
        // so a held duplicate never consumes a per-cycle launch slot; keyed per
        // signature so a DIFFERENT investigation is unaffected.
        if let Intervention::LaunchRecipe { brief } = iv
            && self
                .inflight_investigations
                .contains_key(&recipe_dedup_key(brief))
        {
            return held_plan(
                iv,
                "held: an investigation for this signature is already in flight",
            );
        }

        // Gap-scan coverage backoff (Problem 1 / issue #4186; meta bugs #4255,
        // #4126): the in-flight guard above only holds a duplicate WHILE the
        // covering workstream runs. Once it COMPLETES, an unchanged, still
        // -recurring gap would re-launch an equivalent `WorkstreamCoverage`
        // recipe EVERY tick — the churn that spawned duplicate backlog issues
        // (#4186/#4190/#4191/#4198/#4201/#4203/#4206). The exponential backoff
        // suppresses that relaunch (keyed by the same `recipe_dedup_key`) within
        // a growing, BOUNDED window even after completion, yet always re-admits
        // once the window elapses (never permanent silence). Scoped to the
        // coverage sequence group so ordinary process-health / cross-cutting
        // investigations are unaffected. Checked BEFORE the cost gate so a held
        // duplicate never consumes a per-cycle launch slot; only PEEKED here (the
        // window is armed in `act` on a successful launch).
        if let Intervention::LaunchRecipe { brief } = iv
            && brief.sequence_group.as_deref() == Some(WORKSTREAM_COVERAGE_GROUP)
            && self
                .coverage_backoff
                .peek(&recipe_dedup_key(brief), (self.clock)())
                == BackoffDecision::Suppress
        {
            return held_plan(
                iv,
                "held: an equivalent coverage was launched recently (backoff window)",
            );
        }

        // Autonomy: HIGH-RISK requires opt-in, else it is escalated (held).
        if let Err(e) = self.autonomy.admit(iv) {
            return held_plan(iv, e.to_string());
        }

        // Budget + concurrency: only for cost-bearing launches/audits.
        if is_cost_bearing(iv) {
            if *launches >= self.max_launches_per_cycle {
                return held_plan(iv, "held: per-cycle launch cap reached");
            }
            if let Some(spent) = observed.spent_today_usd
                && let Err(e) = self.budget.admit(spent)
            {
                return held_plan(iv, e.to_string());
            }
            // Conflict-avoidance: serialise sweeps sharing a sequence group.
            if let Intervention::LaunchRecipe { brief } = iv
                && let Err(e) = self.sequencer.admit(brief.sequence_group.as_deref())
            {
                return held_plan(iv, e.to_string());
            }
            *launches += 1;
        }

        admitted_plan(iv)
    }

    /// Execute one admitted intervention by dispatching to its reused capability.
    /// This is the M2+ Act seam. Anti-recursion is applied per-subject before any
    /// PR/deploy action.
    pub fn act(&mut self, iv: &Intervention) -> Result<ActOutcome, OverseerError> {
        match iv {
            Intervention::LaunchRecipe { brief } => {
                // Anti-recursion fail-closed for the backlog-coverage closing edge
                // (issue #4128, D3b): a workstream launched to COVER an uncovered
                // gap is the Overseer acting autonomously on Simard's own backlog,
                // so it must NEVER fire without a DISTINCT steward identity — the
                // same guard the legacy `act_flag_workstream_gaps` path enforced.
                // Scoped to the coverage sequence group so ordinary investigation
                // launches (process-health, cross-cutting) are unaffected.
                if brief.sequence_group.as_deref() == Some(WORKSTREAM_COVERAGE_GROUP)
                    && !self.recursion.is_configured()
                {
                    return Err(OverseerError::Recursion {
                        subject: format!(
                            "launch workstream-coverage recipe (unconfigured steward identity): {}",
                            recipe_dedup_key(brief)
                        ),
                    });
                }
                let handle = self.caps.recipes.launch(brief)?;
                // Register the launch in the in-flight dedup set so a re-observed
                // recurring signature is HELD in `gate` until this investigation
                // completes (reconciled at the top of `run_cycle`).
                self.inflight_investigations
                    .insert(recipe_dedup_key(brief), handle.clone());
                // Arm the coverage backoff window (Problem 1 / issue #4186) so an
                // equivalent coverage is not relaunched every tick AFTER this
                // workstream completes and frees its in-flight slot. Only the
                // coverage closing edge is deduped this way; ordinary
                // investigations rely on the in-flight guard alone.
                if brief.sequence_group.as_deref() == Some(WORKSTREAM_COVERAGE_GROUP) {
                    self.coverage_backoff
                        .commit(&recipe_dedup_key(brief), (self.clock)());
                }
                Ok(ActOutcome::Launched(handle))
            }
            Intervention::VerifyAndMergePr { repo, pr } => {
                let report = self.caps.prs.verify(repo, *pr)?;
                if !report.ready {
                    return Ok(ActOutcome::Escalated);
                }
                // `verify()` is only the objective pre-filter. The authoritative
                // agentic review runs inside `merge()` (step 3); when it refuses
                // — or the LLM provider is unavailable and the judge fails closed
                // — `merge()` returns `NotMergeReady`, which is an ESCALATION, not
                // an error (never a blind merge).
                match self.caps.prs.merge(repo, *pr) {
                    Ok(()) => Ok(ActOutcome::Merged),
                    Err(OverseerError::NotMergeReady { .. }) => Ok(ActOutcome::Escalated),
                    Err(e) => Err(e),
                }
            }
            Intervention::ResolveConflict { repo, pr } => {
                self.caps.prs.resolve_conflict(repo, *pr)?;
                Ok(ActOutcome::ConflictResolved)
            }
            Intervention::Deploy { commit } => {
                Ok(ActOutcome::Deployed(self.caps.deployer.deploy(commit)?))
            }
            Intervention::FileIssue { run } => {
                Ok(ActOutcome::IssueFiled(self.caps.issues.file(run)?))
            }
            Intervention::TransferGoal { goal } => {
                self.caps.meetings.transfer_goal(goal)?;
                Ok(ActOutcome::GoalTransferred)
            }
            Intervention::Report => Ok(ActOutcome::Reported),
            Intervention::RunAudit { scope } => {
                self.caps.auditor.run_audit(scope)?;
                Ok(ActOutcome::Audited)
            }
            Intervention::Escalate { .. } => Ok(ActOutcome::Escalated),
            Intervention::Whisper { note, urgency } => self.act_whisper(note, *urgency),
            Intervention::UnblockGoal { goal_id, reason } => self.act_unblock_goal(goal_id, reason),
            Intervention::EscalateBlockedGoal {
                goal_id,
                reason,
                why,
                problem,
                next_step,
                link,
            } => self.act_escalate_blocked_goal(
                goal_id,
                reason,
                why,
                problem,
                next_step,
                link.as_deref(),
            ),
            Intervention::FlagWorkstreamGaps { gaps } => self.act_flag_workstream_gaps(gaps),
            // Agentic merge-queue hygiene (#4097). Anti-recursion fail-closed:
            // both act on an open PR autonomously, so — like the coverage launch —
            // they require a DISTINCT configured steward identity before firing.
            // The argv is built by the intervention.rs builders that can never
            // carry `--admin`/`--no-verify`.
            Intervention::FlagStalePr { repo, pr, note } => {
                if !self.recursion.is_configured() {
                    return Err(OverseerError::Recursion {
                        subject: format!(
                            "flag stale PR {repo}#{pr} (unconfigured steward identity)"
                        ),
                    });
                }
                self.caps.prs.flag_stale_pr(repo, *pr, note)?;
                Ok(ActOutcome::StalePrFlagged {
                    repo: repo.clone(),
                    pr: *pr,
                })
            }
            Intervention::CloseDuplicatePr {
                repo,
                pr,
                duplicate_of,
            } => {
                if !self.recursion.is_configured() {
                    return Err(OverseerError::Recursion {
                        subject: format!(
                            "close duplicate PR {repo}#{pr} (unconfigured steward identity)"
                        ),
                    });
                }
                self.caps.prs.close_duplicate_pr(repo, *pr, *duplicate_of)?;
                Ok(ActOutcome::DuplicatePrClosed {
                    repo: repo.clone(),
                    pr: *pr,
                    duplicate_of: *duplicate_of,
                })
            }
        }
    }

    /// Deliver an advisory whisper: fail CLOSED without a distinct steward
    /// identity (anti-recursion — the Overseer must never whisper without a
    /// configured DISTINCT identity, and never about its own whisper), dedup +
    /// rate-limit via the [`WhisperGate`], then deliver through the sink. The
    /// dedup slot is consumed only on a SUCCESSFUL delivery, so a failed or
    /// panicking sink never silently swallows a future whisper.
    fn act_whisper(
        &mut self,
        note: &str,
        urgency: WhisperUrgency,
    ) -> Result<ActOutcome, OverseerError> {
        // Fail closed: a whisper requires the Overseer's DISTINCT steward
        // identity (RecursionGuard). An unconfigured guard REFUSES the whisper.
        if !self.recursion.is_configured() {
            return Err(OverseerError::Recursion {
                subject: format!("whisper (unconfigured steward identity): {note}"),
            });
        }

        let signature = note_signature(note);
        let now = now_secs();
        match self.whisper_gate.peek(&signature, now) {
            WhisperDecision::Deliver => {
                let sink = self
                    .whisper_sink
                    .as_ref()
                    .ok_or(OverseerError::Capability {
                        what: "whisper.sink",
                        detail: "no whisper sink configured".to_string(),
                    })?;
                let rec = WhisperRecord {
                    note: note.to_string(),
                    urgency,
                    // The originating problem is not carried on the intervention;
                    // the note itself encodes the goal + trigger. Kept generic
                    // here — the delivered note carries the specifics.
                    problem: ProblemKind::LoopDetected,
                    goal_id: None,
                    author: self.recursion.author_login.clone(),
                    signature: signature.clone(),
                };
                let path = sink.deliver(&rec)?;
                // Consume the dedup slot only after a successful delivery.
                self.whisper_gate.commit(&signature, now);
                tracing::info!(
                    target: "overseer::whisper",
                    trigger = "overseer-decide",
                    note = %note,
                    urgency = urgency.label(),
                    delivered = true,
                    signature = %signature,
                    path = %path.display(),
                    "overseer delivered an advisory whisper into Simard's OODA inbox"
                );
                Ok(ActOutcome::Whispered { path, signature })
            }
            WhisperDecision::SuppressDuplicate => {
                tracing::debug!(
                    target: "overseer::whisper",
                    note = %note,
                    urgency = urgency.label(),
                    delivered = false,
                    signature = %signature,
                    reason = "duplicate",
                    "overseer suppressed a duplicate whisper within the dedup window"
                );
                Ok(ActOutcome::WhisperSuppressed {
                    reason: "duplicate",
                })
            }
            WhisperDecision::SuppressCapReached => {
                tracing::debug!(
                    target: "overseer::whisper",
                    note = %note,
                    urgency = urgency.label(),
                    delivered = false,
                    signature = %signature,
                    reason = "cap_reached",
                    "overseer suppressed a whisper: per-hour cap reached"
                );
                Ok(ActOutcome::WhisperSuppressed {
                    reason: "cap_reached",
                })
            }
        }
    }

    /// SELF-HEAL a false-parked standing/perpetual goal: auto-unblock +
    /// reactivate it (the exact `simard goal unblock` operation), deduped so it
    /// never re-fires in a tight loop, then OPTIONALLY whisper Simard to carve a
    /// bounded shippable sub-goal. Fails CLOSED without a DISTINCT steward
    /// identity (anti-recursion — the Overseer must never self-heal a goal
    /// without a configured distinct identity, which also prevents a self-heal
    /// loop). The whisper is best-effort: a whisper failure never fails the
    /// unblock.
    fn act_unblock_goal(
        &mut self,
        goal_id: &str,
        reason: &str,
    ) -> Result<ActOutcome, OverseerError> {
        if !self.recursion.is_configured() {
            return Err(OverseerError::Recursion {
                subject: format!("unblock goal (unconfigured steward identity): {goal_id}"),
            });
        }
        let signature = format!("unblock:{goal_id}");
        let now = now_secs();
        match self.blocked_goal_gate.peek(&signature, now) {
            WhisperDecision::Deliver => {
                self.caps.goals.unblock(goal_id)?;
                // Consume the dedup slot only after a successful unblock.
                self.blocked_goal_gate.commit(&signature, now);
                tracing::info!(
                    target: "overseer::goal_health",
                    goal_id,
                    reason,
                    action = "unblock",
                    "overseer self-healed a false-parked perpetual goal: auto-unblocked + reactivated"
                );
                // Optional advisory whisper: steer Simard to carve a bounded,
                // shippable sub-goal instead of re-attempting the whole standing
                // goal at once. Best-effort and reuses the whisper gate/identity.
                self.try_whisper_carve_subgoal(goal_id);
                Ok(ActOutcome::GoalUnblocked {
                    goal_id: goal_id.to_string(),
                })
            }
            other => Ok(Self::goal_health_suppressed("unblock", goal_id, other)),
        }
    }

    /// ESCALATE a genuinely-blocked "needs human review" goal — as a THIN AGENTIC
    /// TRIGGER (issue #4276, guideline G3). Mirrors the StepFailure →
    /// `self_diagnose.md` seam: rather than a terminal notify-and-count, the
    /// Overseer hands the escalation to the agentic triage recipe
    /// (`escalation_triage.md`) through the SAME `RecipeLauncher` seam, and the
    /// recipe owns the escalate-vs-course-correct DECISION (rewrite an unmeasurable
    /// done-gate, complete a goal already delivered by a merged PR, or ask the
    /// operator ONE specific question — emitting a plain-English Signal per step).
    ///
    /// The Rust part stays thin: it (1) fails CLOSED without a DISTINCT steward
    /// identity (anti-recursion), (2) dedups via the SAME in-flight investigation
    /// set the recipe-launch path uses (a re-escalation while triage is still in
    /// flight launches nothing), (3) launches the triage recipe and consumes ONLY
    /// the `WorkstreamHandle` (never brittle-parses recipe stdout), and (4) sends
    /// the operator a PLAIN-ENGLISH notification (problem + next step + tracking
    /// link — never the raw marker) on both channels, fail-open on the notifier.
    #[allow(clippy::too_many_arguments)]
    fn act_escalate_blocked_goal(
        &mut self,
        goal_id: &str,
        reason: &str,
        why: &str,
        problem: &str,
        next_step: &str,
        link: Option<&str>,
    ) -> Result<ActOutcome, OverseerError> {
        if !self.recursion.is_configured() {
            return Err(OverseerError::Recursion {
                subject: format!(
                    "escalate blocked goal (unconfigured steward identity): {goal_id}"
                ),
            });
        }

        // Build the agentic triage brief. The task description carries the
        // structured context (goal id + plain-English problem + next step + the
        // internal WHY) and points the agent at the recipe asset — exactly like
        // the self_diagnose seam. `recipe_dedup_key` derives a stable per-goal key
        // from this text, so a re-escalation of the same goal reuses the same slot.
        let brief = RecipeBrief {
            task_description: format!(
                "Triage and course-correct a blocked goal before escalating to a human. \
                 Follow prompt_assets/simard/overseer/escalation_triage.md. \
                 Goal id: {goal_id}. Problem (plain English): {problem} \
                 Recommended next step: {next_step} \
                 Internal diagnostic WHY (translate to plain English, do not surface \
                 raw): {why}. Reason marker (translate, do not surface raw): {reason}. \
                 Decide root-cause + course-correction (rewrite an unmeasurable \
                 done-gate to be machine-checkable, complete a goal already delivered \
                 by a merged PR, or ask the operator ONE specific plain-English \
                 question), and send a jargon-free Signal message per step.",
            ),
            target_repo: "rysweet/Simard".to_string(),
            sequence_group: None,
        };
        let key = recipe_dedup_key(&brief);

        // In-flight dedup: never launch a SECOND triage for a goal whose triage is
        // still running (the same mechanism the recipe-launch path uses). A
        // flapping goal cannot spawn triage every cycle.
        if self.inflight_investigations.contains_key(&key) {
            tracing::debug!(
                target: "overseer::goal_health",
                goal_id,
                action = "escalate",
                reason = "duplicate",
                "overseer suppressed a blocked-goal escalation (triage already in flight)"
            );
            return Ok(ActOutcome::GoalHealthSuppressed {
                reason: "duplicate",
            });
        }

        // Hand off to the agentic triage recipe and hold ONLY the handle.
        let handle = self.caps.recipes.launch(&brief)?;
        self.inflight_investigations.insert(key, handle.clone());

        // Notify the operator in PLAIN ENGLISH (fail-open: only when a notifier is
        // configured — a missing notifier never blocks the agentic triage launch).
        if let Some(notifier) = self.notifier.as_ref() {
            let notification =
                OperatorNotification::goal_blocked_triaged(goal_id, problem, next_step, link);
            let report = notifier.notify(&notification);
            tracing::info!(
                target: "overseer::goal_health",
                goal_id,
                action = "escalate",
                triage_workstream = %handle.id,
                dispatched = report.dispatched(),
                all_sent = report.all_sent(),
                "overseer launched agentic escalation-triage and notified the operator in plain English"
            );
        } else {
            tracing::info!(
                target: "overseer::goal_health",
                goal_id,
                action = "escalate",
                triage_workstream = %handle.id,
                "overseer launched agentic escalation-triage (no operator notifier configured)"
            );
        }

        Ok(ActOutcome::Launched(handle))
    }

    /// Map a non-`Deliver` dedup-gate decision for a goal-board health action
    /// (self-heal or escalate) to its suppressed [`ActOutcome`], logging why.
    /// The caller handles [`WhisperDecision::Deliver`] inline and only routes
    /// the two suppression variants here, so this stays a single shared tail for
    /// both act paths (deduped within the window, or the per-hour cap reached).
    fn goal_health_suppressed(
        action: &'static str,
        goal_id: &str,
        decision: WhisperDecision,
    ) -> ActOutcome {
        // `Deliver` is handled inline by the caller and never reaches here.
        let reason = match decision {
            WhisperDecision::SuppressCapReached => "cap_reached",
            _ => "duplicate",
        };
        tracing::debug!(
            target: "overseer::goal_health",
            goal_id,
            action,
            reason,
            "overseer suppressed a goal-board health action (dedup window / per-hour cap)"
        );
        ActOutcome::GoalHealthSuppressed { reason }
    }

    /// FLAG the backlog-coverage gaps the recurring gap-scan found by notifying
    /// the operator on both channels. Routine observations never create GitHub
    /// issues or stewardship backlog items.
    fn act_flag_workstream_gaps(&mut self, gaps: &[GapItem]) -> Result<ActOutcome, OverseerError> {
        if !self.recursion.is_configured() {
            return Err(OverseerError::Recursion {
                subject: format!(
                    "flag workstream gaps (unconfigured steward identity): {} gap(s)",
                    gaps.len()
                ),
            });
        }

        let now = now_secs();
        // Peek every gap first (no commit) so the consolidated notification names
        // exactly the FRESH gaps; a recurring gap within the dedup window is
        // counted as suppressed and not re-notified.
        let mut fresh: Vec<GapItem> = Vec::new();
        let mut suppressed = 0usize;
        for g in gaps {
            let sig = format!("workstream-gap:{}", g.signature);
            match self.gap_gate.peek(&sig, now) {
                WhisperDecision::Deliver => fresh.push(g.clone()),
                WhisperDecision::SuppressDuplicate | WhisperDecision::SuppressCapReached => {
                    suppressed += 1
                }
            }
        }

        if fresh.is_empty() {
            tracing::debug!(
                target: "overseer::gap_scan",
                flagged = 0usize,
                suppressed,
                "overseer gap-scan: every observed gap is within the dedup window (suppressed)"
            );
            return Ok(ActOutcome::WorkstreamGapsFlagged {
                flagged: 0,
                suppressed,
            });
        }

        // ONE consolidated operator notification (email + Signal) naming every
        // fresh gap — the SAME mandatory notifier the merge / goal-health paths use.
        let notifier = self.notifier.as_ref().ok_or(OverseerError::Capability {
            what: "notify.operator",
            detail: "no operator notifier configured".to_string(),
        })?;
        let notification = OperatorNotification::workstream_gap(fresh.len(), &fresh);
        let report = notifier.notify(&notification);
        for g in &fresh {
            let sig = format!("workstream-gap:{}", g.signature);
            self.gap_gate.commit(&sig, now);
        }

        tracing::info!(
            target: "overseer::gap_scan",
            flagged = fresh.len(),
            suppressed,
            dispatched = report.dispatched(),
            all_sent = report.all_sent(),
            "overseer recorded uncovered backlog work and notified the operator"
        );
        Ok(ActOutcome::WorkstreamGapsFlagged {
            flagged: fresh.len(),
            suppressed,
        })
    }

    /// Best-effort advisory whisper steering Simard to carve ONE bounded,
    /// shippable sub-goal from a just-unblocked standing goal. Silently skipped
    /// when the whisperer is disabled/unwired; a delivery error or suppression is
    /// ignored (the self-heal already succeeded).
    fn try_whisper_carve_subgoal(&mut self, goal_id: &str) {
        if !self.whisper_enabled || self.whisper_sink.is_none() {
            return;
        }
        let note = format!(
            "Overseer steering note for goal {goal_id}: this standing goal was auto-unblocked \
             after a no-progress false-park. Carve ONE bounded, shippable sub-goal from it and \
             ship that next, rather than re-attempting the whole standing goal at once."
        );
        let _ = self.act_whisper(&note, WhisperUrgency::Normal);
    }

    /// Recall prior occurrences of a problem's root cause from cognitive memory
    /// (amplihack-memory-lib, G2), keyed on the problem's dedup signature.
    /// Read-only and best-effort: no memory wired ⇒ empty; a recall error ⇒
    /// empty + a `tracing` log (never a silent failure, never a panic). The
    /// caller folds the result into the structured analysis so recall raises
    /// `recurrence` for a repeatedly-seen cause.
    ///
    /// Issue #4128 (D2b): occurrence memory now stores ONE count-in-content fact
    /// per `(signature, cause_label)`. Each such fact is EXPANDED back into its
    /// `count` `PriorOccurrence`s (bounded by [`OCCURRENCE_RECALL_LIMIT`]) so the
    /// downstream recurrence tally — `analyze` counts matching occurrences — still
    /// ratchets to the escalation threshold, without the unbounded row growth that
    /// let the Overseer's own re-observation self-amplify.
    fn recall_occurrences(&self, dedup_key: &str) -> Vec<PriorOccurrence> {
        let Some(mem) = self.memory.as_ref() else {
            return Vec::new();
        };
        let concept = occurrence_concept(dedup_key);
        match mem.search_facts(&concept, OCCURRENCE_RECALL_LIMIT, 0.0) {
            Ok(facts) => {
                let mut out: Vec<PriorOccurrence> = Vec::new();
                for f in facts {
                    let Some(stored) = serde_json::from_str::<StoredOccurrence>(&f.content)
                        .ok()
                        .filter(|o| o.signature == dedup_key)
                    else {
                        continue;
                    };
                    // Expand the count-in-content tally into that many recalled
                    // occurrences, bounded overall so a large saturating count can
                    // never blow up the recalled set.
                    let remaining = (OCCURRENCE_RECALL_LIMIT as usize).saturating_sub(out.len());
                    if remaining == 0 {
                        break;
                    }
                    let repeats = (stored.count.max(1) as usize).min(remaining);
                    let prior = stored.into_prior();
                    for _ in 0..repeats {
                        out.push(prior.clone());
                    }
                }
                out
            }
            Err(e) => {
                tracing::debug!(
                    target: "overseer::root_cause",
                    dedup_key,
                    error = %e,
                    "root-cause occurrence recall failed — degraded to telemetry-only WHY"
                );
                Vec::new()
            }
        }
    }

    /// Read the current recorded occurrence `count` for `(signature, cause_label)`
    /// from cognitive memory (issue #4128, D2b), so a fresh occurrence upserts an
    /// incremented count rather than appending a new row. Best-effort: no memory,
    /// a recall error, or no prior fact ⇒ 0. Takes the MAX across any matching
    /// facts so a legacy multi-row tail (pre-#4128 appends) still ratchets forward
    /// instead of resetting.
    fn recorded_occurrence_count(&self, signature: &str, cause_label: &str) -> u32 {
        let Some(mem) = self.memory.as_ref() else {
            return 0;
        };
        let concept = occurrence_concept(signature);
        match mem.search_facts(&concept, OCCURRENCE_RECALL_LIMIT, 0.0) {
            Ok(facts) => facts
                .iter()
                .filter_map(|f| serde_json::from_str::<StoredOccurrence>(&f.content).ok())
                .filter(|o| o.signature == signature && o.cause_label == cause_label)
                .map(|o| o.count.max(1))
                .max()
                .unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Record this occurrence's root-cause signature + primary cause + action +
    /// outcome into cognitive memory (amplihack-memory-lib, G2) so a later cycle
    /// recalls it and raises `recurrence`. Best-effort: no memory wired ⇒ no-op;
    /// a store error is `tracing`-logged and swallowed (never fatal to the tick,
    /// never silent).
    ///
    /// Issue #4128 (D2b): this UPSERTS a SINGLE count-in-content fact per
    /// `(signature, cause_label)` — it reads the prior count, stores an
    /// incremented count under a stable caller-dedup key (superseding the old
    /// live fact), and prunes the superseded tail — rather than appending a fresh
    /// fact every cycle. Appending grew occurrence rows without bound, which is
    /// exactly what let the Overseer's re-observation of its own signature
    /// self-amplify into the "recurring signature seen 2×" incident.
    fn record_occurrence(&self, entry: &ProblemEntry, outcome: &ActOutcome) {
        let Some(mem) = self.memory.as_ref() else {
            return;
        };
        let Some(primary) = entry.why.primary() else {
            return;
        };
        // Increment the prior count (saturating) so the single stored fact is an
        // honest, bounded running tally of this cause's occurrences.
        let count = self
            .recorded_occurrence_count(&entry.key, &primary.label)
            .saturating_add(1);
        let record = StoredOccurrence {
            signature: entry.key.clone(),
            cause_label: primary.label.clone(),
            action: entry.action.clone(),
            outcome: describe_outcome(outcome),
            count,
        };
        let content = match serde_json::to_string(&record) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(
                    target: "overseer::root_cause",
                    error = %e,
                    "root-cause occurrence serialize failed — occurrence not recorded"
                );
                return;
            }
        };
        let concept = occurrence_concept(&entry.key);
        let caller_key = StoredOccurrence::caller_key(&entry.key, &primary.label);
        let tags = vec![
            entry.key.clone(),
            primary.label.clone(),
            "overseer-root-cause".to_string(),
        ];
        // CallerKey upsert: the incremented-count content supersedes the prior
        // live fact so exactly one live occurrence fact survives per key.
        if let Err(e) = mem.store_fact_with_caller_key(
            &caller_key,
            &concept,
            &content,
            0.9,
            &tags,
            "overseer:root-cause",
        ) {
            tracing::debug!(
                target: "overseer::root_cause",
                signature = %entry.key,
                cause = %primary.label,
                error = %e,
                "root-cause occurrence store failed (best-effort) — recurrence tracking degraded"
            );
            return;
        }
        // Reclaim the superseded revision archived by the upsert so occurrence
        // recall reads exactly one live count-in-content fact (best-effort — a
        // failed prune only leaves a reclaimable archived tail, never a wrong
        // count, since recall takes the MAX count across matching facts).
        if let Err(e) = mem.prune_superseded() {
            tracing::debug!(
                target: "overseer::root_cause",
                signature = %entry.key,
                cause = %primary.label,
                error = %e,
                "root-cause occurrence prune failed (best-effort) — superseded tail left for a later pass"
            );
        }
    }
}

/// Current wall-clock time in whole seconds since the Unix epoch, for the
/// whisper dedup/rate-limit clock. Monotonic enough for windowing; tests drive
/// the [`WhisperGate`] directly with a virtual clock.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True for interventions that spend LLM budget / spawn work.
fn is_cost_bearing(iv: &Intervention) -> bool {
    matches!(
        iv,
        Intervention::LaunchRecipe { .. } | Intervention::RunAudit { .. }
    )
}

/// Stable dedup key for one recipe-runner investigation launch (live defect
/// 2026-07-15). Two launches for the SAME goal / recurring signature must map to
/// the same key so the in-flight guard holds the duplicate; two DIFFERENT
/// investigations must map to distinct keys so neither is starved.
///
/// A recurring-signature launch embeds its `overseer-obs:…` signature token in
/// the task description; keying on that token (when present) makes the guard
/// robust to incidental prose drift around it. Absent the token, the trimmed
/// task description is the key — deterministic for a given problem summary, which
/// is itself derived deterministically from the signal.
fn recipe_dedup_key(brief: &RecipeBrief) -> String {
    let desc = brief.task_description.trim();
    if let Some(start) = desc.find("overseer-obs:") {
        let tail = &desc[start..];
        let end = tail
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(tail.len());
        return tail[..end].to_string();
    }
    desc.to_string()
}

/// The write-back prefix the Overseer stamps on its own observation episodes
/// (issue #2628). A recalled episode carrying this prefix is the Overseer reading
/// its OWN prior observation back out of cognitive memory — folding such a
/// recall-derived problem into a fresh write-back is the self-referential loop
/// that self-amplifies the recurrence counter (issue #4128, D1). The write
/// boundary excludes any problem whose `dedup_key` already carries this prefix.
const OVERSEER_OBS_PREFIX: &str = "overseer-obs:";

/// True for a recall-derived problem that is the Overseer's OWN prior observation
/// (its `dedup_key` already carries the [`OVERSEER_OBS_PREFIX`]). These are
/// filtered at the write boundary (issue #4128, D1) so the Overseer never records
/// an observation OF its own observation.
fn is_recall_derived_self_observation(problem: &Problem) -> bool {
    problem.dedup_key.starts_with(OVERSEER_OBS_PREFIX)
}

/// Stable, deterministic signature for the Overseer's own observation write-back
/// (issue #2628): the sorted, deduped FIRST-ORDER problem `dedup_key`s joined.
/// Two identical observations produce the same signature (so the write-back gate
/// de-dups them); two different observations produce distinct signatures (so both
/// are recorded).
///
/// Recall-derived `overseer-obs:*` problems are excluded (issue #4128, D1): the
/// signature keys ONLY genuinely first-order observations, so the write-back can
/// never nest an `overseer-obs:` prefix on itself and re-observe its own output.
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems
        .iter()
        .filter(|p| !is_recall_derived_self_observation(p))
        .map(|p| p.dedup_key.as_str())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    format!("{OVERSEER_OBS_PREFIX}{}", keys.join("|"))
}

/// The human-readable one-line body of the Overseer's observation write-back.
/// Only FIRST-ORDER problems are described (recall-derived `overseer-obs:*`
/// self-observations are excluded, issue #4128 D1). Every problem summary is
/// `sanitize_recalled`-cleaned (defence in depth: the summaries may themselves
/// already carry recalled text) before it enters the episode content that is
/// persisted into the multi-writer memory graph.
fn observation_content(problems: &[Problem]) -> String {
    let parts: Vec<String> = problems
        .iter()
        .filter(|p| !is_recall_derived_self_observation(p))
        .map(|p| sanitize_recalled(&p.summary))
        .collect();
    sanitize_recalled(&format!(
        "overseer observed {} problem(s): {}",
        parts.len(),
        parts.join("; ")
    ))
}

/// Build a held (not-admitted) `PlannedIntervention` with a gate note and the
/// from-intervention remediation default (`run_cycle` refines it with the WHY).
fn held_plan(iv: &Intervention, note: impl Into<String>) -> PlannedIntervention {
    PlannedIntervention {
        intervention: iv.clone(),
        admitted: false,
        note: note.into(),
        remediation: remediation_for(iv, &RootCause::unknown()),
    }
}

/// Build an admitted `PlannedIntervention` with the from-intervention remediation
/// default (`run_cycle` refines it with the WHY).
fn admitted_plan(iv: &Intervention) -> PlannedIntervention {
    PlannedIntervention {
        intervention: iv.clone(),
        admitted: true,
        note: "admitted".to_string(),
        remediation: remediation_for(iv, &RootCause::unknown()),
    }
}

/// Classify how an intervention relates to a problem's ROOT CAUSE (issue #2635).
///
/// Root-cause-addressing actions (self-heal a false park, launch a fix, escalate
/// a recurring systemic defect for a fix, deliver/merge, steer with a whisper) are
/// `RootCause`. A plain `Report` of a deliberate/intentional block is
/// `Acknowledged` (nothing to fix; it never cries wolf). Handing resource/process
/// pressure to the operator via `Escalate` only mitigates the SYMPTOM — the
/// underlying cause (spend spike, runaway retries, mis-set budget) stays live, so
/// it is a `SymptomMitigation` whose unaddressed cause is surfaced from the WHY.
fn remediation_for(iv: &Intervention, why: &RootCause) -> Remediation {
    match iv {
        Intervention::Escalate { reason } => Remediation::symptom(format!(
            "root cause unaddressed: {} — escalated to the operator ({reason}); the underlying \
             cause is not fixed by the hand-off",
            why.primary_rationale
        )),
        Intervention::Report => Remediation::acknowledged(),
        _ => Remediation::root_cause(),
    }
}

/// A ceiling on how many stored occurrence facts one recall reads back — bounds
/// the read while comfortably covering the recurrence counts that drive the
/// escalate-the-root-cause decision.
const OCCURRENCE_RECALL_LIMIT: u32 = 256;

/// The stable cognitive-memory `concept` token for a problem's occurrence facts:
/// a single alphanumeric token derived deterministically from the dedup key, so
/// recall matches only this problem's occurrences (the content-side `signature`
/// filter guards against any incidental keyword collision).
///
/// Uses SHA-256 (not `DefaultHasher`) so the token is stable across Rust/std
/// versions and platforms — a stored occurrence stays recallable after a
/// toolchain upgrade, preserving recurrence tracking.
fn occurrence_concept(dedup_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(dedup_key.as_bytes());
    let mut token = String::with_capacity(27);
    token.push_str("overseerocc");
    for b in &digest[..8] {
        token.push_str(&format!("{b:02x}"));
    }
    token
}

/// A best-effort human summary of an act outcome, recorded on a stored
/// occurrence so recall can show what happened last time.
fn describe_outcome(outcome: &ActOutcome) -> String {
    match outcome {
        ActOutcome::GoalUnblocked { .. } => "goal auto-unblocked (self-heal)".to_string(),
        ActOutcome::GoalEscalated { .. } => "escalated to operator".to_string(),
        ActOutcome::IssueFiled(_) => "root-cause issue filed".to_string(),
        ActOutcome::Escalated => "escalated to operator (symptom mitigation)".to_string(),
        ActOutcome::Launched(_) => "fix workstream launched".to_string(),
        ActOutcome::Merged => "PR merged".to_string(),
        ActOutcome::Deployed(_) => "deployed".to_string(),
        ActOutcome::ConflictResolved => "conflict resolved".to_string(),
        ActOutcome::GoalTransferred => "goal transferred to Simard".to_string(),
        ActOutcome::Whispered { .. } => "advisory whisper delivered".to_string(),
        ActOutcome::Audited => "quality audit run".to_string(),
        other => format!("{other:?}"),
    }
}

/// The durable form of a [`PriorOccurrence`], stored in cognitive memory with the
/// problem `signature` so recall can filter to this problem's occurrences.
///
/// Issue #4128 (D2b): occurrence memory is an UPSERT of ONE fact per
/// `(signature, cause_label)` carrying a bounded `count`, rather than a fresh
/// appended fact every cycle. The count-in-content is the honest recurrence
/// tally; recall expands it back into that many `PriorOccurrence`s so the
/// recurrence-driven root-cause escalation still ratchets, without the unbounded
/// row growth that let the Overseer's own re-observation self-amplify.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredOccurrence {
    signature: String,
    cause_label: String,
    action: String,
    outcome: String,
    /// How many times this `(signature, cause_label)` has been recorded. Defaults
    /// to 1 for legacy facts written before the count field existed (each such
    /// pre-#4128 fact represented exactly one occurrence).
    #[serde(default = "one_u32")]
    count: u32,
}

/// serde default for [`StoredOccurrence::count`]: a legacy (pre-#4128) occurrence
/// fact carried no count and represented exactly one occurrence.
fn one_u32() -> u32 {
    1
}

impl StoredOccurrence {
    /// The stable caller-dedup key for this occurrence's upsert (issue #4128,
    /// D2b): one live fact per `(signature, cause_label)`. Keyed on both so
    /// distinct causes of the SAME signature never collapse onto one row.
    fn caller_key(signature: &str, cause_label: &str) -> String {
        format!("overseer-occ:{signature}::{cause_label}")
    }

    fn into_prior(self) -> PriorOccurrence {
        PriorOccurrence {
            cause_label: self.cause_label,
            action: self.action,
            outcome: self.outcome,
        }
    }
}

/// The conflict-sequencer / gate marker for the Overseer's backlog-coverage
/// closing edge (issue #4128, D3b). A [`Intervention::LaunchRecipe`] carrying
/// this `sequence_group` is the workstream launched to COVER an uncovered gap:
/// the gate holds it when the gap-scan is disabled (preserving the opt-out that
/// used to live on the notify-only `FlagWorkstreamGaps` path) and the sequencer
/// serialises concurrent coverage launches.
const WORKSTREAM_COVERAGE_GROUP: &str = "workstream-coverage";

/// Per-gap dedup key for a backlog-coverage problem (issue #4128, D3a):
/// `workstream-gap:<sorted, deduped gap signatures>`. Keying on the specific gap
/// set (never the bare `workstream-gap` constant) stops distinct gap sets from
/// collapsing onto one dedup key — which had starved the closing edge / in-flight
/// guard to a single gap. Each gap `signature` is a restricted slug built from
/// trusted identifiers only, so the key is always a safe, stable token.
fn workstream_gap_key(gaps: &[GapItem]) -> String {
    let mut sigs: Vec<&str> = gaps.iter().map(|g| g.signature.as_str()).collect();
    sigs.sort_unstable();
    sigs.dedup();
    format!("workstream-gap:{}", sigs.join("|"))
}

/// Orient: fold `Signal`s into ranked, deduplicated `Problem`s. Dedups against
/// Simard's in-flight work (so the Overseer never fights an engineer already on
/// the case) and against problems already collected this cycle.
pub fn orient(signals: &[Signal], in_flight: &[InFlightItem]) -> Vec<Problem> {
    let mut problems: Vec<Problem> = Vec::new();

    for s in signals {
        let (kind, priority, key, summary) = classify_signal(s);

        // Dedup against Simard's in-flight work.
        if in_flight.iter().any(|i| i.refs.iter().any(|r| r == &key)) {
            continue;
        }
        // Merge into an existing same-key problem rather than duplicating.
        if let Some(existing) = problems.iter_mut().find(|p| p.dedup_key == key) {
            existing.evidence.push(s.clone());
            // A recurring-signature co-signal (recalled from memory) RAISES the
            // matching problem's priority — this problem has happened before, so
            // it deserves more attention than the in-process counters alone gave
            // it. `min` picks the more-important level (Ord sorts Critical first).
            if matches!(s, Signal::RecurringSignature { .. }) {
                existing.priority = existing.priority.min(priority);
            }
            continue;
        }
        problems.push(Problem {
            kind,
            priority,
            dedup_key: key,
            summary,
            evidence: vec![s.clone()],
            // Orient stays PURE: the WHY is enriched later in `run_cycle`.
            why: None,
        });
    }

    problems.sort_by_key(|p| p.priority);
    problems
}

/// Map a single `Signal` to `(kind, priority, dedup_key, summary)`.
fn classify_signal(s: &Signal) -> (ProblemKind, Priority, String, String) {
    match s {
        Signal::DistillFailureRate { pct } => (
            ProblemKind::ProcessHealth,
            Priority::High,
            "process:distill_fail".to_string(),
            format!("distillation parse-failure rate {pct:.0}%"),
        ),
        Signal::RestartChurn { restarts } => (
            ProblemKind::ProcessHealth,
            Priority::High,
            "process:restart_churn".to_string(),
            format!("daemon restart churn ({restarts} restarts)"),
        ),
        Signal::LadderExhausted { count } => (
            ProblemKind::ProcessHealth,
            Priority::Normal,
            "process:ladder_exhausted".to_string(),
            format!("reasoner decide-ladder exhausted ({count})"),
        ),
        Signal::BudgetPressure {
            spent_usd,
            budget_usd,
        } => (
            ProblemKind::ResourcePressure,
            Priority::High,
            "resource:budget".to_string(),
            format!("LLM budget pressure (${spent_usd:.2} of ${budget_usd:.2})"),
        ),
        Signal::EngineerSpawnRate { live } => (
            ProblemKind::ResourcePressure,
            Priority::Normal,
            "resource:engineer_spawn".to_string(),
            format!("elevated engineer spawn ({live} live)"),
        ),
        Signal::MemoryGrowth { nodes_total } => (
            ProblemKind::ResourcePressure,
            Priority::Low,
            "resource:memory_growth".to_string(),
            format!("cognitive-memory growth ({nodes_total} nodes)"),
        ),
        Signal::GymSkipped => (
            ProblemKind::QualityRegression,
            Priority::Low,
            "quality:gym_skipped".to_string(),
            "gym self-eval skipped".to_string(),
        ),
        Signal::CiFailureCluster { repo, failing } => (
            ProblemKind::QualityRegression,
            Priority::High,
            format!("quality:ci:{repo}"),
            format!("CI-failure cluster in {repo} ({failing} failing)"),
        ),
        Signal::PrReadyToMerge { repo, pr } => (
            ProblemKind::DeliveryReady,
            Priority::Normal,
            format!("delivery:pr:{repo}#{pr}"),
            format!("PR {repo}#{pr} is green and merge-ready"),
        ),
        Signal::StaleGoal { goal_id } => (
            ProblemKind::GoalHygiene,
            Priority::Normal,
            format!("goal:stale:{goal_id}"),
            format!("goal {goal_id} re-litigated / stale-complete"),
        ),
        Signal::Anomaly { detail } => (
            ProblemKind::ProcessHealth,
            Priority::Normal,
            format!("anomaly:{detail}"),
            format!("telemetry anomaly: {detail}"),
        ),
        Signal::LoopDetected {
            goal_id,
            consecutive_no_action,
        } => (
            ProblemKind::LoopDetected,
            Priority::High,
            format!("loop:{goal_id}"),
            format!("goal {goal_id} looping — no progress for {consecutive_no_action} cycles"),
        ),
        Signal::DriftCorrection { goal_id, detail } => (
            ProblemKind::DriftCorrection,
            Priority::Normal,
            format!("drift:{goal_id}"),
            format!("goal {goal_id} drifting from intent: {detail}"),
        ),
        Signal::GoalBlocked {
            goal_id,
            needs_review,
            consecutive_no_action,
            ..
        } => (
            ProblemKind::GoalHygiene,
            if *needs_review {
                Priority::High
            } else {
                Priority::Normal
            },
            format!("goal:blocked:{goal_id}"),
            format!(
                "goal {goal_id} blocked{} ({consecutive_no_action} no-action cycle(s))",
                if *needs_review {
                    " — needs human review"
                } else {
                    ""
                }
            ),
        ),
        // Recall-driven (#2628): a signature seen before in memory. The dedup_key
        // is the (sanitized) signature so this MERGES into the matching in-cycle
        // problem (raising its priority in `orient`) rather than spawning a
        // duplicate; standalone it yields a High-priority advisory problem. The
        // signature is UNTRUSTED (multi-writer graph), so the summary is
        // `sanitize_recalled`-cleaned at this admission boundary before it can
        // ever reach an operator notification.
        Signal::RecurringSignature {
            signature,
            occurrences,
        } => (
            ProblemKind::ProcessHealth,
            Priority::High,
            sanitize_recalled(signature),
            sanitize_recalled(&format!(
                "recurring signature seen {occurrences}× in cognitive memory ({signature})"
            )),
        ),
        // The recurring gap-scan surfaces ONE consolidated signal per Observe
        // pass carrying every uncovered gap. It maps to a SINGLE high-priority
        // coverage problem whose dedup key is keyed PER-GAP (issue #4128, D3a):
        // `workstream-gap:<sorted gap signatures>` rather than the bare
        // `workstream-gap` constant. Keying on the specific gap set means a
        // DIFFERENT set of gaps no longer collapses onto one dedup key (which had
        // starved the closing edge / in-flight guard to a single gap). Uncovered
        // p1/p2 goals, bug/P1 issues, and live anomalies all rank High.
        Signal::WorkstreamGap { gaps } => (
            ProblemKind::WorkstreamCoverage,
            Priority::High,
            workstream_gap_key(gaps),
            format!("{} uncovered workstream(s)", gaps.len()),
        ),
        // A diagnosed step failure (#2640): a broken OODA step is HIGH priority.
        // The dedup key is the root cause so repeat failures of the SAME cause
        // collapse into one corrective problem (Orient merges same-key signals)
        // rather than spawning a workstream per occurrence.
        Signal::StepFailureDiagnosed {
            cause, exit_code, ..
        } => (
            ProblemKind::StepFailure,
            Priority::High,
            format!("step-failure:{}", cause.as_str()),
            match exit_code {
                Some(code) => {
                    format!(
                        "OODA step failed — root cause {} (exit {code})",
                        cause.as_str()
                    )
                }
                None => format!("OODA step failed — root cause {}", cause.as_str()),
            },
        ),
        // Agentic merge-queue reasoning (#4097): a PR judged STALE. A merge-queue
        // hygiene problem — flag it (comment), never merge. Low priority (advisory
        // cleanup). Dedup keyed per-PR so a recurring stale PR flags at most once.
        Signal::StalePrDetected { repo, pr } => (
            ProblemKind::DeliveryReady,
            Priority::Low,
            format!("delivery:stale-pr:{repo}#{pr}"),
            format!("PR {repo}#{pr} judged stale — no recent activity"),
        ),
        // A PR judged a DUPLICATE of another (#4097): close the duplicate,
        // referencing the original. Low priority hygiene; dedup keyed per-PR.
        Signal::DuplicatePrDetected {
            repo,
            pr,
            duplicate_of,
        } => (
            ProblemKind::DeliveryReady,
            Priority::Low,
            format!("delivery:dup-pr:{repo}#{pr}"),
            format!("PR {repo}#{pr} judged a duplicate of #{duplicate_of}"),
        ),
        // An open ISSUE triaged READY with no active workstream (#4097): the same
        // backlog-coverage family as the gap-scan, so it ranks the same. Dedup
        // keyed per-issue so a recurring ready issue surfaces at most once.
        Signal::IssueNeedsWorkstream {
            repo,
            issue,
            next_action,
        } => (
            ProblemKind::WorkstreamCoverage,
            Priority::Normal,
            format!("workstream:issue:{repo}#{issue}"),
            format!("issue {repo}#{issue} ready with no workstream — next: {next_action}"),
        ),
        // The running binary is behind merged main (#2590): a HIGH-priority
        // self-deploy problem. Dedup key is constant so a persisting drift
        // (observed every tick until deployed) folds into a single problem.
        Signal::DeployDriftDetected { behind_commits, .. } => (
            ProblemKind::DeployDrift,
            Priority::High,
            "deploy:drift".to_string(),
            format!(
                "running binary is {behind_commits} commit(s) behind merged main — \
                 self-deploy required"
            ),
        ),
    }
}

/// A single reasoned PR paired with the ground-truth GitHub facts the
/// re-narrowing projection needs (#4097). The agentic reasoner PROPOSES via
/// [`ReasonedPr`]; the rail confirms Simard-origin + objective-mergeability from
/// these authoritative fields before anything is authorized to merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionCandidate {
    /// The agentic reviewer's proposal for this PR.
    pub reasoned: ReasonedPr,
    /// `author.login` from `gh` — the anti-recursion author guard's input.
    pub author_login: String,
    /// `headRefName` from `gh` — the engineer-branch narrowing's input.
    pub head_ref: String,
    /// `isDraft` from `gh` — the draft-exclusion rail's input (#4339). A draft
    /// PR can never be merged server-side, so the projection admits ONLY
    /// `Some(false)`; `Some(true)` and `None` (unknown/absent) are excluded
    /// fail-closed.
    pub is_draft: Option<bool>,
    /// The objective-gate snapshot (`mergeable`, checks, base, labels) from `gh`.
    pub snapshot: PrSnapshot,
}

/// Re-narrow the BROAD agentic reasoning into the NARROW set of PRs authorized to
/// merge (#4097). This is the deterministic safety rail that guarantees a
/// broadened REASONING scope can NEVER widen merge AUTHORIZATION: only a PR that
/// passes EVERY gate below is projected into `ObservedState.ready_prs` (which is
/// the ONLY thing that drives `Signal::PrReadyToMerge` → the gated merge chain).
///
/// A candidate is authorized iff ALL hold:
/// 1. disposition is exactly [`PrDisposition::ReadyForMerge`] (a proposal to
///    merge — `NeedsWork`/`Stale`/`Duplicate` are never merge candidates);
/// 2. the author is NOT the overseer bot (`overseer_login`) — anti-recursion, so
///    the Overseer never merges its own artifact;
/// 3. the PR proves Simard-origin — it carries the engineer-PR label OR rides an
///    engineer-exclusive branch namespace (the same G3 narrowing
///    [`merge_ops`](crate::overseer::merge_ops) applies), so an operator's own
///    review PR sharing the author login is never merged;
/// 4. it passes the objective gates ([`evaluate_objective_gates`]: base-allowlist
///    + `MERGEABLE` + all checks green);
/// 5. it is NOT a draft (#4339) — `is_draft == Some(false)`. A draft can never be
///    merged server-side, so `Some(true)` and an unknown/absent draft state
///    (`None`) are both excluded fail-closed.
///
/// This is a pure NARROWING — it can only ever remove candidates. The
/// authoritative six-criteria merge-authority gate (with the agentic MergeJudge)
/// still runs downstream; this only decides which PRs are even proposed to it.
pub fn project_ready_prs(
    candidates: &[ProjectionCandidate],
    base_allowlist: &[String],
    overseer_login: &str,
) -> Vec<PrRef> {
    candidates
        .iter()
        .filter(|c| c.reasoned.disposition == PrDisposition::ReadyForMerge)
        // Anti-recursion author guard: never the overseer bot's own PR.
        .filter(|c| !c.author_login.eq_ignore_ascii_case(overseer_login))
        // Engineer-PR narrowing: prove Simard-origin (label OR engineer branch).
        .filter(|c| {
            c.snapshot
                .labels
                .iter()
                .any(|l| config::is_engineer_pr_label(l))
                || config::is_engineer_branch(&c.head_ref)
        })
        // Draft gate (#4339): a draft can never be merged. Admit ONLY a
        // known-non-draft PR; `Some(true)` and `None` (unknown/absent) are
        // excluded fail-closed. Pure narrowing — mirrors `survey_ready_prs`.
        .filter(|c| c.is_draft == Some(false))
        // Objective gates: base-allowlist + MERGEABLE + all checks green.
        .filter(|c| evaluate_objective_gates(&c.snapshot, base_allowlist).is_ok())
        .map(|c| PrRef {
            repo: c.reasoned.repo.clone(),
            pr: c.reasoned.pr,
        })
        .collect()
}

/// Decide: choose one `Intervention` for a `Problem`. Illustrative routing; a
/// production Overseer would use a prompt-driven reasoner with this deterministic
/// mapping as its floor (mirroring `OodaDecideBrain`'s deterministic fallback).
pub fn decide(problem: &Problem) -> Intervention {
    match problem.kind {
        ProblemKind::DeliveryReady => {
            for s in &problem.evidence {
                match s {
                    Signal::PrReadyToMerge { repo, pr } => {
                        return Intervention::VerifyAndMergePr {
                            repo: repo.clone(),
                            pr: *pr,
                        };
                    }
                    // A stale-PR proposal (#4097): flag it with a comment (never a
                    // merge/close), gated under MergeAuthority.
                    Signal::StalePrDetected { repo, pr } => {
                        return Intervention::FlagStalePr {
                            repo: repo.clone(),
                            pr: *pr,
                            note: problem.summary.clone(),
                        };
                    }
                    // A duplicate-PR proposal (#4097): close it referencing the
                    // original, gated under MergeAuthority.
                    Signal::DuplicatePrDetected {
                        repo,
                        pr,
                        duplicate_of,
                    } => {
                        return Intervention::CloseDuplicatePr {
                            repo: repo.clone(),
                            pr: *pr,
                            duplicate_of: *duplicate_of,
                        };
                    }
                    _ => {}
                }
            }
            Intervention::Report
        }
        ProblemKind::QualityRegression => {
            for s in &problem.evidence {
                if let Signal::CiFailureCluster { repo, failing } = s {
                    return Intervention::FileIssue {
                        run: OrchestratorRunBrief {
                            recipe_name: "smart-orchestrator".to_string(),
                            failed_step: "ci".to_string(),
                            source_module: repo.clone(),
                            failure_kind: "ci_failure_cluster".to_string(),
                            error_text: format!("{failing} failing checks in {repo}"),
                        },
                    };
                }
            }
            Intervention::Report
        }
        ProblemKind::ProcessHealth => Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: problem.summary.clone(),
                target_repo: "rysweet/Simard".to_string(),
                sequence_group: None,
            },
        },
        ProblemKind::CrossCutting => Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: problem.summary.clone(),
                target_repo: "rysweet/Simard".to_string(),
                // Mechanical sweeps on shared OODA-core files serialise here.
                sequence_group: Some("ooda-core".to_string()),
            },
        },
        ProblemKind::ResourcePressure => Intervention::Escalate {
            reason: problem.summary.clone(),
        },
        ProblemKind::GoalHygiene => {
            // Goal-board health takes precedence when the evidence is a blocked
            // goal: self-heal a false-parked perpetual goal, or escalate a
            // genuine "needs human review" block. A stale/re-litigated goal
            // (no `GoalBlocked` evidence) still transfers to Simard for closure.
            if let Some((goal_id, reason, perpetual, needs_review)) =
                problem.evidence.iter().find_map(|s| match s {
                    Signal::GoalBlocked {
                        goal_id,
                        reason,
                        perpetual,
                        needs_review,
                        ..
                    } => Some((goal_id.clone(), reason.clone(), *perpetual, *needs_review)),
                    _ => None,
                })
            {
                // Root-cause context (issue #2635): the recurrence of this
                // blocked-goal cause (from memory recall, folded into the WHY),
                // the one-line WHY string (for the operator escalation), and the
                // repeatedly re-parked goal escalates its ROOT CAUSE to the
                // operator instead of being blindly re-unblocked.
                let recurrence = problem.why.as_ref().map(|w| w.recurrence).unwrap_or(0);
                let why = problem
                    .why
                    .as_ref()
                    .map(|w| w.to_string())
                    .unwrap_or_default();
                return decide_blocked_goal(
                    goal_id,
                    reason,
                    perpetual,
                    needs_review,
                    recurrence,
                    why,
                );
            }
            Intervention::TransferGoal {
                goal: GoalBrief {
                    title: problem.summary.clone(),
                    rationale: "stale / re-litigated goal — transfer to Simard for closure"
                        .to_string(),
                    priority: 3,
                    target_repo: "rysweet/Simard".to_string(),
                },
            }
        }
        // A looping goal is steered with a LIGHTWEIGHT whisper by default. Only
        // once the loop reaches Simard's no-progress breaker threshold (the
        // whisper was insufficient) does the Overseer escalate to a full meeting
        // via the existing MeetingHost/TransferGoal path.
        ProblemKind::LoopDetected => {
            let consecutive = problem.evidence.iter().find_map(|s| match s {
                Signal::LoopDetected {
                    consecutive_no_action,
                    ..
                } => Some(*consecutive_no_action),
                _ => None,
            });
            let acute = consecutive
                .map(|n| n >= NO_PROGRESS_BREAKER_THRESHOLD)
                .unwrap_or(false);
            if acute {
                Intervention::TransferGoal {
                    goal: GoalBrief {
                        title: problem.summary.clone(),
                        rationale: "repeated no-progress loop — a lightweight whisper was \
                                    insufficient; escalate to a meeting with Simard"
                            .to_string(),
                        priority: 2,
                        target_repo: "rysweet/Simard".to_string(),
                    },
                }
            } else {
                Intervention::Whisper {
                    note: compose_whisper_note(problem, &ObservedState::default()),
                    urgency: WhisperUrgency::Normal,
                }
            }
        }
        // Drift is always steered with an advisory whisper.
        ProblemKind::DriftCorrection => Intervention::Whisper {
            note: compose_whisper_note(problem, &ObservedState::default()),
            urgency: WhisperUrgency::Normal,
        },
        // Backlog-coverage gaps get a CLOSING EDGE (issue #4128, D3b): launch a
        // workstream that actually COVERS the uncovered work so the gap stops
        // recurring — never the old notify-only `FlagWorkstreamGaps`, which left
        // the gap uncovered and re-surfaced it every window as the recurring
        // `workstream-gap` signature. The launch is tagged with the
        // `WORKSTREAM_COVERAGE_GROUP` sequence group so the gate can hold it when
        // the gap-scan is disabled (the opt-out that used to live on the notify
        // path) and the sequencer serialises concurrent coverage launches; the
        // per-gap signatures embedded in the brief make the in-flight guard dedup
        // a re-observed identical gap set to a SINGLE in-flight coverage launch.
        ProblemKind::WorkstreamCoverage => {
            // An agentic issue-triage proposal (#4097): a Ready issue with no
            // workstream. Launch a workstream that covers it (the same closing-edge
            // philosophy as the gap-scan), keyed to the concrete issue ref.
            if let Some((repo, issue, next_action)) =
                problem.evidence.iter().find_map(|s| match s {
                    Signal::IssueNeedsWorkstream {
                        repo,
                        issue,
                        next_action,
                    } => Some((repo.clone(), *issue, next_action.clone())),
                    _ => None,
                })
            {
                return Intervention::LaunchRecipe {
                    brief: RecipeBrief {
                        task_description: format!(
                            "Cover the ready, uncovered issue {repo}#{issue} surfaced by the \
                             Overseer's agentic merge-queue/issue triage: launch or track a \
                             workstream that addresses it. Next action: {next_action}."
                        ),
                        target_repo: repo,
                        sequence_group: Some(WORKSTREAM_COVERAGE_GROUP.to_string()),
                    },
                };
            }
            let gaps = problem
                .evidence
                .iter()
                .find_map(|s| match s {
                    Signal::WorkstreamGap { gaps } => Some(gaps.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            // Describe the gaps by category + restricted-slug signature only
            // (both bounded/safe), never free-text titles, so the brief can never
            // inflate a launched task description.
            let details = gaps
                .iter()
                .map(|g| format!("{}: {}", g.category.label(), g.signature))
                .collect::<Vec<_>>()
                .join("; ");
            Intervention::LaunchRecipe {
                brief: RecipeBrief {
                    task_description: format!(
                        "Cover uncovered backlog workstream(s) surfaced by the Overseer \
                         gap-scan so they stop recurring: launch or track a workstream that \
                         closes each gap. {n} gap(s): {details}. ({key})",
                        n = gaps.len(),
                        details = details,
                        key = workstream_gap_key(&gaps),
                    ),
                    target_repo: "rysweet/Simard".to_string(),
                    sequence_group: Some(WORKSTREAM_COVERAGE_GROUP.to_string()),
                },
            }
        }
        // A diagnosed step failure drives a CORRECTIVE workstream (#2640, PART 2):
        // launch a recipe that diagnoses the WHY and applies the remedy, keyed to
        // the real root cause and pointed at the self-diagnosis prompt asset (G3).
        // NEVER a passive Report / silent log.
        ProblemKind::StepFailure => {
            let (cause, exit_code, evidence) = problem
                .evidence
                .iter()
                .find_map(|s| match s {
                    Signal::StepFailureDiagnosed {
                        cause,
                        exit_code,
                        evidence,
                    } => Some((*cause, *exit_code, evidence.clone())),
                    _ => None,
                })
                .unwrap_or((FailureCause::Unknown, None, String::new()));
            let code = exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            Intervention::LaunchRecipe {
                brief: RecipeBrief {
                    task_description: format!(
                        "Self-diagnose and fix a failed OODA step (decision-cycle / engineer / \
                         terminal-shell). Diagnosed root cause: {cause} (exit {code}). Follow \
                         prompt_assets/simard/overseer/self_diagnose.md — determine WHY it \
                         happened from the error and last terminal output, then apply the \
                         corrective remedy so the step succeeds (do not merely log it). \
                         Evidence: {evidence}",
                        cause = cause.as_str(),
                    ),
                    target_repo: "rysweet/Simard".to_string(),
                    sequence_group: None,
                },
            }
        }
        // Autonomous self-deploy rail (#2590): a deploy-drift problem yields a
        // guarded Deploy to the merged head. This is the DETERMINISTIC trigger
        // only — the go/no-go SAFETY judgment stays in the guarded executor
        // (`evaluate_deploy_gate` refuses no-op / rollback / red-canary /
        // crash-loop) and the high-risk AutonomyGate. Fail-CLOSED: with no
        // resolved target commit in evidence it ESCALATES rather than deploying
        // blind.
        ProblemKind::DeployDrift => {
            match problem.evidence.iter().find_map(|s| match s {
                Signal::DeployDriftDetected { target_commit, .. } => Some(target_commit.clone()),
                _ => None,
            }) {
                Some(commit) if !commit.trim().is_empty() => Intervention::Deploy { commit },
                _ => Intervention::Escalate {
                    reason: format!(
                        "deploy drift detected but no target commit was resolved — escalating \
                         rather than deploying blind ({})",
                        problem.summary
                    ),
                },
            }
        }
    }
}

/// Route a blocked goal to the right stewardship action (defense-in-depth for
/// #2609 + the MANDATORY ROOT-CAUSE principle #2635):
///
/// - a RECURRING re-park (memory recall shows the SAME cause re-occurring at or
///   above [`RECURRENCE_ESCALATION_THRESHOLD`]) is NOT blindly re-unblocked every
///   cycle (the operator's rejected antipattern). Instead the ROOT CAUSE is
///   ESCALATED to the operator with the root-cause analysis so the systemic
///   defect can be fixed without generating another tracking issue;
/// - a first-time / infrequent PERPETUAL goal false-parked by the **no-progress**
///   safeguard is SELF-HEALED — auto-unblocked + reactivated (a root-cause fix
///   for a false park, not a symptom patch);
/// - ANY OTHER goal carrying a "needs human review" marker is ESCALATED to the
///   operator WITH its root-cause WHY so the marker AND its analysis reach a human;
/// - a plain operator-set / dependency block is surfaced in the periodic Report
///   and left untouched (respect the deliberate block).
///
/// Reuses the EXISTING no-progress marker predicate ([`is_no_progress_marker`])
/// and the perpetual flag derived from #2589/#2609 — it invents no new notion of
/// either.
fn decide_blocked_goal(
    goal_id: String,
    reason: String,
    perpetual: bool,
    needs_review: bool,
    recurrence: u32,
    why: String,
) -> Intervention {
    // A false-parked perpetual goal (a bare no-progress safeguard block) is still
    // SELF-HEALED locally — auto-unblocked + reactivated — because that is a
    // deterministic root-cause fix for a false park, not a judgement call that
    // needs agentic triage. Everything that genuinely needs a human decision is
    // handed to the agentic triage recipe (see below), so this stays a narrow,
    // safe self-heal rail rather than a competing escalate-vs-course-correct
    // heuristic.
    if perpetual && is_no_progress_marker(&reason) && recurrence < RECURRENCE_ESCALATION_THRESHOLD {
        return Intervention::UnblockGoal { goal_id, reason };
    }

    // Otherwise route to the AGENTIC escalation-triage recipe (issue #4276). The
    // recurrence count and `needs_review` flag are now only TRIGGERS that decide
    // whether triage runs — the escalate-vs-course-correct DECISION itself is
    // owned by the recipe, never by this bare integer threshold. A recurring
    // re-park OR a needs-review block both hand off to triage.
    if recurrence >= RECURRENCE_ESCALATION_THRESHOLD || needs_review {
        let (problem, next_step) = plain_english_blocked(&goal_id, &reason, recurrence);
        return Intervention::EscalateBlockedGoal {
            goal_id,
            reason,
            why,
            problem,
            next_step,
            // Fail-open (issue #4276, A1): the tracking issue that holds the full
            // detail is filed in the goal-curation path and is not reachable at
            // this seam, so no link is threaded here — the operator still gets the
            // plain-English problem + next step, and the triage recipe surfaces the
            // detail. Never block the escalation on link presence.
            link: None,
        };
    }
    Intervention::Report
}

/// Translate a blocked-goal's internal marker + diagnostic WHY into a
/// PLAIN-ENGLISH `(problem, next_step)` for the operator (issue #4276). The raw
/// marker (`🔒 [OODA-SAFEGUARD] … why=UNCLEAR-CRITERIA evidence=[…]`) and the
/// one-line WHY are internal jargon; this is the human-facing seed text the
/// agentic triage recipe further refines. It is deliberately free of every
/// machine token so the operator never sees `OODA-SAFEGUARD` / `UNCLEAR-CRITERIA`
/// / `evidence=[` / `why=` / the 🔒 lock marker.
fn plain_english_blocked(goal_id: &str, reason: &str, recurrence: u32) -> (String, String) {
    let no_progress = is_no_progress_marker(reason);
    let recurring = recurrence >= RECURRENCE_ESCALATION_THRESHOLD;
    let problem = if no_progress {
        format!(
            "Goal \"{goal_id}\" is stuck: Simard can't automatically tell when it is \
             finished, so it keeps re-investigating without shipping anything and is \
             making no real progress."
        )
    } else if recurring {
        format!(
            "Goal \"{goal_id}\" keeps getting blocked for the same reason over and over, \
             so Simard is unable to move it forward on its own."
        )
    } else {
        format!("Goal \"{goal_id}\" is blocked and Simard can't move it forward on its own.")
    };
    let next_step = if no_progress {
        "Give the goal a finish condition a test or check can confirm — for example a \
         specific issue CLOSED, a specific PR MERGED, or a file/command the done-gate \
         can check — or close the goal if a merged PR already delivered it."
            .to_string()
    } else {
        "Review the goal, then either give it a concrete, checkable finish line or close \
         it if it is out of scope or already delivered."
            .to_string()
    };
    (problem, next_step)
}

#[cfg(test)]
mod tests {
    use super::capabilities::*;
    use super::*;
    use crate::error::SimardResult;

    // ── Fakes: each satisfies one capability with canned values. ────────────
    struct FakeStatus(ObservedState);
    impl StatusReader for FakeStatus {
        fn snapshot(&self) -> Result<ObservedState, OverseerError> {
            Ok(self.0.clone())
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

    struct FakePrs {
        ready: bool,
    }
    impl PrOps for FakePrs {
        fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
            Ok(VerifyReport {
                ready: self.ready,
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

    struct FakeGoals(Vec<InFlightItem>);
    impl GoalCurator for FakeGoals {
        fn propose(&self, _goal: &GoalBrief) -> Result<(), OverseerError> {
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

    fn caps(observed: ObservedState, ready: bool, in_flight: Vec<InFlightItem>) -> Capabilities {
        Capabilities {
            status: Box::new(FakeStatus(observed)),
            recipes: Box::new(FakeRecipes),
            prs: Box::new(FakePrs { ready }),
            deployer: Box::new(FakeDeployer),
            meetings: Box::new(FakeMeetings),
            issues: Box::new(FakeIssues),
            goals: Box::new(FakeGoals(in_flight)),
            auditor: Box::new(FakeAuditor),
            memory: Box::new(capabilities::InertMemoryRecall),
        }
    }

    /// A fake drift sensor for the OBSERVE-rail tests: returns a fixed
    /// observation (or `None`) so `run_cycle`'s drift wiring can be exercised
    /// without a live git checkout.
    struct FakeDriftObserver(Option<capabilities::DeployDriftObservation>);
    impl deploy_trigger::DeployDriftObserver for FakeDriftObserver {
        fn observe(&self) -> Option<capabilities::DeployDriftObservation> {
            self.0.clone()
        }
    }

    #[test]
    fn observe_rail_behind_main_emits_deploy_signal_and_admitted_plan() {
        // A wired daemon whose running binary is behind merged main OBSERVEs the
        // drift, raises `DeployDriftDetected`, and (high-risk autonomy on) plans
        // an ADMITTED guarded Deploy to the merged head — no operator command.
        let _guard = deploy_trigger::deploy_throttle_test_guard();
        deploy_trigger::reset_global_deploy_throttle();
        let mut ov = Overseer::new(caps(ObservedState::default(), false, vec![]))
            .with_high_risk_autonomy(true)
            .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(
                capabilities::DeployDriftObservation {
                    target_commit: "mergedHEADabc".to_string(),
                    behind_commits: 3,
                },
            ))));
        let report = ov.run_cycle().expect("cycle");

        assert!(
            report.signals.iter().any(|s| matches!(
                s,
                Signal::DeployDriftDetected { target_commit, behind_commits }
                    if target_commit == "mergedHEADabc" && *behind_commits == 3
            )),
            "the observe rail must raise DeployDriftDetected"
        );
        let deploy = report
            .plan
            .iter()
            .find(|p| p.intervention.label() == "deploy")
            .expect("a Deploy intervention is planned");
        assert!(deploy.admitted, "high-risk autonomy admits the deploy");
        assert!(
            matches!(&deploy.intervention, Intervention::Deploy { commit } if commit == "mergedHEADabc"),
            "Deploy targets the merged head"
        );
        deploy_trigger::reset_global_deploy_throttle();
    }

    #[test]
    fn observe_rail_suppresses_deploy_during_a_crash_loop() {
        // Live crash-loop guard (#2590): never deploy into a restart storm, even
        // with drift present. Reuses the observed restart churn + the deploy
        // gate's threshold.
        let _guard = deploy_trigger::deploy_throttle_test_guard();
        deploy_trigger::reset_global_deploy_throttle();
        let churny = ObservedState {
            restart_churn: Some(deploy::CRASH_LOOP_CHURN_THRESHOLD),
            ..ObservedState::default()
        };
        let mut ov = Overseer::new(caps(churny, false, vec![]))
            .with_high_risk_autonomy(true)
            .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(
                capabilities::DeployDriftObservation {
                    target_commit: "mergedHEADabc".to_string(),
                    behind_commits: 1,
                },
            ))));
        let report = ov.run_cycle().expect("cycle");
        assert!(
            !report
                .signals
                .iter()
                .any(|s| matches!(s, Signal::DeployDriftDetected { .. })),
            "a crash-loop must suppress the deploy signal"
        );
        deploy_trigger::reset_global_deploy_throttle();
    }

    #[test]
    fn observe_rail_anti_thrash_second_tick_within_window_does_not_re_emit() {
        // The process-global throttle admits at most one deploy attempt per
        // min-interval, so a drift re-observed on the very next tick (the daemon
        // rebuilds the Overseer each tick) does not re-emit a deploy signal.
        let _guard = deploy_trigger::deploy_throttle_test_guard();
        deploy_trigger::reset_global_deploy_throttle();
        let obs = || capabilities::DeployDriftObservation {
            target_commit: "mergedHEADabc".to_string(),
            behind_commits: 1,
        };
        let mut first = Overseer::new(caps(ObservedState::default(), false, vec![]))
            .with_high_risk_autonomy(true)
            .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(obs()))));
        let r1 = first.run_cycle().expect("cycle 1");
        assert!(
            r1.signals
                .iter()
                .any(|s| matches!(s, Signal::DeployDriftDetected { .. })),
            "first tick emits the drift signal"
        );
        // A fresh Overseer (as the daemon rebuilds each tick) re-observes the
        // same drift, but the process-global throttle is still inside its window.
        let mut second = Overseer::new(caps(ObservedState::default(), false, vec![]))
            .with_high_risk_autonomy(true)
            .with_deploy_drift_observer(Box::new(FakeDriftObserver(Some(obs()))));
        let r2 = second.run_cycle().expect("cycle 2");
        assert!(
            !r2.signals
                .iter()
                .any(|s| matches!(s, Signal::DeployDriftDetected { .. })),
            "second tick within the min-interval must not re-emit a deploy"
        );
        deploy_trigger::reset_global_deploy_throttle();
    }

    #[test]
    fn observe_rail_is_inert_without_a_wired_observer() {
        // No sensor wired ⇒ the rail never touches the throttle and never emits a
        // deploy signal (the pre-wiring behaviour is exactly preserved).
        let mut ov = Overseer::new(caps(ObservedState::default(), false, vec![]))
            .with_high_risk_autonomy(true);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            !report
                .signals
                .iter()
                .any(|s| matches!(s, Signal::DeployDriftDetected { .. })),
            "an unwired rail emits no deploy signal"
        );
    }

    #[test]
    fn signals_only_fire_above_threshold() {
        let below = ObservedState {
            distill_fail_pct: Some(5.0),
            ..ObservedState::default()
        };
        assert!(signals_from(&below).is_empty());
        // the real-world ~62% case
        let high = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let sigs = signals_from(&high);
        assert_eq!(sigs, vec![Signal::DistillFailureRate { pct: 62.0 }]);
    }

    #[test]
    fn orient_dedups_against_in_flight() {
        let signals = vec![Signal::DistillFailureRate { pct: 62.0 }];
        // An engineer is already on it (same dedup key) → no problem raised.
        let in_flight = vec![InFlightItem {
            id: "g1".to_string(),
            source: "ooda".to_string(),
            refs: vec!["process:distill_fail".to_string()],
        }];
        assert!(orient(&signals, &in_flight).is_empty());
        // Nobody on it → one problem.
        assert_eq!(orient(&signals, &[]).len(), 1);
    }

    #[test]
    fn run_cycle_plans_a_launch_for_process_health() {
        let st = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let mut ov = Overseer::new(caps(st, true, vec![]));
        let report = ov.run_cycle().expect("cycle");
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.plan.len(), 1);
        let planned = &report.plan[0];
        assert!(planned.admitted);
        assert_eq!(planned.intervention.label(), "launch_recipe");
    }

    #[test]
    fn high_risk_deploy_is_gated_by_default() {
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]));
        // Default autonomy holds a deploy.
        let held = ov.gate(
            &Intervention::Deploy {
                commit: "abc123".to_string(),
            },
            &ObservedState::default(),
            &mut 0,
        );
        assert!(!held.admitted);
        // Opt-in autonomy admits it.
        let mut ov = ov.with_high_risk_autonomy(true);
        let admitted = ov.gate(
            &Intervention::Deploy {
                commit: "abc123".to_string(),
            },
            &ObservedState::default(),
            &mut 0,
        );
        assert!(admitted.admitted);
    }

    #[test]
    fn budget_pressure_holds_launches() {
        let observed = ObservedState {
            spent_today_usd: Some(600.0),
            daily_budget_usd: Some(500.0),
            ..ObservedState::default()
        };
        let mut ov = Overseer::new(caps(observed.clone(), true, vec![]));
        let held = ov.gate(
            &Intervention::LaunchRecipe {
                brief: RecipeBrief {
                    task_description: "x".to_string(),
                    target_repo: "rysweet/Simard".to_string(),
                    sequence_group: None,
                },
            },
            &observed,
            &mut 0,
        );
        assert!(!held.admitted);
    }

    #[test]
    fn duplicate_investigation_for_an_in_flight_signature_is_held() {
        // Live-daemon defect (2026-07-15): the overseer had TWO recipe-runner
        // processes (PIDs 1074394 and 1095553) investigating the SAME recurring
        // signature simultaneously, because a recurring `overseer-obs:goal:blocked`
        // signature re-observed each cycle launched a FRESH `smart-orchestrator`
        // recipe while the prior one was still running. `sequence_group` is `None`
        // for these ProcessHealth/RecurringSignature launches, so the conflict
        // sequencer never dedups them. A launch-site guard must ensure a given
        // goal / recurring-signature has at most ONE investigation in flight.
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]));
        let brief = RecipeBrief {
            task_description: "recurring signature seen 2× in cognitive memory \
                               (overseer-obs:goal:blocked:simard-identity-coherence)"
                .to_string(),
            target_repo: "rysweet/Simard".to_string(),
            sequence_group: None,
        };
        let iv = Intervention::LaunchRecipe {
            brief: brief.clone(),
        };

        // Cycle 1: the first investigation for this signature is admitted and the
        // recipe-runner is launched (still Running, per FakeRecipes::poll).
        let first = ov.gate(&iv, &ObservedState::default(), &mut 0);
        assert!(
            first.admitted,
            "the first investigation for a signature must be admitted"
        );
        let launched = ov.act(&first.intervention).expect("launch");
        assert!(
            matches!(launched, ActOutcome::Launched(_)),
            "the admitted investigation must actually launch a workstream"
        );

        // Cycle 2: the SAME recurring signature is re-observed while the first
        // investigation is still in flight. It MUST be held — never a second
        // concurrent recipe-runner for the same signature.
        let second = ov.gate(&iv, &ObservedState::default(), &mut 0);
        assert!(
            !second.admitted,
            "a duplicate investigation for an in-flight signature must be HELD, \
             got admitted plan: {second:?}"
        );
        assert!(
            second.note.to_ascii_lowercase().contains("flight")
                || second.note.to_ascii_lowercase().contains("in-flight")
                || second.note.to_ascii_lowercase().contains("already"),
            "the hold reason must explain the in-flight dedup: {:?}",
            second.note
        );

        // A DIFFERENT signature is unaffected — the guard dedups per signature,
        // never a blanket launch freeze.
        let other = Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: "recurring signature seen 2× in cognitive memory \
                                   (overseer-obs:goal:blocked:some-other-goal)"
                    .to_string(),
                target_repo: "rysweet/Simard".to_string(),
                sequence_group: None,
            },
        };
        let other_plan = ov.gate(&other, &ObservedState::default(), &mut 0);
        assert!(
            other_plan.admitted,
            "an investigation for a DIFFERENT signature must still be admitted"
        );
    }

    #[test]
    fn anti_recursion_refuses_own_pr() {
        let guard = RecursionGuard {
            author_login: "simard-overseer[bot]".to_string(),
            branch_prefix: "overseer/".to_string(),
            goal_source_tag: "overseer:".to_string(),
        };
        let own = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
            author: "simard-overseer[bot]".to_string(),
        };
        assert!(guard.is_own(&own));
        let foreign = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 2,
            author: "someone-else".to_string(),
        };
        assert!(!guard.is_own(&foreign));
    }

    #[test]
    fn conflict_sequencer_serialises_sweeps() {
        let mut seq = ConflictSequencer::default();
        assert!(seq.admit(Some("ooda-core")).is_ok());
        // A second sweep on the same shared files is held until the first frees.
        assert!(seq.admit(Some("ooda-core")).is_err());
        seq.release("ooda-core");
        assert!(seq.admit(Some("ooda-core")).is_ok());
        // Unsequenced feature work is always admitted.
        assert!(seq.admit(None).is_ok());
    }

    #[test]
    fn act_dispatches_merge_when_ready() {
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]));
        let out = ov
            .act(&Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 7,
            })
            .expect("act");
        assert_eq!(out, ActOutcome::Merged);
    }

    #[test]
    fn act_escalates_merge_when_not_ready() {
        let mut ov = Overseer::new(caps(ObservedState::default(), false, vec![]));
        let out = ov
            .act(&Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 7,
            })
            .expect("act");
        assert_eq!(out, ActOutcome::Escalated);
    }

    // ── #4097: a NotMergeReady from the authoritative agentic merge step must
    //    ESCALATE (be handed to the operator), never be tallied as an error and
    //    never a blind merge. A genuine capability/safety error still propagates.

    /// A `PrOps` whose `verify()` passes the objective pre-filter but whose
    /// `merge()` returns a scripted result — so we can drive the Act handler's
    /// mapping of the authoritative merge step's outcome.
    struct ScriptedMergePrs {
        ready: bool,
        merge_result: fn(&str, u32) -> Result<(), OverseerError>,
    }
    impl PrOps for ScriptedMergePrs {
        fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
            Ok(VerifyReport {
                ready: self.ready,
                checks: vec![],
            })
        }
        fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
            (self.merge_result)(repo, pr)
        }
        fn resolve_conflict(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
            Ok(())
        }
    }

    fn caps_with_prs(prs: Box<dyn PrOps>) -> Capabilities {
        Capabilities {
            status: Box::new(FakeStatus(ObservedState::default())),
            recipes: Box::new(FakeRecipes),
            prs,
            deployer: Box::new(FakeDeployer),
            meetings: Box::new(FakeMeetings),
            issues: Box::new(FakeIssues),
            goals: Box::new(FakeGoals(vec![])),
            auditor: Box::new(FakeAuditor),
            memory: Box::new(capabilities::InertMemoryRecall),
        }
    }

    #[test]
    fn act_escalates_when_merge_returns_not_merge_ready() {
        // verify() is ready (objective pre-filter passed), but the authoritative
        // agentic merge step refuses → NotMergeReady. The Act handler must map
        // this to an ESCALATION, not propagate it as an error.
        let prs = ScriptedMergePrs {
            ready: true,
            merge_result: |_repo, pr| {
                Err(OverseerError::NotMergeReady {
                    pr,
                    reason: "the merge-readiness review did not approve yet".to_string(),
                })
            },
        };
        let mut ov = Overseer::new(caps_with_prs(Box::new(prs)));
        let out = ov
            .act(&Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 4097,
            })
            .expect("a NotMergeReady must be handled as an escalation, not an Err");
        assert_eq!(
            out,
            ActOutcome::Escalated,
            "a not-ready-now merge refusal escalates to the operator"
        );
    }

    #[test]
    fn act_propagates_a_genuine_capability_error_from_merge() {
        // A genuine infra/safety failure (e.g. the anti-recursion guard, a `gh`
        // failure) must still propagate as an Err so the tick counts it under
        // `errors` — never silently downgraded to an escalation.
        let prs = ScriptedMergePrs {
            ready: true,
            merge_result: |_repo, _pr| {
                Err(OverseerError::Capability {
                    what: "merge.recursion",
                    detail: "refused own PR".to_string(),
                })
            },
        };
        let mut ov = Overseer::new(caps_with_prs(Box::new(prs)));
        let res = ov.act(&Intervention::VerifyAndMergePr {
            repo: "rysweet/Simard".to_string(),
            pr: 4097,
        });
        assert!(
            matches!(res, Err(OverseerError::Capability { .. })),
            "a genuine capability/safety error must propagate as an error, got {res:?}"
        );
    }

    // ── ecosystem-observe rail (issue #2419) ────────────────────────────────

    /// A fake [`ecosystem_observe::EcosystemObserver`] that records the roster +
    /// in-flight refs it was handed and returns a scripted outcome — no
    /// subprocess, no `gh`, no recipe runner.
    type EcoCallLog = std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, Vec<String>)>>>;
    struct FakeEcoObserver {
        outcome: SimardResult<Option<String>>,
        seen: EcoCallLog,
    }
    impl FakeEcoObserver {
        fn returning(outcome: SimardResult<Option<String>>) -> Self {
            Self {
                outcome,
                seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
        /// Build one that shares its call log with the caller, so a test can
        /// inspect what the rail forwarded after the observer is boxed.
        fn with_log(outcome: SimardResult<Option<String>>) -> (Self, EcoCallLog) {
            let seen: EcoCallLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    outcome,
                    seen: std::sync::Arc::clone(&seen),
                },
                seen,
            )
        }
    }
    impl ecosystem_observe::EcosystemObserver for FakeEcoObserver {
        fn observe(
            &self,
            roster: &[String],
            inflight_refs: &[String],
        ) -> SimardResult<Option<String>> {
            self.seen
                .lock()
                .unwrap()
                .push((roster.to_vec(), inflight_refs.to_vec()));
            match &self.outcome {
                Ok(v) => Ok(v.clone()),
                Err(_) => Err(crate::error::SimardError::AdapterInvocationFailed {
                    base_type: "ecosystem-observe".to_string(),
                    reason: "scripted failure".to_string(),
                }),
            }
        }
    }

    fn overseer_with_eco(outcome: SimardResult<Option<String>>, every_n: u64) -> Overseer {
        // No process-health signal, so any launch in the plan comes solely from
        // the ecosystem-observe rail.
        Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_ecosystem_observer(
                vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
                Box::new(FakeEcoObserver::returning(outcome)),
                every_n,
            )
    }

    #[test]
    fn ecosystem_observe_routes_brief_into_a_gated_launch() {
        let brief = "PROBLEM: azlin CI red -> brief: fix flaky provisioning test".to_string();
        let mut ov = overseer_with_eco(Ok(Some(brief.clone())), 1);
        let report = ov.run_cycle().expect("cycle");
        // The rail's brief became one admitted LaunchRecipe in the plan.
        let launches: Vec<_> = report
            .plan
            .iter()
            .filter(|p| p.intervention.label() == "launch_recipe")
            .collect();
        assert_eq!(launches.len(), 1, "the rail adds exactly one launch");
        assert!(launches[0].admitted, "the launch is gated-admitted");
        match &launches[0].intervention {
            Intervention::LaunchRecipe { brief: b } => {
                assert_eq!(
                    b.task_description, brief,
                    "the opaque brief is forwarded verbatim"
                );
                assert_eq!(b.target_repo, ECOSYSTEM_OBSERVE_TARGET);
            }
            other => panic!("expected LaunchRecipe, got {other:?}"),
        }
    }

    #[test]
    fn ecosystem_observe_none_adds_no_launch() {
        let mut ov = overseer_with_eco(Ok(None), 1);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report
                .plan
                .iter()
                .all(|p| p.intervention.label() != "launch_recipe"),
            "a None observation fabricates no launch"
        );
    }

    #[test]
    fn ecosystem_observe_failure_degrades_without_launch() {
        let mut ov = overseer_with_eco(
            Err(crate::error::SimardError::AdapterInvocationFailed {
                base_type: "ecosystem-observe".to_string(),
                reason: "boom".to_string(),
            }),
            1,
        );
        let report = ov
            .run_cycle()
            .expect("cycle must not error when the rail fails");
        assert!(
            report
                .plan
                .iter()
                .all(|p| p.intervention.label() != "launch_recipe"),
            "a rail failure degrades safely and fabricates no launch"
        );
    }

    #[test]
    fn ecosystem_observe_skipped_when_disabled() {
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(false) // opt-out disables the rail
            .with_ecosystem_observer(
                vec!["rysweet/Simard".to_string()],
                Box::new(FakeEcoObserver::returning(Ok(Some("brief".to_string())))),
                1,
            );
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report
                .plan
                .iter()
                .all(|p| p.intervention.label() != "launch_recipe"),
            "the gap-scan opt-out also disables the ecosystem-observe rail"
        );
    }

    #[test]
    fn ecosystem_observe_respects_every_n_cadence() {
        // every_n = 2 → observe on ticks 0, 2, 4; skip 1, 3.
        let mut ov = overseer_with_eco(Ok(Some("brief".to_string())), 2);
        let launched = |r: &CycleReport| {
            r.plan
                .iter()
                .any(|p| p.intervention.label() == "launch_recipe")
        };
        assert!(launched(&ov.run_cycle().unwrap()), "tick 0 observes");
        assert!(!launched(&ov.run_cycle().unwrap()), "tick 1 skipped");
        assert!(launched(&ov.run_cycle().unwrap()), "tick 2 observes");
        assert!(!launched(&ov.run_cycle().unwrap()), "tick 3 skipped");
    }

    #[test]
    fn ecosystem_observe_unwired_is_a_noop() {
        // No `.with_ecosystem_observer(...)` → the pass is skipped entirely.
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report
                .plan
                .iter()
                .all(|p| p.intervention.label() != "launch_recipe"),
            "an unwired rail never contributes a launch"
        );
    }

    #[test]
    fn ecosystem_observe_hands_inflight_refs_for_dedup() {
        let (observer, log) = FakeEcoObserver::with_log(Ok(None));
        let in_flight = vec![InFlightItem {
            id: "g1".to_string(),
            source: "ooda".to_string(),
            refs: vec!["issue:rysweet/Simard#42".to_string()],
        }];
        let mut ov = Overseer::new(caps(ObservedState::default(), true, in_flight))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_ecosystem_observer(vec!["rysweet/Simard".to_string()], Box::new(observer), 1);
        ov.run_cycle().expect("cycle");
        let calls = log.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "the rail invokes the observer once per due tick"
        );
        assert_eq!(
            calls[0].0,
            vec!["rysweet/Simard".to_string()],
            "the roster is forwarded to the observer"
        );
        assert_eq!(
            calls[0].1,
            vec!["issue:rysweet/Simard#42".to_string()],
            "Simard's in-flight OODA refs are flattened and forwarded for dedup"
        );
    }

    // ── [standing] agentic health-review wiring ───────────────────────────────

    /// A scripted [`health_review::HealthReviewer`] returning a fixed outcome and
    /// counting the reviews it saw, so a test can assert cadence + routing.
    struct FakeHealthReviewer {
        outcome: SimardResult<Vec<Intervention>>,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl FakeHealthReviewer {
        fn returning(outcome: SimardResult<Vec<Intervention>>) -> Self {
            Self {
                outcome,
                calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
        fn with_counter(
            outcome: SimardResult<Vec<Intervention>>,
        ) -> (Self, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
            let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Self {
                    outcome,
                    calls: std::sync::Arc::clone(&calls),
                },
                calls,
            )
        }
    }
    impl health_review::HealthReviewer for FakeHealthReviewer {
        fn review(&self) -> SimardResult<Vec<Intervention>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &self.outcome {
                Ok(ivs) => Ok(ivs.clone()),
                Err(_) => Err(crate::error::SimardError::AdapterInvocationFailed {
                    base_type: "overseer-health-review".to_string(),
                    reason: "scripted failure".to_string(),
                }),
            }
        }
    }

    fn launch_iv(desc: &str) -> Intervention {
        Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: desc.to_string(),
                target_repo: "rysweet/Simard".to_string(),
                sequence_group: None,
            },
        }
    }
    fn escalate_iv(goal_id: &str) -> Intervention {
        Intervention::EscalateBlockedGoal {
            goal_id: goal_id.to_string(),
            reason: "crash_loop".to_string(),
            why: "shared failure signature across goals".to_string(),
            problem: "The daemon is stuck retrying the same broken step over and over.".to_string(),
            next_step: "A human should inspect the failing step and unblock it.".to_string(),
            link: None,
        }
    }

    fn overseer_with_health(outcome: SimardResult<Vec<Intervention>>, every_n: u64) -> Overseer {
        // No process-health signal, so any intervention in the plan comes solely
        // from the health-review rail.
        Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_goal_health_enabled(true)
            .with_health_reviewer(Box::new(FakeHealthReviewer::returning(outcome)), every_n)
            .with_health_review_enabled(true)
    }

    #[test]
    fn health_review_routes_launch_and_escalate_into_gated_plan() {
        let outcome = Ok(vec![
            launch_iv("fix the crash-looping provisioning step"),
            escalate_iv("g-42"),
        ]);
        let mut ov = overseer_with_health(outcome, 1);
        let report = ov.run_cycle().expect("cycle");
        let launches: Vec<_> = report
            .plan
            .iter()
            .filter(|p| p.intervention.label() == "launch_recipe")
            .collect();
        assert_eq!(launches.len(), 1, "the rail adds exactly one launch");
        assert!(launches[0].admitted, "the launch is gated-admitted");
        let escalations: Vec<_> = report
            .plan
            .iter()
            .filter(|p| matches!(p.intervention, Intervention::EscalateBlockedGoal { .. }))
            .collect();
        assert_eq!(escalations.len(), 1, "the rail adds exactly one escalation");
        assert!(
            escalations[0].admitted,
            "the escalation is gated-admitted with goal-health on"
        );
    }

    #[test]
    fn health_review_healthy_verdict_adds_nothing() {
        // A HEALTHY verdict parses to an empty Vec at the rail; the pass adds
        // nothing to the plan.
        let mut ov = overseer_with_health(Ok(vec![]), 1);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report.plan.iter().all(|p| {
                p.intervention.label() != "launch_recipe"
                    && !matches!(p.intervention, Intervention::EscalateBlockedGoal { .. })
            }),
            "a HEALTHY review fabricates no intervention"
        );
    }

    #[test]
    fn health_review_failure_degrades_without_intervention() {
        let mut ov = overseer_with_health(
            Err(crate::error::SimardError::AdapterInvocationFailed {
                base_type: "overseer-health-review".to_string(),
                reason: "boom".to_string(),
            }),
            1,
        );
        let report = ov
            .run_cycle()
            .expect("cycle must not error when the rail fails");
        assert!(
            report.plan.iter().all(|p| {
                p.intervention.label() != "launch_recipe"
                    && !matches!(p.intervention, Intervention::EscalateBlockedGoal { .. })
            }),
            "a rail failure degrades safely and fabricates nothing"
        );
    }

    #[test]
    fn health_review_skipped_when_gap_scan_disabled() {
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(false) // shared throttle disables the rail
            .with_goal_health_enabled(true)
            .with_health_reviewer(
                Box::new(FakeHealthReviewer::returning(Ok(vec![launch_iv("fix")]))),
                1,
            )
            .with_health_review_enabled(true);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report
                .plan
                .iter()
                .all(|p| p.intervention.label() != "launch_recipe"),
            "the gap-scan opt-out also disables the health-review rail"
        );
    }

    #[test]
    fn health_review_skipped_when_dedicated_flag_disabled() {
        // Reviewer wired, gap-scan on, but the dedicated opt-out is off.
        let (reviewer, calls) = FakeHealthReviewer::with_counter(Ok(vec![launch_iv("fix")]));
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_goal_health_enabled(true)
            .with_health_reviewer(Box::new(reviewer), 1)
            .with_health_review_enabled(false);
        let report = ov.run_cycle().expect("cycle");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a disabled rail never invokes the reviewer"
        );
        assert!(
            report
                .plan
                .iter()
                .all(|p| p.intervention.label() != "launch_recipe"),
            "a disabled rail contributes nothing"
        );
    }

    #[test]
    fn health_review_respects_every_n_cadence() {
        // every_n = 2 → review on ticks 0, 2; skip 1, 3.
        let mut ov = overseer_with_health(Ok(vec![launch_iv("fix")]), 2);
        let launched = |r: &CycleReport| {
            r.plan
                .iter()
                .any(|p| p.intervention.label() == "launch_recipe")
        };
        assert!(launched(&ov.run_cycle().unwrap()), "tick 0 reviews");
        assert!(!launched(&ov.run_cycle().unwrap()), "tick 1 skipped");
        assert!(launched(&ov.run_cycle().unwrap()), "tick 2 reviews");
        assert!(!launched(&ov.run_cycle().unwrap()), "tick 3 skipped");
    }

    #[test]
    fn health_review_unwired_is_a_noop() {
        // No `.with_health_reviewer(...)` → the pass is skipped entirely.
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report.plan.iter().all(|p| {
                p.intervention.label() != "launch_recipe"
                    && !matches!(p.intervention, Intervention::EscalateBlockedGoal { .. })
            }),
            "an unwired rail never contributes anything"
        );
    }

    #[test]
    fn health_review_escalation_held_when_goal_health_disabled() {
        // Reviewer emits an escalation, but goal-board health is opted out: the
        // gate HOLDS it (present, not admitted) rather than acting.
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_goal_health_enabled(false)
            .with_health_reviewer(
                Box::new(FakeHealthReviewer::returning(Ok(vec![escalate_iv("g-7")]))),
                1,
            )
            .with_health_review_enabled(true);
        let report = ov.run_cycle().expect("cycle");
        let escalations: Vec<_> = report
            .plan
            .iter()
            .filter(|p| matches!(p.intervention, Intervention::EscalateBlockedGoal { .. }))
            .collect();
        assert_eq!(
            escalations.len(),
            1,
            "the escalation is present in the plan"
        );
        assert!(
            !escalations[0].admitted,
            "goal-health opt-out holds the escalation (not admitted)"
        );
    }

    // ── #4097 observe-merge-queue reasoning wiring ────────────────────────────

    /// A scripted [`merge_queue_observe::MergeQueueReasoner`] that returns a fixed
    /// outcome and records the requests it saw, so a test can assert the rail
    /// forwarded the resolved scope + in-flight refs.
    type MqCallLog = std::sync::Arc<std::sync::Mutex<Vec<(Vec<String>, Vec<String>)>>>;
    struct FakeMergeQueueReasoner {
        outcome: SimardResult<Option<String>>,
        seen: MqCallLog,
    }
    impl FakeMergeQueueReasoner {
        fn with_log(outcome: SimardResult<Option<String>>) -> (Self, MqCallLog) {
            let seen: MqCallLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    outcome,
                    seen: std::sync::Arc::clone(&seen),
                },
                seen,
            )
        }
    }
    impl merge_queue_observe::MergeQueueReasoner for FakeMergeQueueReasoner {
        fn observe(
            &self,
            request: merge_queue_observe::MergeQueueObserveRequest,
        ) -> SimardResult<Option<String>> {
            self.seen
                .lock()
                .unwrap()
                .push((request.scope.clone(), request.inflight_refs.clone()));
            match &self.outcome {
                Ok(v) => Ok(v.clone()),
                Err(_) => Err(crate::error::SimardError::AdapterInvocationFailed {
                    base_type: "observe-merge-queue".to_string(),
                    reason: "scripted failure".to_string(),
                }),
            }
        }
    }

    /// A `PrOps` whose `project_reasoned_ready_prs` returns a scripted projection,
    /// standing in for the deterministic rail that fetches authoritative `gh`
    /// facts. Everything else is trivial.
    struct ProjectingPrs {
        projected: Vec<PrRef>,
    }
    impl PrOps for ProjectingPrs {
        fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
            Ok(VerifyReport {
                ready: true,
                checks: vec![],
            })
        }
        fn merge(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
            Ok(())
        }
        fn resolve_conflict(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
            Ok(())
        }
        fn project_reasoned_ready_prs(
            &self,
            _reasoned: &[capabilities::ReasonedPr],
            _overseer_login: &str,
        ) -> Vec<PrRef> {
            self.projected.clone()
        }
    }

    const MQ_BRIEF: &str = r#"{
        "reasoned_prs": [
            {"repo": "rysweet/Simard", "pr": 10, "disposition": "stale", "rationale": "no activity for 30 days"},
            {"repo": "rysweet/Simard", "pr": 11, "disposition": "ready-for-merge", "rationale": "green + mergeable"}
        ],
        "triaged_issues": [
            {"repo": "rysweet/Simard", "issue": 20, "priority": "high", "readiness": "ready", "next_action": "start a workstream"}
        ]
    }"#;

    #[test]
    fn merge_queue_reasoning_populates_observed_state_default_on_over_roster() {
        // Unset SIMARD_MERGE_REASONING_SCOPE => default-ON over the governed
        // roster. The agentic brief populates the new reasoned fields even though
        // SIMARD_AUTOMERGE_* are unset — the dead-wire fix (#4097).
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_merge_queue_reasoner(
                vec!["rysweet/Simard".to_string()],
                Box::new(FakeMergeQueueReasoner::with_log(Ok(Some(MQ_BRIEF.to_string()))).0),
                1,
            );
        let report = ov.run_cycle().expect("cycle");
        assert_eq!(
            report.observed.reasoned_prs.len(),
            2,
            "both reasoned PRs survive the roster trust boundary"
        );
        assert_eq!(report.observed.triaged_issues.len(), 1);
        assert_eq!(
            report.observed.merge_reasoning_status,
            capabilities::MergeReasoningStatus::RosterWide,
            "unset scope => reasoning ran default-ON over the governed roster"
        );
        // A stale-PR reasoning conclusion becomes a StalePrDetected signal…
        assert!(
            report
                .signals
                .iter()
                .any(|s| matches!(s, Signal::StalePrDetected { pr, .. } if *pr == 10)),
            "the stale reasoning conclusion drives a StalePrDetected signal"
        );
        // …but a ReadyForMerge conclusion NEVER fabricates a PrReadyToMerge (only
        // the re-narrowed ready_prs projection may) — the core safety invariant.
        assert!(
            !report
                .signals
                .iter()
                .any(|s| matches!(s, Signal::PrReadyToMerge { pr, .. } if *pr == 11)),
            "ReadyForMerge reasoning alone must never authorize a merge"
        );
    }

    #[test]
    fn merge_queue_reasoning_forwards_scope_and_inflight_refs() {
        let (reasoner, log) = FakeMergeQueueReasoner::with_log(Ok(None));
        let in_flight = vec![InFlightItem {
            id: "g1".to_string(),
            source: "ooda".to_string(),
            refs: vec!["pr:rysweet/Simard#10".to_string()],
        }];
        let mut ov = Overseer::new(caps(ObservedState::default(), true, in_flight))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(true)
            .with_merge_queue_reasoner(vec!["rysweet/Simard".to_string()], Box::new(reasoner), 1);
        ov.run_cycle().expect("cycle");
        let calls = log.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "the rail invokes the reasoner once per tick"
        );
        assert_eq!(calls[0].0, vec!["rysweet/Simard".to_string()]);
        assert_eq!(calls[0].1, vec!["pr:rysweet/Simard#10".to_string()]);
    }

    #[test]
    fn merge_queue_projection_derives_ready_prs_from_reasoning() {
        // The deterministic rail re-narrows the agentic ReadyForMerge proposals
        // into ready_prs. Here the projecting PrOps admits PR #11.
        let projected = vec![PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 11,
        }];
        let mut ov = Overseer::new(Capabilities {
            status: Box::new(FakeStatus(ObservedState::default())),
            recipes: Box::new(FakeRecipes),
            prs: Box::new(ProjectingPrs { projected }),
            deployer: Box::new(FakeDeployer),
            meetings: Box::new(FakeMeetings),
            issues: Box::new(FakeIssues),
            goals: Box::new(FakeGoals(vec![])),
            auditor: Box::new(FakeAuditor),
            memory: Box::new(capabilities::InertMemoryRecall),
        })
        .with_high_risk_autonomy(true)
        .with_gap_scan_enabled(true)
        .with_merge_queue_reasoner(
            vec!["rysweet/Simard".to_string()],
            Box::new(FakeMergeQueueReasoner::with_log(Ok(Some(MQ_BRIEF.to_string()))).0),
            1,
        );
        let report = ov.run_cycle().expect("cycle");
        assert!(
            report
                .observed
                .ready_prs
                .iter()
                .any(|p| p.repo == "rysweet/Simard" && p.pr == 11),
            "the ReadyForMerge proposal is projected into ready_prs via the rail"
        );
        // ready_prs drives PrReadyToMerge — now legitimately, from the projection.
        assert!(
            report
                .signals
                .iter()
                .any(|s| matches!(s, Signal::PrReadyToMerge { pr, .. } if *pr == 11)),
            "the projected ready PR is the ONLY path to PrReadyToMerge"
        );
    }

    #[test]
    fn merge_queue_reasoning_unwired_leaves_observation_untouched() {
        // No reasoner wired => the pass is skipped and the new fields keep their
        // additive defaults (Unknown status, empty reasoned/triaged).
        let mut ov =
            Overseer::new(caps(ObservedState::default(), true, vec![])).with_gap_scan_enabled(true);
        let report = ov.run_cycle().expect("cycle");
        assert!(report.observed.reasoned_prs.is_empty());
        assert!(report.observed.triaged_issues.is_empty());
        assert_eq!(
            report.observed.merge_reasoning_status,
            capabilities::MergeReasoningStatus::Unknown
        );
    }

    #[test]
    fn merge_queue_reasoning_gap_scan_optout_is_visible_not_silent() {
        // The gap-scan resource opt-out throttles ALL agentic overseer scans,
        // including merge-queue reasoning. #4097 exists to kill SILENT hard-OFFs,
        // so this path must surface WHY reasoning stopped (a loud Disabled status),
        // never leave a silent `Unknown`. A reasoner IS wired; only the opt-out
        // holds it — proving the visibility comes from the opt-out, not an absence.
        let (reasoner, log) = FakeMergeQueueReasoner::with_log(Ok(Some(MQ_BRIEF.to_string())));
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]))
            .with_high_risk_autonomy(true)
            .with_gap_scan_enabled(false) // resource opt-out
            .with_merge_queue_reasoner(vec!["rysweet/Simard".to_string()], Box::new(reasoner), 1);
        let report = ov.run_cycle().expect("cycle");
        assert!(
            matches!(
                report.observed.merge_reasoning_status,
                capabilities::MergeReasoningStatus::Disabled { .. }
            ),
            "the gap-scan opt-out must surface a loud Disabled status, not a silent Unknown; got {:?}",
            report.observed.merge_reasoning_status
        );
        assert!(
            report.observed.reasoned_prs.is_empty(),
            "the opt-out still fabricates no reasoning"
        );
        assert_eq!(
            log.lock().expect("log").len(),
            0,
            "the reasoner recipe is never spawned under the resource opt-out"
        );
    }
}
