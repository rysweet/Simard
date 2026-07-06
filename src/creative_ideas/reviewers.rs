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
use crate::creative_ideas::prompt::{extract_json_value, render_review_context};
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

/// The amplihack agent/skill invocation seam.
///
/// The production adapter ([`SessionAgentInvoker`]) runs a real agentic turn via
/// the same session path the OODA brain uses (idle-liveness only — **no
/// wall-clock timeout** on the turn). Tests inject a deterministic fake.
pub trait AgentInvoker {
    /// Invoke an agent/skill with `prompt` and return its raw text response.
    fn invoke(&self, prompt: &str) -> SimardResult<String>;
}

impl<T: AgentInvoker + ?Sized> AgentInvoker for &T {
    fn invoke(&self, prompt: &str) -> SimardResult<String> {
        (**self).invoke(prompt)
    }
}

/// Production [`AgentInvoker`] backed by the shared session submitter (the same
/// blessed agentic path the OODA brain uses, so it inherits idle-liveness and
/// carries **no wall-clock turn cap**).
///
/// The LLM provider is resolved lazily on each [`Self::invoke`] so a
/// misconfigured provider surfaces as an explicit per-tick error (folded into
/// telemetry / `ThreadOutcome::failed`) rather than a construction-time panic
/// or a silent no-op.
#[derive(Default)]
pub struct SessionAgentInvoker;

impl SessionAgentInvoker {
    /// Construct the production invoker.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl AgentInvoker for SessionAgentInvoker {
    fn invoke(&self, prompt: &str) -> SimardResult<String> {
        use crate::ooda_brain::LlmSubmitter;
        let provider = crate::session_builder::LlmProvider::resolve()?;
        let submitter = crate::ooda_brain::SessionLlmSubmitter::new(provider);
        submitter.submit(prompt)
    }
}

/// The `crusty-old-engineer` skill prompt (scope/feasibility/necessity/utility/
/// inventiveness/RISK/need-for-human-review/practicality).
const CRUSTY_PROMPT: &str =
    include_str!("../../prompt_assets/simard/creative_ideas_review_crusty.md");
/// The `philosophy-guardian` agent prompt ("do we need this?"; no user signal required).
const PHILOSOPHY_PROMPT: &str =
    include_str!("../../prompt_assets/simard/creative_ideas_review_philosophy.md");
/// The `measurability` agent prompt (emit a concrete success metric; G1).
const MEASURABILITY_PROMPT: &str =
    include_str!("../../prompt_assets/simard/creative_ideas_review_measurability.md");

/// Render a review prompt from a template + the idea/context (simple, explicit
/// placeholder substitution — no templating engine).
fn render_review_prompt(template: &str, ctx: &ReviewContext<'_>) -> String {
    template
        .replace("{{IDEA}}", &ctx.idea.idea)
        .replace("{{RATIONALE}}", &ctx.idea.context.rationale)
        .replace("{{CONTEXT}}", &render_review_context(ctx.inputs))
}

/// The wire shape a reviewer agent returns (parsed tolerantly from its JSON
/// envelope). Verdict is validated fail-closed; unknown flags default to false.
#[derive(Debug, Deserialize)]
struct ReviewJson {
    verdict: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    high_risk: bool,
    #[serde(default)]
    irreversible: bool,
    #[serde(default)]
    needs_human: bool,
    #[serde(default)]
    metric: Option<SuccessMetric>,
}

/// Parse a reviewer's raw response into a [`Review`], fail-closed.
///
/// An unparseable envelope or an unknown verdict is a hard
/// [`SimardError::ReviewUnavailable`] — never a silent default review.
fn parse_review(reviewer: &'static str, raw: &str) -> SimardResult<Review> {
    let value = extract_json_value(raw).ok_or_else(|| SimardError::ReviewUnavailable {
        reason: format!("{reviewer}: response contained no JSON envelope"),
    })?;
    let parsed: ReviewJson =
        serde_json::from_value(value).map_err(|e| SimardError::ReviewUnavailable {
            reason: format!("{reviewer}: could not parse review JSON: {e}"),
        })?;
    let verdict = parse_verdict(reviewer, &parsed.verdict)?;
    Ok(Review {
        reviewer,
        verdict,
        notes: parsed.notes,
        flags: ReviewFlags {
            high_risk: parsed.high_risk,
            irreversible: parsed.irreversible,
            needs_human: parsed.needs_human,
        },
        proposed_metric: parsed.metric,
    })
}

/// Fail-closed verdict parse.
fn parse_verdict(reviewer: &'static str, s: &str) -> SimardResult<ReviewVerdict> {
    match s.trim() {
        "Support" => Ok(ReviewVerdict::Support),
        "Concern" => Ok(ReviewVerdict::Concern),
        "Block" => Ok(ReviewVerdict::Block),
        "NeedsHuman" => Ok(ReviewVerdict::NeedsHuman),
        other => Err(SimardError::ReviewUnavailable {
            reason: format!("{reviewer}: unknown verdict '{other}'"),
        }),
    }
}

/// Production adapter for the `crusty-old-engineer` skill.
pub struct CrustyOldEngineerReviewer<I: AgentInvoker> {
    invoker: I,
}

impl<I: AgentInvoker> CrustyOldEngineerReviewer<I> {
    /// Wrap an agent invoker.
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }
}

impl<I: AgentInvoker> Reviewer for CrustyOldEngineerReviewer<I> {
    fn id(&self) -> &'static str {
        CRUSTY_OLD_ENGINEER_ID
    }
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let prompt = render_review_prompt(CRUSTY_PROMPT, ctx);
        let raw = self.invoker.invoke(&prompt)?;
        parse_review(CRUSTY_OLD_ENGINEER_ID, &raw)
    }
}

/// Production adapter for the `philosophy-guardian` agent.
///
/// Explicitly, **a user signal is NOT required** to justify an idea; the prompt
/// treats absence of one as neutral, never a `Block`.
pub struct PhilosophyGuardianReviewer<I: AgentInvoker> {
    invoker: I,
}

impl<I: AgentInvoker> PhilosophyGuardianReviewer<I> {
    /// Wrap an agent invoker.
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }
}

impl<I: AgentInvoker> Reviewer for PhilosophyGuardianReviewer<I> {
    fn id(&self) -> &'static str {
        PHILOSOPHY_GUARDIAN_ID
    }
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let prompt = render_review_prompt(PHILOSOPHY_PROMPT, ctx);
        let raw = self.invoker.invoke(&prompt)?;
        parse_review(PHILOSOPHY_GUARDIAN_ID, &raw)
    }
}

/// Production adapter for the new `measurability` reviewer agent.
///
/// Emits a concrete [`SuccessMetric`] tied where relevant to existing
/// self-metrics (guideline G1: benchmark + live self-measurement).
pub struct MeasurabilityReviewer<I: AgentInvoker> {
    invoker: I,
}

impl<I: AgentInvoker> MeasurabilityReviewer<I> {
    /// Wrap an agent invoker.
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }
}

impl<I: AgentInvoker> Reviewer for MeasurabilityReviewer<I> {
    fn id(&self) -> &'static str {
        MEASURABILITY_ID
    }
    fn review(&self, ctx: &ReviewContext<'_>) -> SimardResult<Review> {
        let prompt = render_review_prompt(MEASURABILITY_PROMPT, ctx);
        let raw = self.invoker.invoke(&prompt)?;
        parse_review(MEASURABILITY_ID, &raw)
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
