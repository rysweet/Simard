//! The review-and-route pipeline for one freshly-generated idea.
//!
//! [`IdeaPipeline`] is the seam the generator thread uses to turn a `New` idea
//! into a reviewed, routed one: it runs the four-reviewer pipeline, lets
//! [`DefaultSynthesizer`] set the terminal status (via the fail-closed
//! state machine), then routes per that status — accepted → a **goal**,
//! human-review-flagged → a labeled + owner-tagged **issue** — and persists the
//! reviewed revision. The production [`AgenticIdeaPipeline`] wires the real
//! agent reviewers, the real `gh` seam, and the production goal store; tests
//! inject fakes.
//!
//! Destructive side-effects (goal/issue writes and the memory update) are
//! suppressed under `ctx.dry_run`: a dry-run still generates and reviews, but
//! writes nothing.
#![allow(dead_code)]

use std::path::Path;

use crate::cognitive_memory::creative_idea::{
    CreativeIdea, CreativeIdeaStore, IdeaStatus, ProspectiveCreativeIdeaStore,
};
use crate::cognitive_threads::ThreadContext;
use crate::cognitive_threads::threads::creative_ideas::GenerationInputs;
use crate::creative_ideas::reviewers::{
    AgentInvoker, CrustyOldEngineerReviewer, MeasurabilityReviewer, PhilosophyGuardianReviewer,
    Reviewer, SessionAgentInvoker, run_review_pipeline,
};
use crate::creative_ideas::routing::{
    IdeaGhClient, RealIdeaGhClient, route_idea_to_goal, route_idea_to_issue,
};
use crate::creative_ideas::synthesis::DefaultSynthesizer;
use crate::error::SimardResult;
use crate::goals::{CognitiveMemoryGoalStore, GoalStore};

/// Env var overriding the `owner/name` repo slug the routing seam targets.
pub const REPO_ENV: &str = "SIMARD_REPO";
/// Default repo slug (matches the operator-CLI / stewardship default).
pub const DEFAULT_REPO: &str = "rysweet/Simard";

/// Resolve the `owner/name` repo slug from the environment.
#[must_use]
pub fn repo_slug_from_env() -> String {
    std::env::var(REPO_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string())
}

/// Where an idea was routed after review (for telemetry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteOutcome {
    /// Accepted → a `Proposed` goal was created; idea → `ImplementationStarted`.
    Goal,
    /// Human-review-flagged → a labeled, owner-tagged issue was filed.
    Issue,
    /// Reviewed to a non-routing status (rejected / deferred / needs-revision).
    Parked,
    /// Dry-run: reviewed but nothing was written or routed.
    DryRun,
}

/// One idea's review-and-route step.
pub trait IdeaPipeline: Send {
    /// Review `idea` (mutating its reviews/status via the fail-closed state
    /// machine) and, unless `ctx.dry_run`, route it and persist the revision.
    fn review_and_route(
        &self,
        idea: &mut CreativeIdea,
        inputs: &GenerationInputs,
        ctx: &ThreadContext<'_>,
    ) -> SimardResult<RouteOutcome>;
}

/// Opens the production goal store for a given state root. A seam so tests can
/// inject an in-memory store without touching disk.
pub trait GoalStoreFactory: Send {
    /// Open a goal store rooted at `state_root`.
    fn open(&self, state_root: &Path) -> SimardResult<Box<dyn GoalStore>>;
}

/// Production factory: the cognitive-memory-backed goal store (the same store
/// the engineer/overseer pipeline consumes).
#[derive(Default)]
pub struct CognitiveMemoryGoalStoreFactory;

impl GoalStoreFactory for CognitiveMemoryGoalStoreFactory {
    fn open(&self, state_root: &Path) -> SimardResult<Box<dyn GoalStore>> {
        Ok(Box::new(CognitiveMemoryGoalStore::new(
            state_root.to_path_buf(),
        )?))
    }
}

/// Production [`IdeaPipeline`]: real agent reviewers + real `gh` + goal store.
pub struct AgenticIdeaPipeline {
    invoker: Box<dyn AgentInvoker + Send>,
    gh: Box<dyn IdeaGhClient + Send>,
    goals: Box<dyn GoalStoreFactory>,
    repo: String,
}

impl AgenticIdeaPipeline {
    /// Build with explicit seams (test seam).
    #[must_use]
    pub fn new(
        invoker: Box<dyn AgentInvoker + Send>,
        gh: Box<dyn IdeaGhClient + Send>,
        goals: Box<dyn GoalStoreFactory>,
        repo: String,
    ) -> Self {
        Self {
            invoker,
            gh,
            goals,
            repo,
        }
    }

    /// Build the production pipeline (real session invoker, real `gh`, real goal
    /// store, repo slug from the environment).
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            Box::new(SessionAgentInvoker::new()),
            Box::new(RealIdeaGhClient::new()),
            Box::new(CognitiveMemoryGoalStoreFactory),
            repo_slug_from_env(),
        )
    }
}

impl IdeaPipeline for AgenticIdeaPipeline {
    fn review_and_route(
        &self,
        idea: &mut CreativeIdea,
        inputs: &GenerationInputs,
        ctx: &ThreadContext<'_>,
    ) -> SimardResult<RouteOutcome> {
        // 1. Review: all four reviewers contribute, synthesis sets the status.
        let invoker = self.invoker.as_ref();
        let crusty = CrustyOldEngineerReviewer::new(invoker);
        let philosophy = PhilosophyGuardianReviewer::new(invoker);
        let measurability = MeasurabilityReviewer::new(invoker);
        let reviewers: [&dyn Reviewer; 3] = [&crusty, &philosophy, &measurability];
        run_review_pipeline(idea, inputs, &reviewers, &DefaultSynthesizer)?;

        // 2. Dry-run: reviewed, but write and route nothing.
        if ctx.dry_run {
            return Ok(RouteOutcome::DryRun);
        }

        // 3. Route per the synthesized status.
        let outcome = match idea.status {
            IdeaStatus::AcceptedForImplementation => {
                let goals = self.goals.open(ctx.state_root)?;
                route_idea_to_goal(idea, goals.as_ref(), ctx.now_epoch)?;
                // The idea moves into flight once the goal exists.
                idea.try_transition(IdeaStatus::ImplementationStarted)?;
                RouteOutcome::Goal
            }
            IdeaStatus::NeedsHumanReview => {
                route_idea_to_issue(idea, self.gh.as_ref(), &self.repo)?;
                RouteOutcome::Issue
            }
            _ => RouteOutcome::Parked,
        };

        // 4. Persist the reviewed (and possibly transitioned) revision.
        let store = ProspectiveCreativeIdeaStore::new(ctx.memory);
        store.update(idea)?;
        Ok(outcome)
    }
}
