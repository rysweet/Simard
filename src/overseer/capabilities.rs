//! Capability traits for the Overseer, each annotated with the EXISTING Simard
//! module/function that satisfies it. These traits are the seam between the
//! Overseer's meta-OODA loop and Simard's already-shipped machinery: an
//! implementation is a thin adapter that calls the cited function, never a
//! reimplementation.
//!
//! Nothing here constructs or runs anything — the module is a design sketch
//! (`#![allow(dead_code)]` in `mod.rs`). The newtypes below are deliberately
//! self-contained so the sketch compiles independently of upstream signature
//! drift; the doc comments carry the precise reuse contract.

use std::fmt;

/// Small, cheap error type shared by every capability. Kept intentionally tiny
/// so `Result<T, OverseerError>` never trips `clippy::result_large_err`.
#[derive(Clone, Debug, PartialEq)]
pub enum OverseerError {
    /// An underlying Simard capability call failed.
    Capability { what: &'static str, detail: String },
    /// A HIGH-RISK intervention was refused by the autonomy gate.
    Gated {
        intervention: String,
        risk: &'static str,
    },
    /// A cost-bearing intervention was refused by the budget gate.
    Budget { spent_usd: f64, budget_usd: f64 },
    /// An intervention was refused because it targets the Overseer's own work.
    Recursion { subject: String },
    /// An intervention was deferred to avoid colliding with in-flight work.
    Conflict { with: String },
}

impl fmt::Display for OverseerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capability { what, detail } => write!(f, "capability {what} failed: {detail}"),
            Self::Gated { intervention, risk } => {
                write!(
                    f,
                    "{intervention} gated (risk={risk}); escalated to operator"
                )
            }
            Self::Budget {
                spent_usd,
                budget_usd,
            } => {
                write!(f, "budget gate: spent ${spent_usd:.2} of ${budget_usd:.2}")
            }
            Self::Recursion { subject } => write!(f, "anti-recursion: refused own {subject}"),
            Self::Conflict { with } => write!(f, "conflict-avoidance: deferred (overlaps {with})"),
        }
    }
}

impl std::error::Error for OverseerError {}

// ─────────────────────────── Observe input ────────────────────────────────

/// The subset of `crate::status::StatusSnapshot` the Overseer reads each Observe
/// pass, flattened into a self-contained value. Every field cites its durable
/// source so an adapter knows exactly which snapshot section to copy.
///
/// **Reuse:** produced by `StatusReader::snapshot`, which wraps
/// `crate::status::assemble(&AssembleOptions)` (`src/status/provider.rs:58`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObservedState {
    /// `StatusSnapshot.telemetry.distill_fail_pct` (`src/status/mod.rs`).
    pub distill_fail_pct: Option<f64>,
    /// `StatusSnapshot.telemetry.restart_churn` + `Daemon.n_restarts`.
    pub restart_churn: Option<u64>,
    /// `StatusSnapshot.memory.decide_ladder_exhausted`.
    pub ladder_exhausted: Option<u64>,
    /// `StatusSnapshot.llm.ledger_today.cost_usd`.
    pub spent_today_usd: Option<f64>,
    /// `StatusSnapshot.llm.daily_budget_usd` (env `SIMARD_DAILY_BUDGET_USD`).
    pub daily_budget_usd: Option<f64>,
    /// `StatusSnapshot.resources.live_engineers`.
    pub live_engineers: Option<u32>,
    /// `StatusSnapshot.memory.nodes_total`.
    pub memory_nodes: Option<u64>,
    /// `StatusSnapshot.gym.skip_gym`.
    pub gym_skipped: bool,
    /// `StatusSnapshot.telemetry.anomalies[]` (free-form strings).
    pub anomalies: Vec<String>,
    /// Merge-ready PRs observed via `PrOps`/`PrGhClient` (`merge_authority.rs`).
    pub ready_prs: Vec<PrRef>,
    /// CI-failure clusters observed across recent runs.
    pub ci_failures: Vec<CiFailure>,
    /// Consecutive OODA cycles the active goal has produced no action / no
    /// progress. Mirrors the OODA no-progress tracker; drives the loop-whisper.
    /// `None` when unknown (e.g. no active goal).
    pub consecutive_no_action: Option<u32>,
    /// The goal Simard is currently working, if any — the subject a whisper
    /// steers. `None` when idle.
    pub active_goal_id: Option<String>,
    /// A short description of observed drift from the active goal's intent, when
    /// the Overseer can see it. `None` when no drift is observed.
    pub drift_detail: Option<String>,
}

/// A `(repo, pr)` pair. `repo` is an `owner/name` slug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrRef {
    pub repo: String,
    pub pr: u32,
}

/// A cluster of failing checks for one repo over the observation window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiFailure {
    pub repo: String,
    pub failing: u32,
}

// ─────────────────────────── Capability briefs ─────────────────────────────

/// Input to `RecipeLauncher::launch`: becomes the `-c task_description=…` context
/// var of a `smart-orchestrator` run. `sequence_group` lets the conflict
/// sequencer serialise mechanical sweeps that touch the same shared files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipeBrief {
    pub task_description: String,
    pub target_repo: String,
    pub sequence_group: Option<String>,
}

/// Handle to a launched workstream (a spawned engineer / recipe subprocess).
///
/// **Reuse:** `crate::agent_supervisor::SubordinateHandle`
/// (`src/agent_supervisor/types.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkstreamHandle {
    pub id: String,
}

/// Terminal / in-flight state of a launched workstream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkstreamStatus {
    Running,
    ProducedPr { repo: String, pr: u32 },
    Failed { reason: String },
}

/// Result of the `pr-verify` checklist (see `prompt_assets/simard/overseer/pr_verify.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub ready: bool,
    pub checks: Vec<CheckItem>,
}

/// One line of the pr-verify checklist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckItem {
    pub name: String,
    pub passed: bool,
    pub note: String,
}

/// Result of a guarded deploy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployReport {
    pub deployed_commit: String,
    pub gates_passed: bool,
}

/// Input to `MeetingHost::transfer_goal` / `GoalCurator::propose`. Mirrors the
/// fields of `crate::meetings::PersistedMeetingGoalUpdate` and a curated goal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalBrief {
    pub title: String,
    pub rationale: String,
    pub priority: u8,
    pub target_repo: String,
}

/// Input to `IssueFiler::file`. Field-for-field a
/// `crate::stewardship::OrchestratorRunSummary` (`src/stewardship/types.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratorRunBrief {
    pub recipe_name: String,
    pub failed_step: String,
    pub source_module: String,
    pub failure_kind: String,
    pub error_text: String,
}

/// Outcome of filing a deduplicated issue.
///
/// **Reuse:** `crate::stewardship::StewardshipOutcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueOutcome {
    FiledNew { url: String },
    MatchedExisting { url: String },
}

/// Scope for a quality-audit run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditScope {
    SelfHealth,
    Repo { slug: String },
    CrossCutting,
}

/// Result of a quality-audit run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditReport {
    pub scope: AuditScope,
    pub passed: bool,
    pub findings: Vec<String>,
}

/// One in-flight goal/workstream the Overseer must dedup/sequence against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlightItem {
    pub id: String,
    pub source: String,
    pub refs: Vec<String>,
}

// ─────────────────────────── Capability traits ─────────────────────────────

/// Assemble the Overseer's primary Observe input.
///
/// **Reuse:** `crate::status::assemble(&AssembleOptions)` (`src/status/provider.rs:58`)
/// returning `crate::status::StatusSnapshot` (`src/status/mod.rs`), plus
/// `crate::cost_tracking::daily_summary` for spend. Read-only; never panics
/// (degraded sections become `None`).
pub trait StatusReader {
    fn snapshot(&self) -> Result<ObservedState, OverseerError>;
}

/// Launch and poll amplihack recipe workstreams — the Overseer's core "drive a
/// fix OUTSIDE Simard's loop" action.
///
/// **Reuse:** `amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml
/// -c task_description=<brief>` exactly as engineers do
/// (`src/bin/simard_engineer_loop_recipe.rs:51`,
/// `src/bin/simard_self_improve_recipe.rs:50`), and the
/// `recipe-runner-rs` + `AMPLIHACK_AGENT_BINARY` pattern in
/// `src/stewardship/recipe_merge_judge.rs:191`. Bound concurrency with
/// `crate::agent_supervisor::spawn_subordinate`
/// (`src/agent_supervisor/lifecycle/spawn.rs:27`, AIMD cap). Parse results with
/// `crate::recipe_output::extract` (`src/recipe_output/extract.rs`).
pub trait RecipeLauncher {
    fn launch(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError>;
    fn poll(&self, handle: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError>;
}

/// Verify, conflict-resolve, and merge pull requests through the gated authority.
///
/// **Reuse:** `crate::stewardship::merge_pr_if_merge_ready`
/// (`src/stewardship/merge_authority.rs:564`) for the merge itself;
/// `evaluate_objective_gates` (`:495`) for CI-green/mergeable/base-allowlist;
/// `crate::review_pipeline::{review_diff, should_commit}` for the review gate;
/// `crate::git_guardrails::check_git_safety` (`src/git_guardrails.rs:41`) around
/// any conflict-resolution push. The Bridge/`print!`/additive/PRD checklist items
/// are NEW additive diff-scans (see the design doc §pr-verify checklist).
pub trait PrOps {
    fn verify(&self, repo: &str, pr: u32) -> Result<VerifyReport, OverseerError>;
    fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError>;
    fn resolve_conflict(&self, repo: &str, pr: u32) -> Result<(), OverseerError>;
}

/// Build, verify, and hand over a new binary — the Overseer's guarded (HIGH-RISK)
/// deploy action.
///
/// **Reuse:** `crate::self_deploy::orchestrator::SelfDeployOrchestrator::run`
/// (`src/self_deploy/orchestrator.rs:229`);
/// `crate::self_relaunch::{build_canary, verify_canary, all_gates_passed,
/// default_gates, handover}`; `crate::safe_update::SafeUpdateOrchestrator`. The
/// deployed-commit marker is `env!("SIMARD_GIT_HASH")` via
/// `self_deploy::health` (`src/self_deploy/health.rs`).
pub trait Deployer {
    fn deploy(&self, commit: &str) -> Result<DeployReport, OverseerError>;
    fn deployed_commit(&self) -> Result<String, OverseerError>;
}

/// Transfer/handoff a goal to Simard via the meeting REPL.
///
/// **Reuse:** `crate::meeting_repl::run_meeting_repl`
/// (`src/meeting_repl/repl.rs:211`); `crate::meeting_facilitator` handoff
/// (`MeetingHandoff`, `write_meeting_handoff`);
/// `crate::meetings::PersistedMeetingGoalUpdate`.
pub trait MeetingHost {
    fn transfer_goal(&self, goal: &GoalBrief) -> Result<(), OverseerError>;
}

/// File a deduplicated GitHub issue for a recurring failure (stewardship mode).
///
/// **Reuse:** `crate::stewardship::process_orchestrator_run`
/// (`src/stewardship/mod.rs:51`) with `OrchestratorRunSummary`; dedup via
/// `crate::stewardship::{failure_signature, find_existing}`
/// (`src/stewardship/dedup.rs`); backlog enqueue via
/// `crate::goal_curation::enqueue_stewardship_issue`.
pub trait IssueFiler {
    fn file(&self, run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError>;
}

/// Propose/curate goals and read the board for dedup/conflict checks.
///
/// **Reuse:** `crate::goal_curation::{load_goal_board, promote_to_active,
/// save_goal_board}`; the flock write-lock `BoardWriteLock` (#2514,
/// `src/goal_curation/operations.rs:190`); `MAX_ACTIVE_GOALS`.
pub trait GoalCurator {
    fn propose(&self, goal: &GoalBrief) -> Result<(), OverseerError>;
    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError>;
}

/// Run a quality-audit loop (crusty-old-engineer-gated).
///
/// **Reuse:** `crate::self_quality_audit::run_self_quality_audit`
/// (`src/self_quality_audit.rs`) and the quality-audit recipe
/// `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`.
pub trait Auditor {
    fn run_audit(&self, scope: &AuditScope) -> Result<AuditReport, OverseerError>;
}
