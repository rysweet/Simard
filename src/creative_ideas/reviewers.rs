//! The reviewer pipeline — four reviewers vet each new idea (design spike #2419).
//!
//! Each new [`CreativeIdea`] is reviewed by a fixed set of reviewers, then the
//! [`FeedbackSynthesizer`] sets the next status. The pipeline is expressed as a
//! trait so reviewers are pluggable and independently testable; production
//! adapters shape an amplihack skill/agent invocation through the
//! [`AgentInvoker`] seam (their prompt bodies are marked `// FUTURE:` stubs),
//! and all tests inject deterministic fakes.
//!
//! The four reviewers, in order:
//! 1. `crusty_old_engineer` — scope/feasibility/necessity/utility/RISK/practicality.
//! 2. `philosophy_guardian` — "do we need this?"; a user signal is NOT required.
//! 3. `measurability` — emits a concrete [`SuccessMetric`].
//! 4. `idea_feedback_synthesis` — the synthesis step (see [`super::synthesis`]).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::creative_idea::CreativeIdea;
use crate::cognitive_threads::threads::creative_ideas::GenerationInputs;
use crate::creative_ideas::synthesis::{FeedbackSynthesizer, SuccessMetric, SynthesisOutcome};
use crate::error::{SimardError, SimardResult};

/// Stable telemetry id — crusty-old-engineer skill reviewer.
pub const CRUSTY_OLD_ENGINEER_ID: &str = "crusty_old_engineer";
/// Stable telemetry id — philosophy-guardian agent reviewer.
pub const PHILOSOPHY_GUARDIAN_ID: &str = "philosophy_guardian";
/// Stable telemetry id — the new measurability reviewer agent.
pub const MEASURABILITY_ID: &str = "measurability";

/// Map a persisted reviewer-id string back to its stable `&'static str` id.
///
/// **Fail-closed**: an unknown id yields `None` so the caller can raise
/// [`SimardError::InvalidCreativeIdeaRecord`] rather than silently defaulting.
#[must_use]
pub fn reviewer_id_from_str(s: &str) -> Option<&'static str> {
    match s {
        CRUSTY_OLD_ENGINEER_ID => Some(CRUSTY_OLD_ENGINEER_ID),
        PHILOSOPHY_GUARDIAN_ID => Some(PHILOSOPHY_GUARDIAN_ID),
        MEASURABILITY_ID => Some(MEASURABILITY_ID),
        crate::creative_ideas::synthesis::IDEA_FEEDBACK_SYNTHESIS_ID => {
            Some(crate::creative_ideas::synthesis::IDEA_FEEDBACK_SYNTHESIS_ID)
        }
        _ => None,
    }
}

/// A reviewer's overall verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReviewVerdict {
    /// The idea is worth pursuing.
    Support,
    /// Reservations, but not a blocker.
    Concern,
    /// The idea should not proceed as written.
    Block,
    /// A human must decide.
    NeedsHuman,
}

/// Risk flags a reviewer can raise; any of these routes the idea to human review.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewFlags {
    /// High blast-radius / risky change.
    pub high_risk: bool,
    /// Irreversible change (data loss, deploy, external side-effect).
    pub irreversible: bool,
    /// Explicitly requires a human decision.
    pub needs_human: bool,
}

/// Immutable inputs handed to each reviewer.
pub struct ReviewContext<'a> {
    /// The idea under review.
    pub idea: &'a CreativeIdea,
    /// The generation inputs (observation window) that produced it.
    pub inputs: &'a GenerationInputs,
}

/// One reviewer's structured output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Review {
    /// Stable reviewer id (for telemetry).
    pub reviewer: &'static str,
    /// The verdict.
    pub verdict: ReviewVerdict,
    /// Free-text notes.
    pub notes: String,
    /// Risk flags.
    pub flags: ReviewFlags,
    /// A proposed success metric (the measurability reviewer only).
    pub proposed_metric: Option<SuccessMetric>,
}

/// A pluggable reviewer.
pub trait Reviewer {
    /// Stable telemetry id.
    fn id(&self) -> &'static str;
    /// Review the idea in `ctx`.
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review>;
}

/// The amplihack skill/agent invocation seam.
///
/// FUTURE (M3): the real implementation shells out to the amplihack binary
/// (`SIMARD_AMPLIHACK_BIN`) to run the `crusty-old-engineer` skill /
/// `philosophy-guardian` agent / measurability agent. During the spike the
/// production adapters below build the prompt and call this seam, but do not
/// interpret the response; all tests inject fakes.
pub trait AgentInvoker {
    /// Invoke an agent/skill with `prompt` and return its raw text response.
    fn invoke(&self, prompt: &str) -> SimardResult<String>;
}

/// Production adapter for the `crusty-old-engineer` skill.
///
/// FUTURE (M3): parse the skill response into a [`Review`]. Until then
/// `review` returns [`SimardError::ReviewUnavailable`]; the spike drives the
/// pipeline through fakes.
pub struct CrustyOldEngineerReviewer<I: AgentInvoker> {
    invoker: I,
}

impl<I: AgentInvoker> CrustyOldEngineerReviewer<I> {
    /// Wrap an agent invoker.
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }

    fn build_prompt(_ctx: &ReviewContext<'_>) -> String {
        // FUTURE (M3): render the real crusty-old-engineer skill prompt from
        // the idea + observation window (scope/feasibility/necessity/utility/
        // inventiveness/RISK/need-for-human-review/practicality).
        String::from("FUTURE: crusty-old-engineer review prompt")
    }
}

impl<I: AgentInvoker> Reviewer for CrustyOldEngineerReviewer<I> {
    fn id(&self) -> &'static str {
        CRUSTY_OLD_ENGINEER_ID
    }
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let prompt = Self::build_prompt(ctx);
        let _raw = self.invoker.invoke(&prompt)?;
        Err(SimardError::ReviewUnavailable {
            reason: "crusty-old-engineer reviewer is not wired during the spike (M3)".to_string(),
        })
    }
}

/// Production adapter for the `philosophy-guardian` agent.
///
/// Explicitly, **a user signal is NOT required** to justify an idea; absence of
/// one is neutral, never a `Block`. FUTURE (M3): parse the agent response.
pub struct PhilosophyGuardianReviewer<I: AgentInvoker> {
    invoker: I,
}

impl<I: AgentInvoker> PhilosophyGuardianReviewer<I> {
    /// Wrap an agent invoker.
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }

    fn build_prompt(_ctx: &ReviewContext<'_>) -> String {
        // FUTURE (M3): "do we need this? will it be an interesting
        // enhancement?" — treat missing user signal as neutral.
        String::from("FUTURE: philosophy-guardian review prompt")
    }
}

impl<I: AgentInvoker> Reviewer for PhilosophyGuardianReviewer<I> {
    fn id(&self) -> &'static str {
        PHILOSOPHY_GUARDIAN_ID
    }
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let prompt = Self::build_prompt(ctx);
        let _raw = self.invoker.invoke(&prompt)?;
        Err(SimardError::ReviewUnavailable {
            reason: "philosophy-guardian reviewer is not wired during the spike (M3)".to_string(),
        })
    }
}

/// Production adapter for the new `measurability` reviewer agent.
///
/// FUTURE (M3): parse the agent response into a concrete [`SuccessMetric`]
/// tied where relevant to existing self-metrics.
pub struct MeasurabilityReviewer<I: AgentInvoker> {
    invoker: I,
}

impl<I: AgentInvoker> MeasurabilityReviewer<I> {
    /// Wrap an agent invoker.
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }

    fn build_prompt(_ctx: &ReviewContext<'_>) -> String {
        // FUTURE (M3): ask for a concrete metric (name/baseline/target/
        // how_measured), preferring recall_precision_at_k / distill fact-yield
        // / reasoner-reliability where relevant.
        String::from("FUTURE: measurability review prompt")
    }
}

impl<I: AgentInvoker> Reviewer for MeasurabilityReviewer<I> {
    fn id(&self) -> &'static str {
        MEASURABILITY_ID
    }
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let prompt = Self::build_prompt(ctx);
        let _raw = self.invoker.invoke(&prompt)?;
        Err(SimardError::ReviewUnavailable {
            reason: "measurability reviewer is not wired during the spike (M3)".to_string(),
        })
    }
}

/// Run the full reviewer pipeline over one idea and apply the synthesized
/// status.
///
/// Invokes every reviewer in `reviewers` (the three vetting reviewers; the
/// `synthesizer` is the fourth step), folds their [`Review`]s through the
/// synthesizer, attaches the accumulated reviews and any success metric to the
/// idea, and applies the next status via
/// [`CreativeIdea::try_transition`] — so an illegal synthesis verdict is a hard
/// [`SimardError::InvalidIdeaTransition`], never a silent corruption.
pub fn run_review_pipeline(
    idea: &mut CreativeIdea,
    inputs: &GenerationInputs,
    reviewers: &[&dyn Reviewer],
    synthesizer: &dyn FeedbackSynthesizer,
) -> SimardResult<SynthesisOutcome> {
    // Collect reviews under an immutable borrow, then release it before mutating.
    let reviews: Vec<Review> = {
        let ctx = ReviewContext { idea, inputs };
        let mut acc = Vec::with_capacity(reviewers.len());
        for reviewer in reviewers {
            acc.push(reviewer.review(&ctx)?);
        }
        acc
    };

    let outcome = {
        let ctx = ReviewContext { idea, inputs };
        synthesizer.synthesize(&ctx, &reviews)?
    };

    idea.reviews = reviews;
    if let Some(metric) = &outcome.set_metric {
        idea.success_metric = Some(metric.clone());
    }
    idea.try_transition(outcome.next_status)?;
    Ok(outcome)
}
