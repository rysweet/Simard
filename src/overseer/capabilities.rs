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

use crate::overseer::diagnosis::FailureDiagnosis;
use crate::overseer::signal::{GapItem, Problem, Signal};

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
    /// The authoritative merge-readiness review did not approve the PR (the
    /// agentic judge refused, or no LLM provider was available so it fails
    /// closed). This is NOT an error — the Act handler maps it to an operator
    /// escalation. Never merge blindly on this outcome.
    NotMergeReady { pr: u32, reason: String },
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
            Self::NotMergeReady { pr, reason } => {
                write!(f, "PR #{pr} is not merge-ready yet: {reason}")
            }
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
    /// Blocked goals observed on Simard's goal board this Observe pass — the
    /// goal-board *health* signal. Populated by the sensor from the durable
    /// board markers (`BLOCKED`, the `🔒 [OODA-SAFEGUARD] … needs human review`
    /// no-progress marker, and the brain-failure marker). Empty when the board
    /// is clean or unreadable (degrade-to-empty, never a panic).
    pub blocked_goals: Vec<BlockedGoal>,
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
    /// The bounded cognitive-memory recall for this Observe pass (issue #2628).
    /// `None` when recall is disabled or has not run; `Some(empty)` when the
    /// graph had nothing relevant (a valid, successful result). Populated by the
    /// Overseer's whole-pass recall via [`MemoryRecall`]; consumed by
    /// `signals_from`/Orient to detect a recurring signature. Kept **distinct**
    /// from [`recall_error`](Self::recall_error) so an empty graph is never
    /// confused with a swallowed error.
    pub recall: Option<MemorySnapshot>,
    /// Set to the surfaced error string when recall **failed** this pass (issue
    /// #2628). Kept separate from [`recall`](Self::recall) — which stays `None`
    /// on failure — so callers, the tick report, and tests can always tell an
    /// empty graph from an unreachable one (no silent fallback).
    pub recall_error: Option<String>,
    /// Genuine backlog-coverage gaps surfaced this Observe pass — important work
    /// that SHOULD have an active workstream but does not (uncovered high-priority
    /// goals, high-signal issues with no PR, live anomalies with no fix in
    /// flight). Populated by the sensor's [`sensor::detect_workstream_gaps`]
    /// projection via [`GoalCurator::workstream_gaps`], already correlated against
    /// in-flight workstreams / open PRs so only GENUINE gaps appear. Empty when
    /// the backlog is fully covered or unreadable (degrade-to-empty, never a
    /// panic).
    ///
    /// [`sensor::detect_workstream_gaps`]: crate::overseer::sensor::detect_workstream_gaps
    pub workstream_gaps: Vec<GapItem>,
    /// The AGENTIC merge-queue reasoning brief's per-PR conclusions (#4097). Each
    /// [`ReasonedPr`] is a REASONING PROPOSAL — never itself an authorization to
    /// merge. Populated by the agentic `observe-merge-queue` recipe pass over the
    /// governed roster, parsed FAIL-CLOSED (`merge_queue_observe::parse_merge_queue_brief`).
    /// NON-EMPTY even when `SIMARD_AUTOMERGE_REPOS`/`SIMARD_AUTOMERGE_AUTHOR` are
    /// unset — reasoning is DEFAULT-ON over the roster; only the re-narrowed
    /// [`ready_prs`](Self::ready_prs) projection authorizes a merge. `signals_from`
    /// lifts `Stale`/`Duplicate` dispositions into `Signal::StalePrDetected` /
    /// `Signal::DuplicatePrDetected` (gated `FlagStalePr` / `CloseDuplicatePr`).
    pub reasoned_prs: Vec<ReasonedPr>,
    /// The AGENTIC merge-queue reasoning brief's per-ISSUE triage (#4097).
    /// Populated by the same agentic pass; parsed FAIL-CLOSED against the roster
    /// trust boundary. A `Ready` triaged issue becomes a
    /// `Signal::IssueNeedsWorkstream` so backlog work with no active workstream is
    /// surfaced into Decide (never merges anything — it proposes a workstream).
    pub triaged_issues: Vec<TriagedIssue>,
    /// WHY merge reasoning is (or is not) running this pass (#4097, R3). Default
    /// [`MergeReasoningStatus::Unknown`] (additive — existing constructors compile
    /// unchanged). Set to [`MergeReasoningStatus::Disabled`] with the raw reason
    /// ONLY when an operator EXPLICITLY disabled reasoning
    /// (`SIMARD_MERGE_REASONING_SCOPE=off`), so a disable is LOUD (WARN log +
    /// surfaced status), never a silent OFF. Unset env is NOT disabled.
    pub merge_reasoning_status: MergeReasoningStatus,
    /// WHAT the agentic health-review pass concluded this tick ([standing]).
    /// Default [`HealthReviewStatus::NotRun`] (additive — existing constructors
    /// compile unchanged), left unchanged when the rail is unwired, the
    /// dedicated opt-out disabled it, or the pass is off-cadence this tick. A
    /// pass that RAN and parsed a verdict sets [`HealthReviewStatus::Reviewed`]
    /// with the agent's one-line `HEALTH_REVIEW_COMPLETE` summary + the count of
    /// typed decisions it drove — so a HEALTHY pass (zero interventions) still
    /// leaves an OBSERVABLE trace instead of a silent no-op, exactly as
    /// [`merge_reasoning_status`](Self::merge_reasoning_status) surfaces WHY
    /// reasoning ran. A pass that RAN but DEGRADED (a truncated report the
    /// bounded escalation ladder could not recover, or a base infra fault) sets
    /// [`HealthReviewStatus::Degraded`] so the weak pass is LOUD, never a silent
    /// OFF. Set by the acting Overseer's `health_review` pass; never fabricated.
    pub health_review_status: HealthReviewStatus,
    /// Structured diagnoses of decision-cycle / engineer / terminal-shell steps
    /// that failed since the last Observe pass (issue #2640, PART 2). The acting
    /// Overseer drains these from the process-global failure sink
    /// ([`crate::overseer::failure_sink::drain_recent`]) each cycle;
    /// `signals_from` lifts each into a corrective `Signal::StepFailureDiagnosed`
    /// so a caught failure drives a fix instead of a silent log. Empty when no
    /// step failed this window.
    pub recent_step_failures: Vec<FailureDiagnosis>,
    /// Autonomous self-deploy drift observed this pass (issue #2590): the running
    /// daemon binary is behind merged `origin/main` and a target commit resolved.
    /// Populated by the acting Overseer's drift-observe rail (fail-safe: a git or
    /// source error, a current daemon, or an unresolved head all leave this
    /// `None`). `signals_from` lifts it into a `Signal::DeployDriftDetected` that
    /// Decide maps to a guarded `Intervention::Deploy`. `None` when there is
    /// nothing to deploy or the rail is disabled/unwired.
    pub deploy_drift: Option<DeployDriftObservation>,
}

/// The self-deploy drift the Overseer observed this pass (issue #2590): the
/// running binary is behind merged `main`, and `target_commit` is the resolved
/// merged head a guarded deploy should converge on. Carried on
/// [`ObservedState::deploy_drift`] so `signals_from` can lift it into a
/// [`Signal::DeployDriftDetected`] purely (the effectful git probe stays in the
/// observe rail).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployDriftObservation {
    /// Merged-head commit the daemon should deploy to (a non-empty git rev).
    pub target_commit: String,
    /// Commits the running binary is behind `origin/main` (`0` when only pins
    /// drifted).
    pub behind_commits: usize,
}

/// A `(repo, pr)` pair. `repo` is an `owner/name` slug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrRef {
    pub repo: String,
    pub pr: u32,
}

/// The agentic reviewer's disposition for one open PR (#4097). A REASONING
/// PROPOSAL, never an authorization: `ReadyForMerge` says "this looks ready",
/// but only the re-narrowing [`project_ready_prs`](crate::overseer::project_ready_prs)
/// projection — author guard + engineer-PR narrowing + objective gates —
/// authorizes a merge. Unknown disposition strings are DROPPED at parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrDisposition {
    /// The reviewer judges the PR ready for merge action. A PROPOSAL only.
    ReadyForMerge,
    /// The PR needs more work (failing checks, requested changes, conflicts).
    NeedsWork,
    /// The PR is stale (no activity for a long window) and should be flagged.
    Stale,
    /// The PR duplicates another (see [`ReasonedPr::duplicate_of`]) and should be
    /// closed with a reference to the original.
    Duplicate,
}

/// The agentic reviewer's per-PR reasoning conclusion (#4097). Bounded, parsed
/// FAIL-CLOSED from the opaque brief. Off-roster `repo`s are dropped (the roster
/// is the reasoning trust boundary); a `Duplicate` with no `duplicate_of` is
/// incoherent and dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasonedPr {
    /// `owner/name` slug — MUST be on the governed roster or the entry is dropped.
    pub repo: String,
    pub pr: u32,
    pub disposition: PrDisposition,
    /// One-line agentic rationale (bounded). Advisory prose only.
    pub rationale: String,
    /// For [`PrDisposition::Duplicate`], the PR number this one duplicates.
    /// `Some` is REQUIRED for a coherent `Duplicate`; `None` otherwise.
    pub duplicate_of: Option<u32>,
}

/// The agentic reviewer's triage priority for one open issue (#4097).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssuePriority {
    High,
    Medium,
    Low,
}

/// The agentic reviewer's readiness verdict for one open issue (#4097).
/// `Ready` means actionable NOW (a workstream can start); `Blocked` means it is
/// waiting on something and must NOT spawn a workstream this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueReadiness {
    Ready,
    Blocked,
    /// Needs clarification / more information before it is actionable.
    NeedsInfo,
}

/// The agentic reviewer's per-ISSUE triage conclusion (#4097). Bounded, parsed
/// FAIL-CLOSED; off-roster `repo`s are dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriagedIssue {
    /// `owner/name` slug — MUST be on the governed roster or the entry is dropped.
    pub repo: String,
    pub issue: u32,
    pub priority: IssuePriority,
    pub readiness: IssueReadiness,
    /// The concrete next action, in plain English (bounded).
    pub next_action: String,
}

/// WHY merge-queue reasoning is (or is not) running (#4097, R3). The distinction
/// the fix hinges on: an UNSET scope env is NOT disabled (reasoning defaults ON
/// over the roster). Only an EXPLICIT operator disable produces
/// [`Self::Disabled`], which is surfaced LOUD (WARN log + this status) so
/// `simard status` can name WHY reasoning is off — never a silent OFF.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MergeReasoningStatus {
    /// Not yet resolved this pass (the additive default so existing constructors
    /// compile unchanged).
    #[default]
    Unknown,
    /// Reasoning ran over the full governed roster (unset scope — default-ON).
    RosterWide,
    /// Reasoning ran over an operator-narrowed explicit scope.
    Narrowed { repos: Vec<String> },
    /// An operator EXPLICITLY disabled reasoning. `reason` carries the raw signal
    /// (e.g. `SIMARD_MERGE_REASONING_SCOPE=off`) so the disable is loud.
    Disabled { reason: String },
}

/// WHAT the agentic Overseer health-review pass concluded this tick ([standing]).
///
/// Mirrors [`MergeReasoningStatus`]: the distinction the observability hinges on
/// is that a pass that RAN and found nothing wrong is NOT the same as a pass
/// that never ran. Default [`Self::NotRun`] (the additive default) covers an
/// unwired / disabled / off-cadence tick; a pass that produced an honest verdict
/// sets [`Self::Reviewed`] (so a HEALTHY pass leaves an observable trace, never a
/// silent no-op); and a pass that degraded to no remediation sets
/// [`Self::Degraded`] so the weak pass is LOUD in status rather than a silent OFF.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum HealthReviewStatus {
    /// No pass ran this tick: the rail is unwired, the dedicated opt-out disabled
    /// it, or it is off-cadence (the additive default so existing constructors
    /// compile unchanged).
    #[default]
    NotRun,
    /// A pass RAN and parsed an honest verdict. `summary` is the recipe's
    /// one-line `HEALTH_REVIEW_COMPLETE` text; `decisions` is the count of typed
    /// remediation interventions it drove (`0` on a HEALTHY pass — an observable
    /// "reviewed, nothing to do", never a fabricated action).
    Reviewed { summary: String, decisions: usize },
    /// A pass RAN but DEGRADED end to end (a truncated report the bounded
    /// escalation ladder could not recover, or a base infra fault) and took no
    /// remediation. Surfaced so the weak pass is LOUD, never a silent OFF.
    Degraded,
}

/// A cluster of failing checks for one repo over the observation window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiFailure {
    pub repo: String,
    pub failing: u32,
}

/// One blocked goal observed on Simard's goal board — the unit of goal-board
/// *health* the Overseer observes and (in the acting loop) acts on.
///
/// Derived from a `GoalProgress::Blocked(reason)` on the live board by the
/// sensor's pure projection (`sensor::blocked_goals_from_board`). It reuses the
/// EXISTING standing/perpetual detection (`ActiveGoal::is_perpetual`, #2589/#2609)
/// and the EXISTING safeguard-marker predicates
/// (`goal_curation::no_progress_breaker::is_no_progress_marker`,
/// `ooda_actions::advance_goal::is_brain_failure_marker`) — it never invents a
/// second notion of either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedGoal {
    /// The blocked goal's id on the active board.
    pub id: String,
    /// The verbatim `GoalProgress::Blocked` reason string (carries the marker).
    pub reason: String,
    /// True when the goal is standing/perpetual (`ActiveGoal::is_perpetual`).
    pub perpetual: bool,
    /// True when the block carries a "needs human review" safeguard marker
    /// (the no-progress OODA-SAFEGUARD marker or the brain-failure marker) —
    /// the signal that a human must be reached.
    pub needs_review: bool,
    /// Consecutive no-action / no-progress cycles parsed from the safeguard
    /// marker, or `0` when the block is not a counted safeguard marker.
    pub consecutive_no_action: u32,
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
/// any conflict-resolution push. The no-`memory`-naming/`print!`/additive/PRD checklist items
/// are NEW additive diff-scans (see the design doc §pr-verify checklist).
pub trait PrOps {
    fn verify(&self, repo: &str, pr: u32) -> Result<VerifyReport, OverseerError>;
    fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError>;
    fn resolve_conflict(&self, repo: &str, pr: u32) -> Result<(), OverseerError>;

    /// Survey the given `owner/name` repos for Simard's OWN green + MERGEABLE
    /// PRs and return them as a candidate list (issue #4097). This is the THIN
    /// DETERMINISTIC sensor rail that populates
    /// [`ObservedState::ready_prs`](crate::overseer::capabilities::ObservedState),
    /// re-activating the dormant `PrReadyToMerge`/`VerifyAndMergePr` machinery.
    ///
    /// It is a candidate LIST only — a cheap author-filter + the already-fetched
    /// `mergeable`/`statusCheckRollup` objective pre-filter. It NEVER merges and
    /// NEVER runs the MergeJudge: the authoritative six-criteria gate stays
    /// downstream in `merge_authority` and remains the single source of merge
    /// truth. The default is EMPTY (default-off, fail-closed) so an
    /// implementation that has not opted in performs no autonomous merge.
    fn survey_ready_prs(&self, _repos: &[String]) -> Vec<PrRef> {
        Vec::new()
    }

    /// Flag an open PR judged STALE by the agentic merge-queue reasoner (#4097)
    /// with a `gh pr comment` (see [`crate::overseer::intervention::flag_stale_pr_argv`]).
    /// NEVER merges, NEVER closes. The default fails CLOSED so an implementation
    /// that has not wired the `gh` seam performs no autonomous hygiene action.
    fn flag_stale_pr(&self, _repo: &str, _pr: u32, _note: &str) -> Result<(), OverseerError> {
        Err(OverseerError::Capability {
            what: "flag_stale_pr",
            detail: "no PR-ops gh seam wired".to_string(),
        })
    }

    /// Close an open PR judged a DUPLICATE by the agentic merge-queue reasoner
    /// (#4097) with `gh pr close` referencing the original (see
    /// [`crate::overseer::intervention::close_duplicate_pr_argv`]). NEVER merges,
    /// NEVER `--admin`/`--no-verify`. The default fails CLOSED.
    fn close_duplicate_pr(
        &self,
        _repo: &str,
        _pr: u32,
        _duplicate_of: u32,
    ) -> Result<(), OverseerError> {
        Err(OverseerError::Capability {
            what: "close_duplicate_pr",
            detail: "no PR-ops gh seam wired".to_string(),
        })
    }

    /// RE-NARROW the agentic reasoner's `ReadyForMerge` proposals into the set of
    /// PRs actually authorized to merge (#4097), the DETERMINISTIC safety rail.
    ///
    /// The agentic observe/orient pass reasons BROADLY over the governed roster
    /// and PROPOSES a [`PrDisposition::ReadyForMerge`] for some PRs. That proposal
    /// alone NEVER authorizes a merge. This method fetches the AUTHORITATIVE `gh`
    /// facts (author, head branch, mergeable/checks/base/labels) for each proposed
    /// PR and runs [`project_ready_prs`](crate::overseer::project_ready_prs): the
    /// author-guard + engineer-PR narrowing + objective gates. Only survivors are
    /// returned — they populate `ObservedState.ready_prs`, the ONLY thing that
    /// drives `Signal::PrReadyToMerge` into the gated merge chain.
    ///
    /// The default returns EMPTY (fail-closed) so an impl that has not wired the
    /// authoritative `gh` seam authorizes no autonomous merge. The agent can never
    /// widen merge authorization; it can only ever propose candidates this rail
    /// then re-verifies against ground truth.
    fn project_reasoned_ready_prs(
        &self,
        _reasoned: &[ReasonedPr],
        _overseer_login: &str,
    ) -> Vec<PrRef> {
        Vec::new()
    }
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
/// (`src/stewardship/dedup.rs`). Filing never writes the resulting issue back
/// into the goal board.
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

    /// Read the goal-board *health* — the goals currently `Blocked` — so the
    /// Observe pass can surface them into [`ObservedState::blocked_goals`].
    ///
    /// **Reuse:** `crate::goal_curation::load_goal_board` projected by
    /// `crate::overseer::sensor::blocked_goals_from_board`. Read-only; a board
    /// read failure degrades to an empty list, never a panic. The default
    /// returns an empty list for fakes that do not model a board.
    fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
        Ok(Vec::new())
    }

    /// Observe the goal board **once** and project both the blocked-goal health
    /// list ([`blocked_goals`](Self::blocked_goals)) and the in-flight dedup set
    /// ([`in_flight`](Self::in_flight)) from a single read.
    ///
    /// The Observe/Orient pass needs both projections every tick; reading them
    /// through the two single-projection methods loads (and JSON-deserializes)
    /// the very same board snapshot twice. Real adapters override this to load
    /// once and project twice. The default composes the two methods so fakes
    /// that do not model a shared board need no change.
    ///
    /// Returns `(blocked_goals, in_flight)`.
    fn observe_board(&self) -> Result<(Vec<BlockedGoal>, Vec<InFlightItem>), OverseerError> {
        Ok((self.blocked_goals()?, self.in_flight()?))
    }

    /// Survey the WHOLE work picture for backlog-coverage GAPS — important work
    /// that SHOULD have an active workstream but does not — for the recurring
    /// "WHAT WORKSTREAMS ARE WE MISSING?" gap-scan, already correlated against
    /// in-flight workstreams + open PRs so only GENUINE gaps are returned.
    /// `anomalies` are the live telemetry anomalies from this Observe pass (so an
    /// anomaly with no fix in flight can be flagged); the adapter surveys the goal
    /// board + high-signal open issues itself.
    ///
    /// **Reuse:** `crate::goal_curation::load_goal_board` projected by
    /// [`crate::overseer::sensor::detect_workstream_gaps`], plus a best-effort
    /// `gh issue list` label survey and the open-PR list for correlation. Read-
    /// only; any survey failure degrades to an empty list (logged), never a panic.
    /// The default returns an empty list for fakes that do not model a board.
    fn workstream_gaps(&self, _anomalies: &[String]) -> Result<Vec<GapItem>, OverseerError> {
        Ok(Vec::new())
    }

    /// Auto-unblock + reactivate a false-parked goal — the exact operation
    /// `simard goal unblock` performs: restore a `Blocked` goal to `NotStarted`
    /// so the next OODA cycle re-enters the spawn path.
    ///
    /// **Reuse:** the `simard goal unblock` board mutation
    /// (`src/operator_cli/goal.rs`) via `load_goal_board` → set status
    /// `NotStarted` → `save_goal_board`. The default is a no-op for fakes that
    /// do not model a board (real adapters override it).
    fn unblock(&self, _goal_id: &str) -> Result<(), OverseerError> {
        Ok(())
    }
}

/// Run a quality-audit loop (crusty-old-engineer-gated).
///
/// **Reuse:** `crate::self_quality_audit::run_self_quality_audit`
/// (`src/self_quality_audit.rs`) and the quality-audit recipe
/// `prompt_assets/simard/recipes/monthly-self-quality-audit.yaml`.
pub trait Auditor {
    fn run_audit(&self, scope: &AuditScope) -> Result<AuditReport, OverseerError>;
}

// ───────────────────── cognitive-memory recall (#2628) ─────────────────────
//
// The Overseer's bounded READ access to Simard's cognitive-memory graph plus
// one deliberate, de-duplicated episodic WRITE-back, as a first-class part of
// its Observe/Orient loop. Every type here is an owned, self-contained
// projection so signal derivation stays pure; the concrete adapter
// (`wiring::MemoryRecallOps`) is a thin reuse of the already-shipped
// `CognitiveMemoryOps` handle the daemon already shares — never a second store,
// never a reimplementation of memory logic (guideline G2: no new memory-lib
// API is required).

/// Maximum length (bytes) any single recalled/derived text may reach before it
/// is allowed to egress (a `Problem.summary`, a log line, an operator
/// notification). Bounds log/notification-injection blast radius.
pub const RECALLED_TEXT_MAX_LEN: usize = 8192;

/// Sanitize a piece of **untrusted** recalled/derived text before it may reach
/// any egress surface (a `Problem.summary`, a `tracing` field, an operator
/// notification). Simard's cognitive-memory graph is multi-writer, so recalled
/// content is untrusted input: this is the single admission boundary that
/// neutralises log/notification injection and header spoofing.
///
/// It (1) replaces every control character — including `CR`, `LF`, `TAB`, and
/// ANSI `ESC` — with a single space so no newline or control byte survives, and
/// (2) caps the result at [`RECALLED_TEXT_MAX_LEN`] bytes on a UTF-8 boundary so
/// a huge recalled blob can never flood a log or a notification. Plain text
/// (no control chars, within the cap) is returned unchanged.
pub fn sanitize_recalled(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(RECALLED_TEXT_MAX_LEN));
    for c in s.chars() {
        // Reserve room on a UTF-8 boundary so the cap is never exceeded.
        if out.len() + c.len_utf8() > RECALLED_TEXT_MAX_LEN {
            break;
        }
        if c.is_control() {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Hard, constant per-kind caps the recall pass enforces so recall can never fan
/// out into an unbounded read. Budgets are constants (not env knobs): bounding
/// result **size** — plus the panic-isolated tick — is what keeps recall
/// non-blocking, since the calls are in-process against the shared store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecallBudget {
    pub semantic: u32,
    pub episodic: u32,
    pub procedural: u32,
    pub prospective: u32,
}

impl Default for RecallBudget {
    /// The library-balanced default: `5 / 5 / 3 / 5`.
    fn default() -> Self {
        Self {
            semantic: 5,
            episodic: 5,
            procedural: 3,
            prospective: 5,
        }
    }
}

/// The keyword sets the Overseer recalls against, derived from the cycle's
/// Signals and Problems — **never** a full-graph scan. Mirrors
/// `crate::stewardship::failure_signature` semantics so the recall key and the
/// stewardship dedup key line up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecallKeys {
    /// Free-text keywords built from the detected Signals/Problems (e.g.
    /// `distill_fail`, `restart_churn`, a blocked-goal id). Deduped + sorted so
    /// recall is reproducible.
    pub keywords: Vec<String>,
    /// Stable `failure_signature`-style keys, one per Problem, used both to
    /// query episodes and to detect a recurring signature on recall.
    pub signatures: Vec<String>,
}

impl RecallKeys {
    /// Derive the recall keys from this cycle's `signals` and `problems`. The
    /// keyword set comes from the signals (one stable token per variant); the
    /// signatures are the problems' `dedup_key`s (already `failure_signature`
    /// shaped). Both are deduped and sorted so recall is deterministic.
    pub fn from_signals(signals: &[Signal], problems: &[Problem]) -> Self {
        let mut keywords: Vec<String> = signals.iter().filter_map(signal_keyword).collect();
        keywords.sort();
        keywords.dedup();

        let mut signatures: Vec<String> = problems.iter().map(|p| p.dedup_key.clone()).collect();
        signatures.sort();
        signatures.dedup();

        Self {
            keywords,
            signatures,
        }
    }

    /// A single deterministic query string joining every keyword and signature —
    /// the shape the underlying keyword/ranked recalls (and the single-`&str`
    /// `check_triggers` probe) consume. Order-stable because the fields are
    /// already sorted.
    pub fn query(&self) -> String {
        let mut terms: Vec<&str> = self.keywords.iter().map(String::as_str).collect();
        terms.extend(self.signatures.iter().map(String::as_str));
        terms.join(" ")
    }
}

/// One stable keyword for a signal variant (values elided). `None` for variants
/// that carry no useful recall key on their own.
fn signal_keyword(s: &Signal) -> Option<String> {
    let kw = match s {
        Signal::DistillFailureRate { .. } => "distill_fail".to_string(),
        Signal::RestartChurn { .. } => "restart_churn".to_string(),
        Signal::LadderExhausted { .. } => "ladder_exhausted".to_string(),
        Signal::BudgetPressure { .. } => "budget_pressure".to_string(),
        Signal::EngineerSpawnRate { .. } => "engineer_spawn".to_string(),
        Signal::MemoryGrowth { .. } => "memory_growth".to_string(),
        Signal::GymSkipped => "gym_skipped".to_string(),
        Signal::CiFailureCluster { repo, .. } => format!("ci:{repo}"),
        Signal::PrReadyToMerge { repo, pr } => format!("pr:{repo}#{pr}"),
        Signal::StaleGoal { goal_id } => format!("goal:{goal_id}"),
        Signal::Anomaly { detail } => format!("anomaly:{detail}"),
        Signal::LoopDetected { goal_id, .. } => format!("loop:{goal_id}"),
        Signal::DriftCorrection { goal_id, .. } => format!("drift:{goal_id}"),
        Signal::GoalBlocked { goal_id, .. } => format!("blocked:{goal_id}"),
        Signal::RecurringSignature { signature, .. } => signature.clone(),
        // A consolidated batch of gaps carries no single recall key on its own.
        Signal::WorkstreamGap { .. } => String::new(),
        // Recurring step failures are keyed on their root cause, so a repeated
        // failure mode (e.g. arg-list-too-long) recalls prior diagnoses.
        Signal::StepFailureDiagnosed { cause, .. } => format!("step-failure:{}", cause.as_str()),
        Signal::StalePrDetected { repo, pr } => format!("stale-pr:{repo}#{pr}"),
        Signal::DuplicatePrDetected { repo, pr, .. } => format!("dup-pr:{repo}#{pr}"),
        Signal::IssueNeedsWorkstream { repo, issue, .. } => format!("issue-ws:{repo}#{issue}"),
        Signal::DeployDriftDetected { .. } => "deploy-drift".to_string(),
    };
    if kw.is_empty() { None } else { Some(kw) }
}

/// The bundle of recalled results for one Observe pass, stored on
/// [`ObservedState::recall`] and consumed by `signals_from`/Orient. An empty
/// snapshot is a valid, successful result (the graph had nothing relevant); it
/// is **distinct** from a recall error.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemorySnapshot {
    pub facts: Vec<RecalledFact>,
    pub episodes: Vec<RecalledEpisode>,
    pub procedures: Vec<RecalledProcedure>,
    pub prospectives: Vec<RecalledProspective>,
}

/// A flattened, owned projection of a semantic fact. All free text is untrusted
/// input (see the security model in the reference doc).
#[derive(Clone, Debug, PartialEq)]
pub struct RecalledFact {
    pub id: String,
    /// Concept / prior root-cause — untrusted text.
    pub content: String,
    /// Ranking score from the underlying ranked recall.
    pub score: f32,
}

/// A flattened, owned projection of an episodic memory.
#[derive(Clone, Debug, PartialEq)]
pub struct RecalledEpisode {
    pub id: String,
    /// Untrusted summary text.
    pub summary: String,
    /// Parsed `failure_signature` — the LOAD-BEARING key Orient counts to raise a
    /// [`Signal::RecurringSignature`]. `None` when the episode carried no
    /// signature.
    pub failure_signature: Option<String>,
    pub score: f32,
}

/// A flattened, owned projection of a procedural runbook. Advisory-only egress.
#[derive(Clone, Debug, PartialEq)]
pub struct RecalledProcedure {
    pub id: String,
    /// Stored runbook text — untrusted; advisory-only.
    pub content: String,
}

/// A flattened, owned projection of a prospective memory / deferred idea.
#[derive(Clone, Debug, PartialEq)]
pub struct RecalledProspective {
    pub id: String,
    /// Deferred-intention text — untrusted.
    pub content: String,
}

/// The Overseer's deliberate episodic write-back payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationEpisode {
    /// Human-readable one-line summary of what the Overseer observed/decided.
    pub content: String,
    /// The observation signature this episode is keyed on — also the de-dup key
    /// for the write-back gate.
    pub signature: String,
}

/// Outcome of a deliberate write-back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// A new episode was written; carries the new node id.
    Stored { node_id: String },
    /// An identical-signature observation was written within the dedup window;
    /// nothing was persisted this tick.
    Deduplicated,
}

/// Bounded READ access to Simard's cognitive-memory graph, plus one deliberate,
/// de-duplicated episodic WRITE-back. Every read method is **fail-closed**: an
/// underlying memory error is returned as `OverseerError::Capability`, never
/// collapsed into an empty result (that would be a silent fallback). Each read
/// takes a single-kind `limit` (the caller passes the matching field of
/// [`RecallBudget`]) so no method ever sees budget fields it does not use.
///
/// **Reuse:** the production adapter (`crate::overseer::wiring::MemoryRecallOps`)
/// maps each method onto an already-shipped
/// [`CognitiveMemoryOps`](crate::cognitive_memory::CognitiveMemoryOps) query over
/// the daemon's single shared `Arc` handle — no new memory-library API.
pub trait MemoryRecall: Send + Sync {
    /// Recall up to `limit` semantic facts relevant to `keys` (concepts, prior
    /// root-causes).
    fn recall_semantic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledFact>, OverseerError>;

    /// Recall up to `limit` episodic memories relevant to `keys` (prior
    /// occurrences of a problem and their outcomes). Carries each episode's
    /// failure signature so Orient can detect a recurring signature.
    fn recall_episodic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledEpisode>, OverseerError>;

    /// Recall up to `limit` procedural runbooks relevant to `keys`. Surfaced by
    /// Decide when a recurring signature is seen.
    fn recall_procedural(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProcedure>, OverseerError>;

    /// Recall up to `limit` prospective memories / ideas whose triggers match
    /// `keys` (deferred intentions the current situation should re-surface).
    fn recall_prospective(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProspective>, OverseerError>;

    /// Write the Overseer's own observation back as one episodic memory. Returns
    /// whether it was stored or (in a backend that models its own dedup)
    /// suppressed; the Overseer additionally gates this call so a repeated
    /// signature within the window never reaches the backend at all.
    fn record_observation(
        &self,
        episode: &ObservationEpisode,
    ) -> Result<RecordOutcome, OverseerError>;
}

/// An inert [`MemoryRecall`] for tests that do not exercise the memory seam:
/// every read returns an empty result and the write-back is a no-op. Only used
/// by capability-constructor helpers in other `overseer` test modules (their
/// Overseers leave recall disabled, so this handle is never actually queried).
#[cfg(test)]
pub(crate) struct InertMemoryRecall;

#[cfg(test)]
impl MemoryRecall for InertMemoryRecall {
    fn recall_semantic(
        &self,
        _keys: &RecallKeys,
        _limit: u32,
    ) -> Result<Vec<RecalledFact>, OverseerError> {
        Ok(Vec::new())
    }
    fn recall_episodic(
        &self,
        _keys: &RecallKeys,
        _limit: u32,
    ) -> Result<Vec<RecalledEpisode>, OverseerError> {
        Ok(Vec::new())
    }
    fn recall_procedural(
        &self,
        _keys: &RecallKeys,
        _limit: u32,
    ) -> Result<Vec<RecalledProcedure>, OverseerError> {
        Ok(Vec::new())
    }
    fn recall_prospective(
        &self,
        _keys: &RecallKeys,
        _limit: u32,
    ) -> Result<Vec<RecalledProspective>, OverseerError> {
        Ok(Vec::new())
    }
    fn record_observation(
        &self,
        _episode: &ObservationEpisode,
    ) -> Result<RecordOutcome, OverseerError> {
        Ok(RecordOutcome::Deduplicated)
    }
}
