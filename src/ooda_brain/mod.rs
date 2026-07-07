//! Prompt-driven OODA brain for high-leverage decision sites (issue #1266).
//!
//! Establishes the pattern at the engineer-lifecycle skip branch in
//! `ooda_actions::advance_goal::spawn::dispatch_spawn_engineer`. Future PRs
//! migrate observe/orient/decide/curate/review to the same prompt-driven
//! shape (see PR description).
//!
//! Module split (per #1266 400-LOC cap):
//!   - `mod.rs`     — public surface: trait, types, re-exports, `apply_decision_to_state`.
//!   - `fallback.rs`— `DeterministicLifecycleBrain` (preserves today's behavior).
//!   - `rustyclawd.rs` — `RustyClawdBrain` + `LlmSubmitter` + `build_rustyclawd_brain`.
//!   - `context.rs` — `gather_engineer_lifecycle_ctx` + `redact_secrets`.

use crate::error::SimardResult;
use crate::ooda_loop::OodaState;
use std::path::PathBuf;

pub mod confidence;
mod context;
mod decide;
mod fallback;
mod judgment_record;
mod orient;
pub mod parse_failure;
pub mod prompt_store;
mod recipe_brain;
mod rustyclawd;
// Crate-visible so other recipe-runner spawn sites (goal decomposition, progress
// checking) can bound their free-text `-c` context vars with the same helper —
// closing the E2BIG argv-overflow class and the #2127 newline/YAML class at once.
pub mod sanitize;

#[cfg(test)]
mod decide_tests;
#[cfg(test)]
mod orient_tests;
#[cfg(test)]
mod prompt_store_tests;
#[cfg(test)]
mod tests;

pub use confidence::{
    CalibrationWindow, ECE_BINS, ECE_METRIC, ECE_WINDOW, HIGH_STAKES_URGENCY, JudgedDecision,
    JudgedLifecycle, LOW_TRUST_CONFIDENCE, SELF_CONSISTENCY_K, Vote, confidence_or_low_trust,
    effective_k, is_high_stakes, is_irreversible_lifecycle, lifecycle_conservative_rank,
    self_consistency_vote, should_self_consistency_sample, validate_confidence,
};
pub use context::{count_live_engineer_claims, gather_engineer_lifecycle_ctx, redact_secrets};
pub use decide::{
    DecideContext, DecideJudgment, DeterministicDecideBrain, OodaDecideBrain,
    PROMPT_NAME as DECIDE_PROMPT_NAME,
};
pub use fallback::DeterministicLifecycleBrain;
pub use judgment_record::{
    BrainJudgmentRecord, BrainPhase, clear as clear_brain_judgments, push as push_brain_judgment,
    take_all as take_brain_judgments, with_cycle_scope as with_brain_judgment_scope,
};
pub use orient::{
    DeterministicOrientBrain, FAILURE_PENALTY_PER_CONSECUTIVE, OodaOrientBrain, OrientContext,
    OrientJudgment, PROMPT_NAME as ORIENT_PROMPT_NAME, RustyClawdOrientBrain,
    build_rustyclawd_orient_brain,
};
pub use parse_failure::ParseFailureRecord;
pub use recipe_brain::RecipeBrain;
/// Shared escalation-ladder backbone + verdict-parse instrumentation reused by
/// the recipe-backed merge-judge (issue #2419 family / #2429). Exposed
/// crate-wide so `stewardship::recipe_merge_judge` runs on the SAME ladder /
/// transport / metric as the OODA brains rather than reinventing them.
pub(crate) use recipe_brain::{
    EscalationConfig, LadderRung, LifecycleParseOutcome, build_phase_escalation_note,
    extract_recipe_decision_output, record_verdict_parse_metric, run_brain_ladder,
};
/// Backward-compatible type aliases (issue #2132).
pub type RecipeDecideBrain = RecipeBrain;
pub type RecipeEngineerLifecycleBrain = RecipeBrain;
pub type RecipeOrientBrain = RecipeBrain;
pub use rustyclawd::{
    LlmSubmitter, PROMPT_NAME as ACT_PROMPT_NAME, RustyClawdBrain, SessionLlmSubmitter,
    build_rustyclawd_brain,
};

// ---------------------------------------------------------------------------
// Context fed to the brain
// ---------------------------------------------------------------------------

/// All read-only context the brain needs to decide what to do about a goal
/// that already has a live engineer worktree. Best-effort: any field may be
/// defaulted if the underlying source is missing — the brain reasons about
/// partial context.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EngineerLifecycleCtx {
    pub goal_id: String,
    pub goal_description: String,
    pub cycle_number: u32,
    pub consecutive_skip_count: u32,
    pub failure_count: u32,
    pub worktree_path: PathBuf,
    pub worktree_mtime_secs_ago: u64,
    pub sentinel_pid: Option<i32>,
    pub last_engineer_log_tail: String,
    /// How many commits the running binary's embedded git SHA is behind
    /// `origin/main` HEAD (best-effort `git rev-list` count). 0 if equal,
    /// missing, or unparseable. Used by the `consider_self_update` doctrine.
    #[serde(default)]
    pub commits_behind: u32,
    /// How many engineer worktrees currently have a live `.simard-engineer-claim`
    /// heartbeat (alive sentinel pid). Includes the worktree under inspection.
    /// `consider_self_update` is unsafe to act on while this is > 1 (or > 0
    /// from a non-engineer-lifecycle site) because the safe-update drain phase
    /// would block on the in-flight engineer.
    #[serde(default)]
    pub in_flight_engineer_count: u32,
    /// Minutes since the last safe-update attempt (success or failure).
    /// `u64::MAX` means "never attempted on this host". Compared against
    /// `safe_update::UpdateConfig::min_minutes_since_last_attempt` (default 30).
    #[serde(default = "default_minutes_since_last_update")]
    pub minutes_since_last_update_attempt: u64,
}

fn default_minutes_since_last_update() -> u64 {
    u64::MAX
}

// ---------------------------------------------------------------------------
// Decision: tagged enum the LLM emits as JSON `{"choice":"...","rationale":"..."}`
// ---------------------------------------------------------------------------

/// What the brain decided to do. Matches the JSON schema in
/// `prompt_assets/simard/ooda_brain.md`. Tagged on `choice` for
/// forward-compatibility (unknown tags fail to parse → caller falls back to
/// `ContinueSkipping`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum EngineerLifecycleDecision {
    /// Engineer is healthy / making progress. No-op this cycle.
    ContinueSkipping { rationale: String },
    /// Worktree is wedged. Tear it down and respawn with extra context.
    ReclaimAndRedispatch {
        rationale: String,
        #[serde(default)]
        redispatch_context: String,
    },
    /// Goal is consuming budget without progress. Bump failure count so the
    /// existing FAILURE_PENALTY in `orient.rs` demotes it next cycle.
    Deprioritize { rationale: String },
    /// Worth a human eyeball. Queue a tracking issue.
    OpenTrackingIssue {
        rationale: String,
        title: String,
        body: String,
    },
    /// Cannot proceed without external input. Mark goal blocked.
    MarkGoalBlocked { rationale: String, reason: String },
    /// The running binary is meaningfully behind `origin/main` and conditions
    /// look right for a safe-update. The brain only emits this after weighing
    /// the four-part doctrine documented in `prompt_assets/simard/ooda_brain.md`:
    ///
    /// 1. `commits_behind >= UpdateConfig::min_commits_since_build` (default 3)
    /// 2. `in_flight_engineer_count == 0` (or ≤1 from this site, since we
    ///    are inspecting one engineer when this brain runs)
    /// 3. `minutes_since_last_update_attempt >= min_minutes_since_last_attempt`
    ///    (default 30 — backoff to avoid thrash)
    /// 4. The current goal's engineer is healthy enough to be safely paused
    ///
    /// The act-phase dispatcher re-validates the safety predicate before
    /// invoking `simard safe-update`; if it cannot run safely the choice is
    /// recorded as deferred (success-equivalent, no state mutation).
    ConsiderSelfUpdate { rationale: String },
}

// ---------------------------------------------------------------------------
// Closed-loop outcome verification (issue #2751)
// ---------------------------------------------------------------------------

/// The structured context handed to the brain for live outcome verification.
/// Assembled by the gather step in
/// [`outcome_verify`](crate::goal_curation::outcome_verify) from the artifact
/// evidence (an INPUT, not the decider) and the freshly-gathered live signals.
///
/// `Clone` so hermetic test doubles can capture the exact ctx they were handed
/// and assert the gather→ctx wiring.
#[derive(Clone, Debug, PartialEq)]
pub struct GoalOutcomeCtx {
    /// Goal identity.
    pub goal_id: String,
    pub goal_title: String,
    /// The goal's REAL success criteria — what "achieved" actually means.
    pub success_criteria: String,
    /// Artifact-level signals from the completion-evidence gate (merged PR,
    /// closed issue, deployed). Fed as INPUT so the brain can weigh
    /// artifact-vs-outcome; it is NOT the decider.
    pub artifact_signals: crate::goal_curation::completion_gate::CompletionEvidence,
    /// Live signals gathered this cycle. The Rail-3 override checks
    /// `.iter().any(|s| s.verified)` — a compromised prompt cannot forge these.
    pub live_signals: Vec<crate::goal_curation::live_signal::LiveSignal>,
    /// How many times this goal has already been re-verified (bumped on each
    /// `reopen` / `replan`). Lets the brain notice a goal that keeps landing
    /// artifacts without ever producing the live effect.
    pub reverify_count: u32,
}

/// What the brain decided about a goal's LIVE outcome. Tagged on `choice`
/// (snake_case), matching [`EngineerLifecycleDecision`]. Only `MarkAchieved`
/// that survives the outcome-verify Rail-3 permits archival.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum GoalOutcomeDecision {
    /// Real success criteria observed live. Archive ONLY if ≥1 verified signal
    /// (Rail-3); otherwise the rail overrides this to `KeepOpenAndReport`.
    MarkAchieved { rationale: String },
    /// Artifact landed, live effect absent — keep the goal active.
    Reopen { rationale: String },
    /// Live effect absent AND the current plan won't produce it — re-scope.
    Replan {
        rationale: String,
        #[serde(default)]
        replan_hint: String,
    },
    /// Ambiguous / absent / unverifiable — no archive, surface a report. The
    /// fail-closed default (also what [`Default`] returns).
    KeepOpenAndReport { rationale: String },
}

impl Default for GoalOutcomeDecision {
    /// Fail-closed: the absence of a positive verified outcome is never an
    /// achievement. An un-migrated brain, a parse gap, or a rail override all
    /// resolve here — never to `MarkAchieved`.
    fn default() -> Self {
        GoalOutcomeDecision::KeepOpenAndReport {
            rationale: String::new(),
        }
    }
}

impl GoalOutcomeDecision {
    /// Stable snake_case label — identical to the serde `choice` tag. Shared by
    /// the judgment record, the metric context, and the curate-seam log line.
    pub fn variant_label(&self) -> &'static str {
        match self {
            Self::MarkAchieved { .. } => "mark_achieved",
            Self::Reopen { .. } => "reopen",
            Self::Replan { .. } => "replan",
            Self::KeepOpenAndReport { .. } => "keep_open_and_report",
        }
    }

    /// The rationale the brain carried on the chosen variant.
    pub fn rationale(&self) -> &str {
        match self {
            Self::MarkAchieved { rationale }
            | Self::Reopen { rationale }
            | Self::Replan { rationale, .. }
            | Self::KeepOpenAndReport { rationale } => rationale,
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency/overlap-aware engineer admission (issue #2690)
// ---------------------------------------------------------------------------

/// The goal Simard is about to spawn an engineer for, plus its **predicted file
/// footprint**. Assembled best-effort by the admission gather step.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CandidateGoal {
    /// Goal id.
    pub id: String,
    /// Goal title / task text — the work order the engineer would receive.
    pub title: String,
    /// Predicted target paths (repo-relative POSIX), derived best-effort from
    /// the goal's `wip_refs` then prior-PR file lists. EMPTY when unknown — an
    /// empty scope means "no overlap knowable" ⇒ admit (fail-open), and the
    /// exact-path rail is inert.
    #[serde(default)]
    pub predicted_scope: Vec<String>,
}

/// One in-flight engineer, with the facts the brain weighs to judge overlap.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LiveEngineerSignal {
    /// Goal id the live engineer is pursuing (recovered from its worktree dir).
    pub goal_id: String,
    /// PID recorded in the worktree claim sentinel.
    #[serde(default)]
    pub pid: i32,
    /// The engineer's worktree path (used only to compute `changed_files`).
    #[serde(default)]
    pub worktree_path: String,
    /// Files this engineer is touching: `git diff --name-only <merge-base>` ∪
    /// working-tree diff, repo-relative POSIX. Empty on any git error
    /// (absent-tolerant ⇒ no overlap ⇒ fail-open).
    #[serde(default)]
    pub changed_files: Vec<String>,
    /// Intersection of `changed_files` with the candidate's `predicted_scope`.
    /// Non-empty ⇒ an overlap signal.
    #[serde(default)]
    pub overlap_with_candidate: Vec<String>,
    /// `true` when the candidate goal's `wip_refs` reference this engineer's
    /// goal_id / PR (an explicit dependency, not just an incidental overlap).
    #[serde(default)]
    pub depended_on: bool,
}

/// The structured context handed to the brain for the admission decision.
/// Assembled by `gather_engineer_admission_ctx` — a **pure, best-effort**
/// function. Every `gh` / `git` call is made **off the state lock**, is
/// absent-tolerant, and degrades to a default (empty) value; the gather step
/// never panics and never blocks.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EngineerAdmissionCtx {
    /// The goal about to be spawned, with its predicted file footprint.
    pub candidate: CandidateGoal,
    /// Every OTHER live engineer (the candidate's own goal is excluded — the
    /// same-goal case is already handled upstream by the lifecycle branch).
    #[serde(default)]
    pub live_engineers: Vec<LiveEngineerSignal>,
    /// Resolved target repo root (used for merge-base resolution + rendering).
    #[serde(default)]
    pub repo_root: String,
}

/// What the brain decided about admitting a NEW engineer for a candidate goal
/// given the live engineer set (issue #2690). Tagged on `choice` (snake_case),
/// matching [`EngineerLifecycleDecision`] and [`GoalOutcomeDecision`].
///
/// Fail-**open** polarity: an un-migrated brain or a broken brain resolves to
/// `Admit` (scheduling is an optimization — wrongly stalling a spawn is cheaper
/// to recover from than wrongly blocking the fleet). The one control that
/// survives a broken/compromised brain is the deterministic exact-path rail in
/// the seam.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum EngineerAdmissionDecision {
    /// No blocking overlap — spawn now (the existing path, unchanged).
    Admit { rationale: String },
    /// A live engineer is touching files this goal needs — do NOT spawn this
    /// cycle. Retried naturally next OODA round. `blocked_by` names the goal(s)
    /// in the way; `retry_after_secs` is an optional advisory hint.
    Defer {
        #[serde(default)]
        blocked_by: Vec<String>,
        rationale: String,
        #[serde(default)]
        retry_after_secs: Option<u64>,
    },
    /// Spawn now, but instruct the engineer to rebase onto `after_goal_id`'s
    /// work before editing `overlap_files`. Advisory hint threaded into the
    /// engineer `task` string — no new machinery.
    SerializeAfter {
        after_goal_id: String,
        #[serde(default)]
        overlap_files: Vec<String>,
        rationale: String,
    },
}

impl Default for EngineerAdmissionDecision {
    /// Fail-open: the absence of a positive block is always an admit. An
    /// un-migrated brain, a parse gap, or the seam's Rail-2 fallback all resolve
    /// here — never to a spawn-stalling `Defer`.
    fn default() -> Self {
        EngineerAdmissionDecision::Admit {
            rationale: String::new(),
        }
    }
}

impl EngineerAdmissionDecision {
    /// Stable snake_case label — identical to the serde `choice` tag. Shared by
    /// the judgment record, the metric context, and the admission seam log line.
    pub fn variant_label(&self) -> &'static str {
        match self {
            Self::Admit { .. } => "admit",
            Self::Defer { .. } => "defer",
            Self::SerializeAfter { .. } => "serialize_after",
        }
    }

    /// The rationale the brain carried on the chosen variant.
    pub fn rationale(&self) -> &str {
        match self {
            Self::Admit { rationale }
            | Self::Defer { rationale, .. }
            | Self::SerializeAfter { rationale, .. } => rationale,
        }
    }

    /// The goal ids this decision names as blocking / serialized-after, for the
    /// judgment + metric context. Empty for `Admit`.
    pub fn blocking_goals(&self) -> Vec<String> {
        match self {
            Self::Admit { .. } => Vec::new(),
            Self::Defer { blocked_by, .. } => blocked_by.clone(),
            Self::SerializeAfter { after_goal_id, .. } => vec![after_goal_id.clone()],
        }
    }
}

// ---------------------------------------------------------------------------
// Resource-aware engineer admission (issue #2706)
// ---------------------------------------------------------------------------

/// The structured RESOURCE picture handed to the brain before admitting another
/// engineer (issue #2706). Assembled best-effort, off the state lock, by
/// `gather_resource_admission_ctx`: every field that comes from a probe is an
/// `Option` and degrades to `None` on any error, so a failing probe never fails
/// the gate. This is the complement to the dependency/overlap
/// [`EngineerAdmissionCtx`]: that one asks "will this engineer *collide* with an
/// in-flight one?"; this one asks "can the HOST *afford* another engineer right
/// now (disk / build-cache / load)?".
///
/// The AIMD controller upstream bounds engineer COUNT; it is blind to the disk
/// and build-cache that parallel `cargo` builds consume. This ctx is the
/// evidence the brain reasons over each admission cycle to add resource-aware
/// admission on top of count control.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ResourceAdmissionCtx {
    /// Goal id the candidate engineer would pursue (untrusted — sanitised before
    /// templating into the prompt).
    pub goal_id: String,

    /// Filesystem used-percent on the engineer-worktree state-root filesystem,
    /// `(1 - free/total) * 100`. `None` if the stat failed — an unknown disk
    /// makes the deterministic ceiling rail INERT (fail-open), so the brain still
    /// reasons over the remaining signals.
    #[serde(default)]
    pub disk_used_pct: Option<f64>,
    /// Free / total space on that filesystem, in GiB (rounded), for the prompt.
    #[serde(default)]
    pub disk_free_gb: Option<f64>,
    #[serde(default)]
    pub disk_total_gb: Option<f64>,

    /// Aggregate bytes under the engineer-worktree root + shared build cache
    /// (best-effort; `None` if not computed this cycle — the walk is costly, so
    /// `disk_used_pct` is the dominant, always-cheap signal).
    #[serde(default)]
    pub build_cache_bytes: Option<u64>,
    /// Number of engineer worktrees currently on disk under the state root.
    #[serde(default)]
    pub worktree_count: Option<u32>,

    /// System load average over 1 / 5 / 15 minutes (`/proc/loadavg` on Linux;
    /// `None` off-Linux or on read failure).
    #[serde(default)]
    pub load_avg_1: Option<f64>,
    #[serde(default)]
    pub load_avg_5: Option<f64>,
    #[serde(default)]
    pub load_avg_15: Option<f64>,
    /// Logical CPU count (`available_parallelism`), for interpreting load.
    #[serde(default)]
    pub cpu_count: Option<u32>,

    /// Live-claimed engineers right now (in-flight builds), from
    /// `count_live_engineer_claims`. Typed `u32`; `0` when none — a zero count is
    /// a real, knowable fact, not an unknown.
    #[serde(default)]
    pub in_flight_engineers: u32,

    /// Current AIMD concurrency cap so the brain reasons about count and
    /// resources together. `None` when adaptive scaling is not active/available.
    #[serde(default)]
    pub aimd_current_max: Option<u32>,

    /// The resolved hard ceiling this cycle (echoed so the prompt knows the
    /// deterministic limit it is reasoning below). The ceiling is ENFORCED in
    /// Rust, never by the prompt.
    pub admission_ceiling_pct: f64,
}

/// What the brain decided about admitting another engineer given the current
/// RESOURCE picture (issue #2706). Internally serde-tagged on `choice`
/// (snake_case), so an **unknown tag fails to parse** (the seam then fails
/// closed) rather than silently defaulting.
///
/// The enum has **no `Default`**: the fail-closed decision on a brain error is
/// made in the seam, not by defaulting the enum. Every variant carries a
/// `rationale` recorded verbatim (scrubbed) in the judgment record and metric.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum ResourceAdmissionDecision {
    /// The host has resource headroom — proceed (subject to the hard rail).
    Admit { rationale: String },
    /// Resources are tight — skip this cycle, retry next round (benign).
    Defer { rationale: String },
    /// Reclaim disk first (invoke the disk-health capability), then skip and
    /// retry next round against the freed space (benign).
    ReclaimFirst { rationale: String },
}

impl ResourceAdmissionDecision {
    /// Stable snake_case label — identical to the serde `choice` tag. Shared by
    /// the judgment record, the metric context, and the seam log line.
    pub fn variant_label(&self) -> &'static str {
        match self {
            Self::Admit { .. } => "admit",
            Self::Defer { .. } => "defer",
            Self::ReclaimFirst { .. } => "reclaim_first",
        }
    }

    /// The rationale the brain carried on the chosen variant.
    pub fn rationale(&self) -> &str {
        match self {
            Self::Admit { rationale }
            | Self::Defer { rationale }
            | Self::ReclaimFirst { rationale } => rationale,
        }
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Single-decision-site trait. Sync on purpose: the act-phase dispatcher is
/// sync, and the LLM-backed impl bridges to async internally so callers do
/// not see a runtime requirement.
pub trait OodaBrain: Send + Sync {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision>;

    /// Decide whether to admit a NEW engineer for `ctx.candidate` right now,
    /// given the live engineer set and file-overlap signals (issue #2690).
    /// Called at the spawn/admission decision point for a genuinely NEW
    /// engineer on a DIFFERENT goal — repeated structured evaluation of "will
    /// this new engineer collide with an in-flight one?".
    ///
    /// Scheduling optimization only — MUST fail **open**. Defaulted to the
    /// fail-open [`EngineerAdmissionDecision::Admit`] so every existing
    /// `OodaBrain` impl and test double compiles unchanged and an un-migrated
    /// brain can NEVER accidentally stall a spawn. The production [`RecipeBrain`]
    /// overrides this to run the reasoning recipe.
    fn decide_engineer_admission(
        &self,
        _ctx: &EngineerAdmissionCtx,
    ) -> SimardResult<EngineerAdmissionDecision> {
        Ok(EngineerAdmissionDecision::Admit {
            rationale: "admission-scheduling not implemented by this brain".into(),
        })
    }

    /// Decide whether the HOST can afford another engineer right now, given the
    /// current resource picture (disk %, build-cache/worktree sizes, load
    /// average, in-flight engineers) — issue #2706. Called at the spawn/admission
    /// decision point each relevant cycle (repeated structured evaluation of
    /// "can we afford one more?"), augmenting the AIMD count control with
    /// resource-aware admission.
    ///
    /// Resource admission is an optimisation only — MUST fail **open**. Defaulted
    /// to the fail-open [`ResourceAdmissionDecision::Admit`] so every existing
    /// `OodaBrain` impl and test double compiles unchanged and an un-migrated
    /// brain can NEVER accidentally stall a spawn. The production [`RecipeBrain`]
    /// overrides this to run the reasoning recipe. The one guarantee that
    /// survives a broken brain is the deterministic disk-ceiling rail in the
    /// seam (it can only be MORE conservative, never less).
    fn decide_resource_admission(
        &self,
        _ctx: &ResourceAdmissionCtx,
    ) -> SimardResult<ResourceAdmissionDecision> {
        Ok(ResourceAdmissionDecision::Admit {
            rationale: "resource-admission not implemented by this brain".into(),
        })
    }

    /// Reason about whether the goal's real success criteria are met LIVE in
    /// production (issue #2751). Called each curate cycle for
    /// completion-candidate goals — repeated structured evaluation of "is this
    /// goal *actually* achieved, live?".
    ///
    /// Defaulted to the conservative, fail-closed [`GoalOutcomeDecision::KeepOpenAndReport`]
    /// so every existing `OodaBrain` impl and test double compiles unchanged and
    /// an un-migrated brain can NEVER accidentally complete a goal. The
    /// production [`RecipeBrain`] overrides this to run the reasoning recipe.
    fn decide_goal_outcome_verification(
        &self,
        _ctx: &GoalOutcomeCtx,
    ) -> SimardResult<GoalOutcomeDecision> {
        Ok(GoalOutcomeDecision::KeepOpenAndReport {
            rationale: "outcome-verification not implemented by this brain".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Pure side-effect application (state mutation only — no IO)
// ---------------------------------------------------------------------------

/// Apply a brain decision to OODA state and return the human-readable detail
/// string the caller should attach to the resulting `ActionOutcome`.
///
/// Pure-state: does NOT kill processes, remove worktrees, or shell out to
/// `gh`. Those side effects live in `ooda_actions::advance_goal::spawn` so
/// this helper stays unit-testable without process spawning.
pub fn apply_decision_to_state(
    decision: &EngineerLifecycleDecision,
    state: &mut OodaState,
    goal_id: &str,
) -> String {
    match decision {
        EngineerLifecycleDecision::ContinueSkipping { rationale } => {
            format!("brain: continue_skipping ({rationale})")
        }
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale,
            redispatch_context,
        } => {
            // Clear the in-state assignment so the next cycle re-spawns. The
            // caller still needs to perform the kill / `git worktree remove`
            // IO outside this pure helper.
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.assigned_to = None;
            }
            state.engineer_worktrees.remove(goal_id);
            if redispatch_context.is_empty() {
                format!("brain: reclaim_and_redispatch ({rationale})")
            } else {
                format!(
                    "brain: reclaim_and_redispatch ({rationale}); redispatch_context={redispatch_context}"
                )
            }
        }
        EngineerLifecycleDecision::Deprioritize { rationale } => {
            // Bump the failure counter ourselves so even though the cycle
            // post-processor will see success=false and increment again, we
            // still get a visible bump on this very cycle (defends against
            // future refactors of cycle.rs that might not auto-increment).
            let entry = state
                .goal_failure_counts
                .entry(goal_id.to_string())
                .or_insert(0);
            *entry = entry.saturating_add(1);
            format!("brain: deprioritized ({rationale})")
        }
        EngineerLifecycleDecision::OpenTrackingIssue {
            rationale, title, ..
        } => {
            // The actual `gh issue create` shell-out happens in spawn.rs;
            // here we just return the descriptive detail string.
            format!("brain: open_tracking_issue title='{title}' ({rationale})")
        }
        EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason } => {
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.status = crate::goal_curation::GoalProgress::Blocked(reason.clone());
            }
            format!("brain: mark_goal_blocked ({rationale}); reason={reason}")
        }
        EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
            // Pure-state helper: the brain has emitted the choice but the
            // act-phase dispatcher decides whether to actually invoke
            // `simard safe-update` based on the live in-flight predicate.
            // We do NOT mutate state here — the failure-counter / blocked
            // status logic is irrelevant to a self-update decision.
            format!("brain: consider_self_update ({rationale})")
        }
    }
}

// ---------------------------------------------------------------------------
// Inline tests (issue #1979 — per-source-file coverage of the public surface
// declared here. The sibling `tests.rs` file covers brain dispatch end-to-end;
// these inline tests pin the contracts declared in *this* file so coverage
// tools see #[test]s alongside the public surface, including the
// `BrainPhase` serde round-trip that the `parse_failure::counters()` map
// uses as a HashMap key.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod inline_tests_1979 {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
    use crate::ooda_brain::BrainPhase;
    use crate::ooda_loop::OodaState;

    fn state_with_active_goal(id: &str) -> OodaState {
        let mut board = GoalBoard::default();
        board.active.push(ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: id.to_string(),
            description: "test".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: Some("engineer-a".to_string()),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
        OodaState::new(board)
    }

    // ----- BrainPhase serde round-trip -----------------------------------
    // The map `parse_failure::counters(): (BrainPhase, goal_id) -> u32` keys
    // on `BrainPhase`. An incorrect Hash/Eq/serde impl silently re-buckets
    // failures across phases — these tests pin the round-trip so an
    // accidental rename or representation change is caught early.

    #[test]
    fn brain_phase_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&BrainPhase::Act).unwrap(), "\"act\"");
        assert_eq!(
            serde_json::to_string(&BrainPhase::Decide).unwrap(),
            "\"decide\""
        );
        assert_eq!(
            serde_json::to_string(&BrainPhase::Orient).unwrap(),
            "\"orient\""
        );
    }

    #[test]
    fn brain_phase_round_trips_through_json() {
        for &phase in &[BrainPhase::Act, BrainPhase::Decide, BrainPhase::Orient] {
            let s = serde_json::to_string(&phase).unwrap();
            let back: BrainPhase = serde_json::from_str(&s).unwrap();
            assert_eq!(phase, back);
        }
    }

    #[test]
    fn brain_phase_distinct_variants_are_not_equal() {
        // Guard against a future refactor that accidentally collapses
        // variants — counters() would re-bucket all phases together.
        assert_ne!(BrainPhase::Act, BrainPhase::Decide);
        assert_ne!(BrainPhase::Decide, BrainPhase::Orient);
        assert_ne!(BrainPhase::Act, BrainPhase::Orient);
    }

    // ----- apply_decision_to_state — branches sibling tests do not pin --

    #[test]
    fn apply_decision_continue_skipping_does_not_mutate_state() {
        let mut state = state_with_active_goal("g1");
        let before_assigned = state.active_goals.active[0].assigned_to.clone();
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::ContinueSkipping {
                rationale: "hb ok".into(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("continue_skipping"));
        assert!(detail.contains("hb ok"));
        assert_eq!(state.active_goals.active[0].assigned_to, before_assigned);
    }

    #[test]
    fn apply_decision_reclaim_clears_assignment_and_worktree() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale: "wedged 7h".into(),
                redispatch_context: "retry with extra ctx".into(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("reclaim_and_redispatch"));
        assert!(detail.contains("retry with extra ctx"));
        assert!(state.active_goals.active[0].assigned_to.is_none());
        // worktree map remove is best-effort even when entry is absent.
        assert!(!state.engineer_worktrees.contains_key("g1"));
    }

    #[test]
    fn apply_decision_reclaim_omits_context_marker_when_empty() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale: "wedged".into(),
                redispatch_context: String::new(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("reclaim_and_redispatch"));
        assert!(
            !detail.contains("redispatch_context="),
            "empty redispatch_context must NOT be appended; got: {detail}"
        );
    }

    #[test]
    fn apply_decision_deprioritize_bumps_failure_counter() {
        let mut state = state_with_active_goal("g1");
        let before = state.goal_failure_counts.get("g1").copied().unwrap_or(0);
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::Deprioritize {
                rationale: "chronic".into(),
            },
            &mut state,
            "g1",
        );
        let after = state.goal_failure_counts.get("g1").copied().unwrap_or(0);
        assert_eq!(after, before + 1, "deprioritize must bump failure counter");
        assert!(detail.contains("deprioritized"));
    }

    #[test]
    fn apply_decision_mark_blocked_sets_goal_status() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::MarkGoalBlocked {
                rationale: "human input".into(),
                reason: "needs API key".into(),
            },
            &mut state,
            "g1",
        );
        match &state.active_goals.active[0].status {
            GoalProgress::Blocked(r) => assert_eq!(r, "needs API key"),
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(detail.contains("mark_goal_blocked"));
        assert!(detail.contains("needs API key"));
    }

    #[test]
    fn apply_decision_open_tracking_issue_returns_descriptive_detail() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::OpenTrackingIssue {
                rationale: "panic seen".into(),
                title: "engineer panicked".into(),
                body: "see logs".into(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("open_tracking_issue"));
        assert!(detail.contains("engineer panicked"));
        // No state mutation — the actual `gh issue create` lives elsewhere.
        assert!(state.active_goals.active[0].assigned_to.is_some());
    }

    // ----- EngineerLifecycleCtx default minutes_since_last_update --------
    #[test]
    fn lifecycle_ctx_serde_default_minutes_since_last_update_is_max() {
        // When the field is absent from incoming JSON (e.g. older cycle
        // reports), the serde default must be u64::MAX so safe-update's
        // min-gap predicate does not immediately permit an update.
        let json = r#"{
            "goal_id":"g","goal_description":"","cycle_number":0,
            "consecutive_skip_count":0,"failure_count":0,
            "worktree_path":"/tmp","worktree_mtime_secs_ago":0,
            "sentinel_pid":null,"last_engineer_log_tail":""
        }"#;
        let ctx: EngineerLifecycleCtx = serde_json::from_str(json).unwrap();
        assert_eq!(
            ctx.minutes_since_last_update_attempt,
            u64::MAX,
            "missing field must default to 'never attempted' (u64::MAX), not 0"
        );
        // commits_behind / in_flight_engineer_count use plain #[serde(default)]
        // (the type's Default → 0). Pinning so a future serde rename catches.
        assert_eq!(ctx.commits_behind, 0);
        assert_eq!(ctx.in_flight_engineer_count, 0);
    }

    #[test]
    fn lifecycle_ctx_serde_round_trip_preserves_all_fields() {
        let ctx = EngineerLifecycleCtx {
            goal_id: "g1".into(),
            goal_description: "ship".into(),
            cycle_number: 12,
            consecutive_skip_count: 3,
            failure_count: 1,
            worktree_path: PathBuf::from("/tmp/wt"),
            worktree_mtime_secs_ago: 100,
            sentinel_pid: Some(42),
            last_engineer_log_tail: "ok".into(),
            commits_behind: 4,
            in_flight_engineer_count: 2,
            minutes_since_last_update_attempt: 30,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: EngineerLifecycleCtx = serde_json::from_str(&json).unwrap();
        assert_eq!(back.goal_id, ctx.goal_id);
        assert_eq!(back.cycle_number, ctx.cycle_number);
        assert_eq!(back.sentinel_pid, ctx.sentinel_pid);
        assert_eq!(back.commits_behind, ctx.commits_behind);
        assert_eq!(
            back.minutes_since_last_update_attempt,
            ctx.minutes_since_last_update_attempt
        );
    }
}
