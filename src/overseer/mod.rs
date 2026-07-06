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
pub mod config;
pub mod conflict;
pub mod deploy;
pub mod diagnosis;
pub mod failure_sink;
pub mod guardrails;
pub mod intervention;
pub mod launch;
pub mod meeting_ops;
pub mod merge_ops;
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
mod tests_root_cause;
#[cfg(test)]
mod tests_whisper;

pub use capabilities::{
    Auditor, BlockedGoal, Deployer, GoalCurator, IssueFiler, MeetingHost, MemoryRecall,
    MemorySnapshot, ObservationEpisode, ObservedState, OrchestratorRunBrief, OverseerError, PrOps,
    RecallBudget, RecallKeys, RecalledEpisode, RecalledFact, RecalledProcedure,
    RecalledProspective, RecipeBrief, RecipeLauncher, RecordOutcome, StatusReader,
    sanitize_recalled,
};
pub use config::{
    daily_budget_usd, gap_scan_enabled, gap_scan_every_n, goal_health_enabled,
    memory_recall_enabled, overseer_acting_enabled, overseer_author_login, overseer_enabled,
};
pub use diagnosis::{FailureCause, FailureDiagnosis, classify_terminal_failure};
pub use guardrails::{
    AutonomyGate, BudgetGate, ConflictSequencer, RecursionGuard, RiskClass, Subject,
    WhisperDecision, WhisperGate, classify,
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
use capabilities::{DeployReport, GoalBrief, InFlightItem, IssueOutcome, WorkstreamHandle};
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
}

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
            // 15-minute dedup window so a persistent blocked goal is self-healed
            // / escalated once per window (never in a per-tick loop); a generous
            // per-hour cap covers many distinct goals without flooding.
            blocked_goal_gate: WhisperGate::new(900, 20),
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

    /// Enable/disable the whisperer (config opt-out). Off by default; the daemon
    /// sets this from [`config::whisper_enabled`].
    pub fn with_whisper_enabled(mut self, enabled: bool) -> Self {
        self.whisper_enabled = enabled;
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

    /// Wire the cognitive-memory handle (amplihack-memory-lib, G2) used by the
    /// root-cause analysis to recall prior occurrences of a problem's root cause
    /// and record new ones. `None` until wired: the analysis degrades gracefully
    /// to telemetry-only WHYs (never a silent failure).
    pub fn with_memory(mut self, memory: Arc<dyn CognitiveMemoryOps>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Run one meta-OODA turn. Observe → Orient → Decide → plan+gate. Does NOT
    /// execute side effects; returns the plan for M2+ Act to run.
    pub fn run_cycle(&mut self) -> Result<CycleReport, OverseerError> {
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

        // Drain diagnosed step failures (#2640, PART 2) from the process-global
        // failure sink into this Observe pass, so a caught decision-cycle /
        // engineer / terminal-shell failure surfaces as a corrective
        // `Signal::StepFailureDiagnosed` the Orient/Decide loop acts on — a fix,
        // not a silent log line. Draining here (not in the pure
        // `observed_from_snapshot` projection) keeps that projection side-effect
        // free; the sink is bounded so this is O(capacity).
        observed.recent_step_failures = failure_sink::drain_recent();

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

        Ok(CycleReport {
            observed,
            signals,
            problems,
            plan,
            entries,
        })
    }

    /// Run ONE whole-pass, fail-closed cognitive-memory recall (issue #2628):
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

    /// Apply autonomy, budget, and conflict gates to one intervention, producing
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

        // Gap-scan opt-out: when disabled, hold the whole flag-gaps action (no
        // notification, no issue) even though gaps were observed.
        if matches!(iv, Intervention::FlagWorkstreamGaps { .. }) && !self.gap_scan_enabled {
            return held_plan(iv, "held: gap-scan disabled (SIMARD_OVERSEER_GAP_SCAN)");
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
                Ok(ActOutcome::Launched(self.caps.recipes.launch(brief)?))
            }
            Intervention::VerifyAndMergePr { repo, pr } => {
                let report = self.caps.prs.verify(repo, *pr)?;
                if report.ready {
                    self.caps.prs.merge(repo, *pr)?;
                    Ok(ActOutcome::Merged)
                } else {
                    Ok(ActOutcome::Escalated)
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
            } => self.act_escalate_blocked_goal(goal_id, reason, why),
            Intervention::FlagWorkstreamGaps { gaps } => self.act_flag_workstream_gaps(gaps),
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

    /// ESCALATE a genuinely-blocked "needs human review" goal to the operator on
    /// BOTH channels (email + Signal) with the goal id + reason, so the marker
    /// actually reaches a human. Deduped so it never re-fires in a loop; fails
    /// CLOSED without a DISTINCT steward identity (anti-recursion).
    fn act_escalate_blocked_goal(
        &mut self,
        goal_id: &str,
        reason: &str,
        why: &str,
    ) -> Result<ActOutcome, OverseerError> {
        if !self.recursion.is_configured() {
            return Err(OverseerError::Recursion {
                subject: format!(
                    "escalate blocked goal (unconfigured steward identity): {goal_id}"
                ),
            });
        }
        let signature = format!("escalate:{goal_id}");
        let now = now_secs();
        match self.blocked_goal_gate.peek(&signature, now) {
            WhisperDecision::Deliver => {
                let notifier = self.notifier.as_ref().ok_or(OverseerError::Capability {
                    what: "notify.operator",
                    detail: "no operator notifier configured".to_string(),
                })?;
                // Carry the root-cause WHY into the operator notification so a
                // human receives the analysis, not just the bare symptom.
                let notification =
                    OperatorNotification::goal_blocked_with_why(goal_id, reason, why);
                let report = notifier.notify(&notification);
                // Consume the dedup slot only after a dispatch attempt (the
                // notifier itself never drops — it queues/logs on failure).
                self.blocked_goal_gate.commit(&signature, now);
                tracing::info!(
                    target: "overseer::goal_health",
                    goal_id,
                    reason,
                    action = "escalate",
                    dispatched = report.dispatched(),
                    all_sent = report.all_sent(),
                    "overseer escalated a genuinely-blocked needs-human-review goal to the operator"
                );
                Ok(ActOutcome::GoalEscalated {
                    goal_id: goal_id.to_string(),
                })
            }
            other => Ok(Self::goal_health_suppressed("escalate", goal_id, other)),
        }
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

    /// FLAG the backlog-coverage gaps the recurring gap-scan found: notify the
    /// operator on BOTH channels (email + Signal) with ONE consolidated,
    /// provenance-labelled summary AND file one DEDUPED issue per fresh gap via
    /// the SAME M1 stewardship path goal-health uses. Fails CLOSED without a
    /// DISTINCT steward identity (anti-recursion). Deduped per gap signature so a
    /// recurring gap notifies/files at most once per window; the dedup slot is
    /// consumed only after a successful file, so a failure retries rather than
    /// silently losing the gap.
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
        // counted as suppressed and neither re-notified nor re-filed.
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

        // One DEDUPED issue per fresh gap via the M1 stewardship path.
        for g in &fresh {
            let run = OrchestratorRunBrief {
                recipe_name: "smart-orchestrator".to_string(),
                failed_step: "workstream-gap-scan".to_string(),
                source_module: "overseer".to_string(),
                failure_kind: format!("workstream_gap:{}", g.category.label()),
                error_text: format!("{} — {}", g.ref_id, g.why_it_matters),
            };
            self.caps.issues.file(&run)?;
            let sig = format!("workstream-gap:{}", g.signature);
            self.gap_gate.commit(&sig, now);
        }

        tracing::info!(
            target: "overseer::gap_scan",
            flagged = fresh.len(),
            suppressed,
            dispatched = report.dispatched(),
            all_sent = report.all_sent(),
            "overseer flagged uncovered backlog work: notified the operator + filed deduped issue(s)"
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
    fn recall_occurrences(&self, dedup_key: &str) -> Vec<PriorOccurrence> {
        let Some(mem) = self.memory.as_ref() else {
            return Vec::new();
        };
        let concept = occurrence_concept(dedup_key);
        match mem.search_facts(&concept, OCCURRENCE_RECALL_LIMIT, 0.0) {
            Ok(facts) => facts
                .into_iter()
                .filter_map(|f| {
                    serde_json::from_str::<StoredOccurrence>(&f.content)
                        .ok()
                        .filter(|o| o.signature == dedup_key)
                        .map(StoredOccurrence::into_prior)
                })
                .collect(),
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

    /// Record this occurrence's root-cause signature + primary cause + action +
    /// outcome into cognitive memory (amplihack-memory-lib, G2) so a later cycle
    /// recalls it and raises `recurrence`. Best-effort: no memory wired ⇒ no-op;
    /// a store error is `tracing`-logged and swallowed (never fatal to the tick,
    /// never silent).
    fn record_occurrence(&self, entry: &ProblemEntry, outcome: &ActOutcome) {
        let Some(mem) = self.memory.as_ref() else {
            return;
        };
        let Some(primary) = entry.why.primary() else {
            return;
        };
        let record = StoredOccurrence {
            signature: entry.key.clone(),
            cause_label: primary.label.clone(),
            action: entry.action.clone(),
            outcome: describe_outcome(outcome),
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
        let tags = vec![
            entry.key.clone(),
            primary.label.clone(),
            "overseer-root-cause".to_string(),
        ];
        if let Err(e) = mem.store_fact(&concept, &content, 0.9, &tags, "overseer:root-cause") {
            tracing::debug!(
                target: "overseer::root_cause",
                signature = %entry.key,
                cause = %primary.label,
                error = %e,
                "root-cause occurrence store failed (best-effort) — recurrence tracking degraded"
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

/// Stable, deterministic signature for the Overseer's own observation write-back
/// (issue #2628): the sorted, deduped problem `dedup_key`s joined. Two identical
/// observations produce the same signature (so the write-back gate de-dups them);
/// two different observations produce distinct signatures (so both are recorded).
fn observation_signature(problems: &[Problem]) -> String {
    let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    format!("overseer-obs:{}", keys.join("|"))
}

/// The human-readable one-line body of the Overseer's observation write-back.
/// Every problem summary is `sanitize_recalled`-cleaned (defence in depth: the
/// summaries may themselves already carry recalled text) before it enters the
/// episode content that is persisted into the multi-writer memory graph.
fn observation_content(problems: &[Problem]) -> String {
    let parts: Vec<String> = problems
        .iter()
        .map(|p| sanitize_recalled(&p.summary))
        .collect();
    sanitize_recalled(&format!(
        "overseer observed {} problem(s): {}",
        problems.len(),
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
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredOccurrence {
    signature: String,
    cause_label: String,
    action: String,
    outcome: String,
}

impl StoredOccurrence {
    fn into_prior(self) -> PriorOccurrence {
        PriorOccurrence {
            cause_label: self.cause_label,
            action: self.action,
            outcome: self.outcome,
        }
    }
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
        // pass, so it maps to a SINGLE high-priority coverage problem with a
        // stable, evidence-independent dedup key. Uncovered p1/p2 goals, bug/P1
        // issues, and live anomalies all rank High.
        Signal::WorkstreamGap { gaps } => (
            ProblemKind::WorkstreamCoverage,
            Priority::High,
            "workstream-gap".to_string(),
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
    }
}

/// Decide: choose one `Intervention` for a `Problem`. Illustrative routing; a
/// production Overseer would use a prompt-driven reasoner with this deterministic
/// mapping as its floor (mirroring `OodaDecideBrain`'s deterministic fallback).
pub fn decide(problem: &Problem) -> Intervention {
    match problem.kind {
        ProblemKind::DeliveryReady => {
            for s in &problem.evidence {
                if let Signal::PrReadyToMerge { repo, pr } = s {
                    return Intervention::VerifyAndMergePr {
                        repo: repo.clone(),
                        pr: *pr,
                    };
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
                // STABLE primary cause label (for a deduped root-cause issue), so
                // a repeatedly re-parked goal escalates its ROOT CAUSE instead of
                // being blindly re-unblocked.
                let recurrence = problem.why.as_ref().map(|w| w.recurrence).unwrap_or(0);
                let why = problem
                    .why
                    .as_ref()
                    .map(|w| w.to_string())
                    .unwrap_or_default();
                let cause_label = problem
                    .why
                    .as_ref()
                    .and_then(|w| w.primary())
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "unknown-cause".to_string());
                return decide_blocked_goal(
                    goal_id,
                    reason,
                    perpetual,
                    needs_review,
                    recurrence,
                    why,
                    cause_label,
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
        // Backlog-coverage gaps carry the consolidated `WorkstreamGap` evidence
        // forward verbatim so Act can notify + file the specific gaps.
        ProblemKind::WorkstreamCoverage => {
            let gaps = problem
                .evidence
                .iter()
                .find_map(|s| match s {
                    Signal::WorkstreamGap { gaps } => Some(gaps.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            Intervention::FlagWorkstreamGaps { gaps }
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
    }
}

/// Route a blocked goal to the right stewardship action (defense-in-depth for
/// #2609 + the MANDATORY ROOT-CAUSE principle #2635):
///
/// - a RECURRING re-park (memory recall shows the SAME cause re-occurring at or
///   above [`RECURRENCE_ESCALATION_THRESHOLD`]) is NOT blindly re-unblocked every
///   cycle (the operator's rejected antipattern). Instead the ROOT CAUSE is
///   ESCALATED — a deduplicated issue describing *why it keeps getting re-parked*
///   — so the systemic defect is fixed rather than the symptom re-patched;
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
    cause_label: String,
) -> Intervention {
    // Recurring re-park: escalate the ROOT CAUSE (deduped issue), never re-patch.
    // The issue's routing + dedup fields are STABLE across recurrences: the
    // `source_module` routes to Simard (the goal-board/OODA subsystem that
    // re-parks the goal), and neither `failure_kind` nor `error_text` embeds the
    // (ever-changing) recurrence count or the rendered WHY — only the stable
    // `goal_id` + primary `cause_label` — so the same systemic defect is filed
    // ONCE and deduped by `stewardship::failure_signature`, not once per cycle.
    if recurrence >= RECURRENCE_ESCALATION_THRESHOLD {
        return Intervention::FileIssue {
            run: OrchestratorRunBrief {
                recipe_name: "overseer-root-cause".to_string(),
                failed_step: format!("goal-unblock:{goal_id}"),
                source_module: "simard::overseer".to_string(),
                failure_kind: "recurring_goal_reblock".to_string(),
                error_text: format!(
                    "goal `{goal_id}` is repeatedly re-parked despite symptom-level unblocks; \
                     the systemic root cause is `{cause_label}`. The Overseer escalates the root \
                     cause instead of re-patching the symptom every cycle."
                ),
            },
        };
    }
    if perpetual && is_no_progress_marker(&reason) {
        return Intervention::UnblockGoal { goal_id, reason };
    }
    if needs_review {
        return Intervention::EscalateBlockedGoal {
            goal_id,
            reason,
            why,
        };
    }
    Intervention::Report
}

#[cfg(test)]
mod tests {
    use super::capabilities::*;
    use super::*;

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
}
