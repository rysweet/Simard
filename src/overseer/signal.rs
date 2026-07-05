//! The Overseer's `Signal` and `Problem` vocabulary — the Observe/Orient data
//! model. `Signal`s are cheap, additive indicators derived from one Observe pass;
//! Orient folds a set of `Signal`s into ranked, deduplicated `Problem`s.

use crate::overseer::capabilities::ObservedState;

/// A raw, low-level indicator derived from one Observe pass (StatusSnapshot +
/// logs + PR/CI/goal state). Non-authoritative on its own; Orient turns a set of
/// Signals into ranked `Problem`s. Each variant cites the durable field it comes
/// from (see `ObservedState`).
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// Distillation parse-failure rate exceeds threshold (`distill_fail_pct`).
    DistillFailureRate { pct: f64 },
    /// Daemon self-relaunch/restart churn over the window (`restart_churn`).
    RestartChurn { restarts: u64 },
    /// Reasoner/brain decide-ladder exhaustion (`ladder_exhausted`).
    LadderExhausted { count: u64 },
    /// Daily LLM spend approaching/over budget (`spent_today_usd`/`daily_budget_usd`).
    BudgetPressure { spent_usd: f64, budget_usd: f64 },
    /// Engineer spawn/live count elevated (`live_engineers`).
    EngineerSpawnRate { live: u32 },
    /// Cognitive-memory growth beyond expectation (`memory_nodes`).
    MemoryGrowth { nodes_total: u64 },
    /// Gym self-eval skipped (`gym_skipped`).
    GymSkipped,
    /// A cluster of CI failures across recent runs (`ci_failures`).
    CiFailureCluster { repo: String, failing: u32 },
    /// A PR is green + merge-ready and awaiting a merge decision (`ready_prs`).
    PrReadyToMerge { repo: String, pr: u32 },
    /// A goal has been re-litigated / "stale-complete" repeatedly.
    StaleGoal { goal_id: String },
    /// A free-form anomaly surfaced by `TelemetrySignals.anomalies[]`.
    Anomaly { detail: String },
    /// A live goal has gone `consecutive_no_action` cycles without progress —
    /// the primary lightweight-whisper trigger. Fires strictly BELOW Simard's
    /// no-progress breaker so the Overseer can nudge before the hard breaker
    /// trips. From `ObservedState.{consecutive_no_action, active_goal_id}`.
    LoopDetected {
        goal_id: String,
        consecutive_no_action: u32,
    },
    /// Active work appears to be drifting from a goal's stated intent. From
    /// `ObservedState.{drift_detail, active_goal_id}`.
    DriftCorrection { goal_id: String, detail: String },
}

/// Coarse relative importance. `Ord` sorts ascending so `Critical` comes first,
/// mirroring `crate::cognitive_threads::Priority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical,
    High,
    Normal,
    Low,
}

/// Problem family used by Decide to pick an intervention shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// Parse failures, restart churn, ladder exhaustion.
    ProcessHealth,
    /// Budget pressure, engineer-spawn spikes, memory growth.
    ResourcePressure,
    /// A PR ready to merge, or a conflict to resolve.
    DeliveryReady,
    /// CI-failure clusters, gym skipped.
    QualityRegression,
    /// Stale/re-litigated goals.
    GoalHygiene,
    /// Naming/architecture sweeps, terminology cleanups, cross-repo initiatives.
    CrossCutting,
    /// A live goal looping without progress — steered by a lightweight whisper.
    LoopDetected,
    /// Active work drifting from a goal's intent — nudged by an advisory whisper.
    DriftCorrection,
}

/// A classified, deduplicated, prioritised problem — the output of Orient and the
/// input to Decide. Carries the evidence `Signal`s plus a `dedup_key` used to
/// avoid fighting Simard's in-flight work and to avoid duplicate interventions.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub priority: Priority,
    /// Stable dedup key. An adapter should mirror
    /// `crate::stewardship::failure_signature` semantics so the same problem does
    /// not spawn a duplicate workstream or a duplicate issue.
    pub dedup_key: String,
    pub summary: String,
    pub evidence: Vec<Signal>,
}

// Thresholds are illustrative defaults for the sketch; real values would be
// `SIMARD_OVERSEER_*` env knobs clamped to floors (see the design doc).
const DISTILL_FAIL_PCT_THRESHOLD: f64 = 20.0;
const RESTART_CHURN_THRESHOLD: u64 = 3;
const BUDGET_PRESSURE_FRACTION: f64 = 0.8;
const ENGINEER_SPAWN_THRESHOLD: u32 = 8;

/// Consecutive no-action cycles at (or above) which the Overseer whispers a
/// loop-correction. Deliberately BELOW
/// [`crate::goal_curation::no_progress_breaker::NO_PROGRESS_BREAKER_THRESHOLD`]
/// so the lightweight whisper nudges Simard before the hard breaker escalates.
pub const WHISPER_LOOP_THRESHOLD: u32 = 2;

/// Pure Observe→Signal derivation. No I/O; unit-testable with a hand-built
/// `ObservedState`. Real thresholds would be env-tunable.
pub fn signals_from(state: &ObservedState) -> Vec<Signal> {
    let mut out = Vec::new();

    if let Some(pct) = state.distill_fail_pct
        && pct >= DISTILL_FAIL_PCT_THRESHOLD
    {
        out.push(Signal::DistillFailureRate { pct });
    }
    if let Some(restarts) = state.restart_churn
        && restarts >= RESTART_CHURN_THRESHOLD
    {
        out.push(Signal::RestartChurn { restarts });
    }
    if let Some(count) = state.ladder_exhausted
        && count > 0
    {
        out.push(Signal::LadderExhausted { count });
    }
    if let (Some(spent), Some(budget)) = (state.spent_today_usd, state.daily_budget_usd)
        && budget > 0.0
        && spent >= budget * BUDGET_PRESSURE_FRACTION
    {
        out.push(Signal::BudgetPressure {
            spent_usd: spent,
            budget_usd: budget,
        });
    }
    if let Some(live) = state.live_engineers
        && live >= ENGINEER_SPAWN_THRESHOLD
    {
        out.push(Signal::EngineerSpawnRate { live });
    }
    if state.gym_skipped {
        out.push(Signal::GymSkipped);
    }
    for cf in &state.ci_failures {
        out.push(Signal::CiFailureCluster {
            repo: cf.repo.clone(),
            failing: cf.failing,
        });
    }
    for pr in &state.ready_prs {
        out.push(Signal::PrReadyToMerge {
            repo: pr.repo.clone(),
            pr: pr.pr,
        });
    }
    for detail in &state.anomalies {
        out.push(Signal::Anomaly {
            detail: detail.clone(),
        });
    }

    // A live goal looping without progress: whisper trigger. Requires an active
    // goal (idle churn with no goal is not a goal loop to steer).
    if let (Some(n), Some(goal_id)) = (state.consecutive_no_action, state.active_goal_id.as_ref())
        && n >= WHISPER_LOOP_THRESHOLD
    {
        out.push(Signal::LoopDetected {
            goal_id: goal_id.clone(),
            consecutive_no_action: n,
        });
    }
    // Active work drifting from a goal's intent: advisory whisper trigger.
    if let (Some(detail), Some(goal_id)) =
        (state.drift_detail.as_ref(), state.active_goal_id.as_ref())
    {
        out.push(Signal::DriftCorrection {
            goal_id: goal_id.clone(),
            detail: detail.clone(),
        });
    }

    out
}
