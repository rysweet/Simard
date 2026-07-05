//! Feedback synthesis — the fourth reviewer step (design spike #2419).
//!
//! The `idea-feedback-synthesis` step reads **all** reviews plus the idea
//! context, summarizes next steps, and **sets the next status** per the
//! [`IdeaStatus`](crate::cognitive_memory::creative_idea::IdeaStatus) state
//! machine. The pipeline runner applies the returned status through
//! [`CreativeIdea::try_transition`](crate::cognitive_memory::creative_idea::CreativeIdea::try_transition),
//! so an illegal `next_status` is a hard
//! [`InvalidIdeaTransition`](crate::error::SimardError::InvalidIdeaTransition),
//! never a silent corruption.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::creative_idea::IdeaStatus;
use crate::creative_ideas::reviewers::{Review, ReviewContext, ReviewVerdict};
use crate::error::SimardResult;

/// Stable telemetry id for the synthesis step (the fourth "reviewer").
pub const IDEA_FEEDBACK_SYNTHESIS_ID: &str = "idea_feedback_synthesis";

/// A concrete, measurable success criterion for an idea.
///
/// Emitted by the `measurability` reviewer and carried on the idea; it is the
/// **only** thing that can later move the idea to `ImplementationCompleted`
/// (see [`crate::creative_ideas::routing::mark_completed`]). Tied where relevant
/// to existing self-metrics (`recall_precision_at_k`, distill fact-yield,
/// reasoner-reliability).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SuccessMetric {
    /// Metric name, e.g. `"recall_precision_at_k"`.
    pub name: String,
    /// Optional numeric baseline to improve on.
    pub baseline: Option<f64>,
    /// Target expression, e.g. `">= +0.05 over 7-day baseline"`.
    pub target: String,
    /// How the metric is measured.
    pub how_measured: String,
}

/// The outcome of the synthesis step.
#[derive(Clone, Debug, PartialEq)]
pub struct SynthesisOutcome {
    /// The next status — MUST be a legal transition from the idea's current
    /// status (enforced by the pipeline runner).
    pub next_status: IdeaStatus,
    /// Human-readable summary of the next steps.
    pub next_steps: String,
    /// A metric to attach to the idea (from the measurability reviewer).
    pub set_metric: Option<SuccessMetric>,
}

/// Folds all reviews into a next-status decision plus next-steps text.
pub trait FeedbackSynthesizer {
    /// Stable telemetry id.
    fn id(&self) -> &'static str {
        IDEA_FEEDBACK_SYNTHESIS_ID
    }
    /// Decide the next status from the accumulated reviews.
    fn synthesize(
        &self,
        ctx: &ReviewContext<'_>,
        reviews: &[Review],
    ) -> SimardResult<SynthesisOutcome>;
}

/// The default, deterministic synthesis policy (design doc §"Synthesis policy").
///
/// 1. Any reviewer sets `irreversible` **or** `high_risk` **or** `needs_human`
///    → `NeedsHumanReview`.
/// 2. Any `Block` (from a non-`philosophy_guardian` reviewer) with no human
///    flag → `Rejected`.
/// 3. No metric produced → `NeedsRevision` (an idea with no way to measure
///    success is not acceptable).
/// 4. Otherwise, sufficient support + a metric → `AcceptedForImplementation`.
pub struct DefaultSynthesizer;

impl FeedbackSynthesizer for DefaultSynthesizer {
    fn synthesize(
        &self,
        _ctx: &ReviewContext<'_>,
        reviews: &[Review],
    ) -> SimardResult<SynthesisOutcome> {
        let human_flagged = reviews
            .iter()
            .any(|r| r.flags.high_risk || r.flags.irreversible || r.flags.needs_human);

        // The measurability reviewer's metric is the one that can later gate
        // `ImplementationCompleted`.
        let metric = reviews.iter().find_map(|r| r.proposed_metric.clone());

        // A fatal block from any reviewer other than philosophy-guardian
        // (whose absence-of-user-signal is never a `Block`).
        let hard_block = reviews.iter().any(|r| {
            matches!(r.verdict, ReviewVerdict::Block)
                && r.reviewer != crate::creative_ideas::reviewers::PHILOSOPHY_GUARDIAN_ID
        });

        let (next_status, next_steps) = if human_flagged {
            (
                IdeaStatus::NeedsHumanReview,
                "flagged high-risk/irreversible/needs-human by a reviewer; routing to human review"
                    .to_string(),
            )
        } else if hard_block {
            (
                IdeaStatus::Rejected,
                "blocked by a reviewer with no fixable path; rejected".to_string(),
            )
        } else if metric.is_none() {
            (
                IdeaStatus::NeedsRevision,
                "no success metric produced; revise so effectiveness is measurable".to_string(),
            )
        } else {
            (
                IdeaStatus::AcceptedForImplementation,
                "sufficient support and a success metric; accepted for implementation".to_string(),
            )
        };

        Ok(SynthesisOutcome {
            next_status,
            next_steps,
            set_metric: metric,
        })
    }
}
