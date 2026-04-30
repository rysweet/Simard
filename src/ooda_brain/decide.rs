//! Prompt-driven brain for the OODA **Decide** phase (extends the pattern
//! established for the engineer-lifecycle skip branch in PR #1458).
//!
//! Today the Decide phase performs one judgment site per priority: mapping a
//! `Priority` (goal_id + reason + urgency) to an [`ActionKind`]. Historically
//! that mapping was a deterministic match-arm on `goal_id` prefix. This module
//! reframes it as a prompt-driven decision so the routing rules live in
//! `prompt_assets/simard/ooda_decide.md` and can be iterated without code
//! changes.
//!
//! Per the standing architectural mandate, the daemon never depends on LLM
//! availability for Decide: [`DeterministicFallbackDecideBrain`] preserves the
//! pre-#1458 mapping bit-for-bit and is the floor when no LLM is configured.

use super::rustyclawd::LlmSubmitter;
use crate::error::{SimardError, SimardResult};
use crate::ooda_loop::ActionKind;

const ADAPTER_TAG: &str = "ooda-decide-brain";

/// Embedded prompt — single source of truth. Editing the markdown file
/// changes brain behaviour without code changes.
const DECIDE_PROMPT: &str = include_str!("../../prompt_assets/simard/ooda_decide.md");

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
            | Self::ExtractIdeas { rationale } => rationale,
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
        let judgment = match ctx.goal_id.as_str() {
            "__memory__" => DecideJudgment::ConsolidateMemory { rationale },
            "__improvement__" => DecideJudgment::RunImprovement { rationale },
            "__poll_activity__" => DecideJudgment::PollDeveloperActivity { rationale },
            "__extract_ideas__" => DecideJudgment::ExtractIdeas { rationale },
            _ => DecideJudgment::AdvanceGoal { rationale },
        };
        Ok(judgment)
    }
}

// ---------------------------------------------------------------------------
// LLM-backed brain (mirrors the RustyClawdBrain shape from PR #1458)
// ---------------------------------------------------------------------------

/// LLM-backed Decide brain. Generic over [`LlmSubmitter`] so tests can swap
/// in a canned-response stub without touching production wiring. The daemon
/// uses [`build_rustyclawd_decide_brain`] to construct one wired to a real
/// session.
pub struct RustyClawdDecideBrain<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdDecideBrain<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    /// Render the embedded prompt with the context. Exposed so tests can
    /// snapshot the rendering separate from LLM submission.
    pub fn render_prompt(&self, ctx: &DecideContext) -> String {
        DECIDE_PROMPT
            .replace("{goal_id}", &ctx.goal_id)
            .replace("{urgency}", &format!("{:.3}", ctx.urgency))
            .replace("{reason}", &ctx.reason)
    }
}

impl<S: LlmSubmitter> OodaDecideBrain for RustyClawdDecideBrain<S> {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let prompt = self.render_prompt(ctx);
        let raw = self.submitter.submit(&prompt)?;
        parse_judgment_from_response(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })
    }
}

/// Extract a JSON object from the LLM response (LLMs sometimes wrap JSON in
/// prose / markdown fences) and parse it as a judgment.
fn parse_judgment_from_response(raw: &str) -> Result<DecideJudgment, String> {
    let stripped = raw.trim();
    let candidate = if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
        && end >= start
    {
        &stripped[start..=end]
    } else {
        return Err(format!(
            "no JSON object found in LLM response (got {} bytes)",
            raw.len()
        ));
    };
    serde_json::from_str::<DecideJudgment>(candidate)
        .map_err(|e| format!("decide-brain-parse-error: {e}; payload={candidate}"))
}

// ---------------------------------------------------------------------------
// Production constructor
// ---------------------------------------------------------------------------

/// Production constructor mirroring [`super::build_rustyclawd_brain`].
/// Returns `Err` if no LLM provider is configured; callers must fall back
/// to [`DeterministicFallbackDecideBrain`] so the daemon's Decide phase
/// behaves identically to its pre-prompt-driven self when LLM access is
/// unavailable.
pub fn build_rustyclawd_decide_brain() -> SimardResult<Box<dyn OodaDecideBrain>> {
    let provider = crate::session_builder::LlmProvider::resolve()?;
    let submitter = super::rustyclawd::SessionLlmSubmitter::new(provider);
    Ok(Box::new(RustyClawdDecideBrain::new(submitter)))
}
