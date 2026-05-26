//! Types and trait for the OODA **Decide** phase.
//!
//! The Decide phase maps a `Priority` (goal_id + reason + urgency) to an
//! [`ActionKind`]. The LLM-backed implementation lives in
//! [`super::recipe_decide::RecipeDecideBrain`] (recipe-runner-rs subprocess).
//! This module defines the shared types, trait, and the deterministic
//! fallback brain that preserves pre-#1458 behaviour when no LLM is
//! configured.
//!
//! Per the standing architectural mandate, the daemon never depends on LLM
//! availability for Decide: [`DeterministicFallbackDecideBrain`] preserves the
//! pre-#1458 mapping bit-for-bit and is the floor when no LLM is configured.

use crate::error::SimardResult;
use crate::ooda_loop::ActionKind;
use crate::ooda_loop::SyntheticPriorityKind;

/// Prompt asset name. The on-disk file is read fresh per call via
/// [`prompt_store::global`]; the compile-time embedded baseline is
/// preserved in [`prompt_store::embedded_fallback`] so the daemon never
/// fails because a prompt file is missing.
pub const PROMPT_NAME: &str = "ooda_decide.md";

// ---------------------------------------------------------------------------
// Context fed to the brain
// ---------------------------------------------------------------------------

/// Read-only view of one priority entry that the Decide brain judges. Mirrors
/// the per-priority columns that the deterministic match-arm consumed
/// (`goal_id`, `urgency`, `reason`) so a fallback impl can reproduce the
/// existing routing exactly.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DecideContext {
    pub goal_id: String,
    pub urgency: f64,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Judgment: tagged enum the LLM emits as `{"choice":"...","rationale":"..."}`
// ---------------------------------------------------------------------------

/// What action kind the brain decided this priority maps to. Tagged on
/// `choice` for forward-compatibility; unknown tags fail to parse and the
/// caller falls back to the deterministic mapping.
///
/// Variants intentionally cover every [`ActionKind`] (not just the five
/// emitted today) so the prompt can evolve to route to currently-unused
/// kinds (`research_query`, `run_gym_eval`, `build_skill`, `launch_session`)
/// without touching this file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum DecideJudgment {
    AdvanceGoal { rationale: String },
    RunImprovement { rationale: String },
    ConsolidateMemory { rationale: String },
    ResearchQuery { rationale: String },
    RunGymEval { rationale: String },
    BuildSkill { rationale: String },
    LaunchSession { rationale: String },
    PollDeveloperActivity { rationale: String },
    ExtractIdeas { rationale: String },
    SafeUpdate { rationale: String },
}

impl DecideJudgment {
    /// Project the tagged judgment back onto the existing [`ActionKind`]
    /// enum so the Decide phase can keep emitting `PlannedAction` values
    /// unchanged.
    pub fn action_kind(&self) -> ActionKind {
        match self {
            Self::AdvanceGoal { .. } => ActionKind::AdvanceGoal,
            Self::RunImprovement { .. } => ActionKind::RunImprovement,
            Self::ConsolidateMemory { .. } => ActionKind::ConsolidateMemory,
            Self::ResearchQuery { .. } => ActionKind::ResearchQuery,
            Self::RunGymEval { .. } => ActionKind::RunGymEval,
            Self::BuildSkill { .. } => ActionKind::BuildSkill,
            Self::LaunchSession { .. } => ActionKind::LaunchSession,
            Self::PollDeveloperActivity { .. } => ActionKind::PollDeveloperActivity,
            Self::ExtractIdeas { .. } => ActionKind::ExtractIdeas,
            Self::SafeUpdate { .. } => ActionKind::SafeUpdate,
        }
    }

    pub fn rationale(&self) -> &str {
        match self {
            Self::AdvanceGoal { rationale }
            | Self::RunImprovement { rationale }
            | Self::ConsolidateMemory { rationale }
            | Self::ResearchQuery { rationale }
            | Self::RunGymEval { rationale }
            | Self::BuildSkill { rationale }
            | Self::LaunchSession { rationale }
            | Self::PollDeveloperActivity { rationale }
            | Self::ExtractIdeas { rationale }
            | Self::SafeUpdate { rationale } => rationale,
        }
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Single-decision-site trait for the Decide phase. Sync on purpose to match
/// [`super::OodaBrain`] — the LLM-backed impl bridges to async internally so
/// callers do not need a runtime.
pub trait OodaDecideBrain: Send + Sync {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment>;
}

// ---------------------------------------------------------------------------
// Deterministic fallback — preserves pre-#1458 behaviour bit-for-bit
// ---------------------------------------------------------------------------

/// Fallback impl that mirrors the deterministic match-arm previously inlined
/// in `src/ooda_loop/decide.rs`. This is the safety floor: when no LLM is
/// configured, the daemon's Decide phase behaves identically to its
/// pre-prompt-driven self.
#[derive(Debug, Default)]
pub struct DeterministicFallbackDecideBrain;

impl OodaDecideBrain for DeterministicFallbackDecideBrain {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let rationale = "fallback-brain: prefix-routed".to_string();
        // Route synthetic priorities via the typed enum (single source of
        // truth in `ooda_loop::priority_kind`). Real goal_ids — and any
        // unrecognized synthetic — fall through to AdvanceGoal.
        let judgment = match SyntheticPriorityKind::from_synthetic_id(ctx.goal_id.as_str()) {
            Some(SyntheticPriorityKind::ConsolidateMemory) => {
                DecideJudgment::ConsolidateMemory { rationale }
            }
            Some(SyntheticPriorityKind::RunImprovement) => {
                DecideJudgment::RunImprovement { rationale }
            }
            Some(SyntheticPriorityKind::PollDeveloperActivity) => {
                DecideJudgment::PollDeveloperActivity { rationale }
            }
            Some(SyntheticPriorityKind::ExtractIdeas) => DecideJudgment::ExtractIdeas { rationale },
            Some(SyntheticPriorityKind::SafeUpdate) => DecideJudgment::SafeUpdate { rationale },
            Some(SyntheticPriorityKind::EvalWatchdog) | None => {
                DecideJudgment::AdvanceGoal { rationale }
            }
        };
        Ok(judgment)
    }
}
