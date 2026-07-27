//! The Creative Ideas generator thread (design spike #2419).
//!
//! `CreativeIdeasThread` implements [`CognitiveThread`] and reuses
//! [`ThreadKind::BackgroundThought`] (Decision 2). It is **default-ON, opt-out**
//! (`enabled()` reads [`CreativeIdeasConfig::enabled`], default `true`), so the
//! scheduler ticks it on a stock deployment unless `SIMARD_CREATIVE_IDEAS_ENABLED`
//! is set to a falsey value. The OODA daemon registers it via
//! [`register_creative_ideas_if_enabled`], independent of the generic
//! `SIMARD_COGNITIVE_THREADS_ENABLED` master switch — consistent with the
//! default-ON Overseer/Journal threads.
//!
//! `tick` is **total by contract**: every internal `Err` is caught,
//! `tracing::warn!`-logged with the stable `creative_ideas` id, and returned as
//! [`ThreadOutcome::failed`] — a single idea's failure never aborts the batch or
//! the daemon. It honors `ctx.shutdown` (cooperative cancellation) and
//! `ctx.dry_run` (no external side-effect).
#![allow(dead_code)]

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::mind::Mind;
use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
use crate::cognitive_memory::creative_idea::{
    CreativeIdea, CreativeIdeaStore, IdeaContext, MemoryLink, ProspectiveCreativeIdeaStore,
};
use crate::creative_ideas::CreativeIdeasConfig;
use crate::creative_ideas::dedup;
use crate::creative_ideas::dedup_gate::{self, PlannedAction};
use crate::creative_ideas::pipeline::{AgenticIdeaPipeline, IdeaPipeline, RouteOutcome};
use crate::creative_ideas::source::AgenticIdeaSource;
use crate::error::{SimardError, SimardResult};
use crate::ooda_brain::{DeterministicLifecycleBrain, OodaBrain};

/// Stable telemetry id.
pub const CREATIVE_IDEAS_ID: &str = "creative_ideas";

/// How many recent episodes/entries to fold into the observation window.
const OBSERVATION_LIMIT: usize = 20;
/// How many stored episodes to scan for the observation window.
const EPISODE_SCAN_LIMIT: u32 = 200;
/// How many previously-generated ideas to load for dedup/novelty.
const PREVIOUS_IDEAS_LIMIT: u32 = 256;

/// A >= 24h window of recent progress & behavior (from the Journal / OODA).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityWindow {
    /// The window size actually covered, in seconds.
    pub window_secs: u64,
    /// Digested activity entries (newest first).
    pub entries: Vec<String>,
}

/// Typed, read-only inputs assembled by the generator's observation window.
#[derive(Clone, Debug, Default)]
pub struct GenerationInputs {
    /// Active/proposed goals.
    pub current_goals: Vec<String>,
    /// >= 24h of progress & behavior.
    pub recent_activity: ActivityWindow,
    /// Cognitive-memory episodic digests.
    pub episodic_summaries: Vec<String>,
    /// Open goals/PRs/sessions.
    pub works_in_progress: Vec<String>,
    /// Overseer observations (cross-pollination).
    pub overseer_observations: Vec<String>,
    /// Insights extracted from meetings/conversations.
    pub conversation_insights: Vec<String>,
    /// Previously-generated ideas (for dedup/novelty).
    pub previous_ideas: Vec<CreativeIdea>,
}

/// A raw idea candidate produced by an [`IdeaSource`], before review.
#[derive(Clone, Debug, PartialEq)]
pub struct RawIdea {
    /// The idea text.
    pub idea: String,
    /// Supporting memory links.
    pub links: Vec<MemoryLink>,
    /// Why this idea surfaced.
    pub rationale: String,
}

/// Produces raw idea candidates from the observation window.
pub trait IdeaSource: Send {
    /// Produce up to `n` diverse raw idea candidates from `inputs`.
    fn generate(&self, inputs: &GenerationInputs, n: usize) -> SimardResult<Vec<RawIdea>>;
}

/// Deterministic idea source for tests (no network).
#[derive(Clone, Debug, Default)]
pub struct FakeIdeaSource {
    ideas: Vec<RawIdea>,
    fail: bool,
}

impl FakeIdeaSource {
    /// Build a source that yields the given ideas (truncated to `n`).
    #[must_use]
    pub fn with_ideas(ideas: Vec<RawIdea>) -> Self {
        Self { ideas, fail: false }
    }

    /// Build a source whose `generate` always fails (drives the tick-is-total
    /// test).
    #[must_use]
    pub fn failing() -> Self {
        Self {
            ideas: Vec::new(),
            fail: true,
        }
    }
}

impl IdeaSource for FakeIdeaSource {
    fn generate(&self, _inputs: &GenerationInputs, n: usize) -> SimardResult<Vec<RawIdea>> {
        if self.fail {
            return Err(SimardError::ReviewUnavailable {
                reason: "fake idea source configured to fail".to_string(),
            });
        }
        Ok(self.ideas.iter().take(n).cloned().collect())
    }
}

/// The Creative Ideas generator cognitive thread.
pub struct CreativeIdeasThread {
    cfg: CreativeIdeasConfig,
    source: Box<dyn IdeaSource>,
    pipeline: Box<dyn IdeaPipeline>,
    /// The semantic dedup + enhance reasoner (issue #2925). Consulted per
    /// candidate before persistence. The production brain is a `RecipeBrain`;
    /// tests inject a stub. When `semantic_enabled` is false the brain is NOT
    /// consulted (the gate runs the deterministic word-set backstop only).
    brain: Box<dyn OodaBrain>,
    /// Whether the agentic (semantic) dedup layer is active. False falls the gate
    /// back to deterministic word-set dedup (today's behaviour, no enhance).
    semantic_enabled: bool,
    /// Stage-1 coarse-shortlist size fed to the reasoner per candidate.
    shortlist_k: usize,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl CreativeIdeasThread {
    /// Build the thread from an explicit config + idea source, defaulting to the
    /// production review/route pipeline (test seam).
    #[must_use]
    pub fn new(cfg: CreativeIdeasConfig, source: Box<dyn IdeaSource>) -> Self {
        Self::with_pipeline(cfg, source, Box::new(AgenticIdeaPipeline::from_env()))
    }

    /// Build with an explicit config, idea source, and review/route pipeline
    /// (the fully-injectable test seam). The semantic dedup layer defaults to
    /// **off** here (deterministic word-set dedup, today's behaviour); the
    /// production entrypoints ([`Self::from_env`], [`register`]) wire the
    /// recipe-backed reasoner. Use [`Self::with_pipeline_and_brain`] to inject a
    /// stub reasoner in tests.
    #[must_use]
    pub fn with_pipeline(
        cfg: CreativeIdeasConfig,
        source: Box<dyn IdeaSource>,
        pipeline: Box<dyn IdeaPipeline>,
    ) -> Self {
        Self::with_pipeline_and_brain(
            cfg,
            source,
            pipeline,
            Box::new(DeterministicLifecycleBrain),
            /* semantic_enabled */ false,
        )
    }

    /// Fully-injectable seam: explicit config, source, pipeline, dedup reasoner,
    /// and whether the semantic layer is active (issue #2925).
    #[must_use]
    pub fn with_pipeline_and_brain(
        cfg: CreativeIdeasConfig,
        source: Box<dyn IdeaSource>,
        pipeline: Box<dyn IdeaPipeline>,
        brain: Box<dyn OodaBrain>,
        semantic_enabled: bool,
    ) -> Self {
        Self {
            cfg,
            source,
            pipeline,
            brain,
            semantic_enabled,
            shortlist_k: dedup_gate::shortlist_k_from_env(),
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }

    /// Build from the environment with the production idea source + pipeline +
    /// recipe-backed semantic dedup reasoner (issue #2925).
    #[must_use]
    pub fn from_env() -> Self {
        let (brain, semantic_enabled) = build_dedup_brain();
        Self::with_pipeline_and_brain(
            CreativeIdeasConfig::from_env(),
            Box::new(AgenticIdeaSource::from_env()),
            Box::new(AgenticIdeaPipeline::from_env()),
            brain,
            semantic_enabled,
        )
    }

    /// The core, fallible tick body. `tick` wraps this and folds any `Err` into
    /// a [`ThreadOutcome::failed`] so the public contract stays infallible.
    fn run_tick(&mut self, ctx: &mut ThreadContext<'_>) -> SimardResult<GenerationReport> {
        let inputs = self.assemble_inputs(ctx);

        let raw = self.source.generate(&inputs, self.cfg.batch)?;
        let generated = raw.len();

        // Bound the number of reasoner calls per tick BEFORE the semantic gate.
        // The gate (not a batch word-set filter) is now the dedup authority.
        let selected = dedup::select_balanced(raw, self.cfg.batch);
        let surviving = selected.len();

        let mut report = GenerationReport {
            generated,
            surviving,
            persisted: 0,
            reviewed: 0,
            routed_goal: 0,
            routed_issue: 0,
            review_errors: 0,
            skipped: 0,
            enhanced: 0,
            dedup_errors: 0,
            dry_run: ctx.dry_run,
        };

        for raw_idea in &selected {
            if ctx.shutdown.load(Ordering::Relaxed) {
                break;
            }

            // Semantic dedup + enhance decision (issue #2925). The pool is the
            // trigger-scoped previous ideas already loaded by `assemble_inputs`.
            let plan = dedup_gate::plan_candidate(
                raw_idea,
                &inputs.previous_ideas,
                self.brain.as_ref(),
                self.semantic_enabled,
                self.shortlist_k,
                dedup::DEFAULT_DEDUP_THRESHOLD,
            );

            match plan {
                PlannedAction::Create => {
                    self.create_candidate(raw_idea, &inputs, ctx, &mut report)?;
                }
                PlannedAction::Skip { .. } => {
                    // A true semantic duplicate: drop it, nothing persisted.
                    report.skipped += 1;
                }
                PlannedAction::Enhance {
                    target_node_id,
                    rationale,
                } => {
                    let store = ProspectiveCreativeIdeaStore::new(ctx.memory);
                    match dedup_gate::apply_enhance(
                        &store,
                        &target_node_id,
                        raw_idea,
                        &rationale,
                        ctx.dry_run,
                    )? {
                        true => report.enhanced += 1, // 0 new nodes
                        // The target vanished between shortlist and apply — never
                        // lose the idea; fall back to creating it.
                        false => self.create_candidate(raw_idea, &inputs, ctx, &mut report)?,
                    }
                }
                PlannedAction::FailClosed { reason } => {
                    // Fail-CLOSED: never silently create a duplicate on a broken
                    // reasoner. Drop this candidate this cycle; it is regenerated
                    // and retried next run. Surfaced (not swallowed).
                    report.dedup_errors += 1;
                    tracing::error!(
                        target: "creative_ideas",
                        reason = %reason,
                        "[simard] creative-ideas: dedup reasoner failed for one candidate — dropped this cycle (fail-closed), retried next run (#2925)"
                    );
                }
            }
        }

        // Per-tick telemetry (issue #2925): generated / skipped / enhanced /
        // created counts. `[simard]`-prefixed structured tracing — no println!.
        tracing::info!(
            target: "creative_ideas",
            generated = report.generated,
            skipped = report.skipped,
            enhanced = report.enhanced,
            created = report.persisted,
            dedup_errors = report.dedup_errors,
            "[simard] creative_ideas dedup: generated={} skipped={} enhanced={} created={}",
            report.generated,
            report.skipped,
            report.enhanced,
            report.persisted,
        );

        // Durability (issue #2798): no explicit checkpoint is needed here — the
        // pinned amplihack-memory engine's WAL is write-through and replayed on
        // open, so ideas persisted above survive a non-graceful daemon exit and
        // are recovered on reopen. Pinned by the durability regression tests; the
        // always-empty tab was the state-root resolver divergence, not a
        // durability defect (fixed in `routes::resolve_state_root`, D1).
        Ok(report)
    }

    /// CREATE one candidate: persist it as a fresh `New` idea (skipped under
    /// dry-run) and run the review/route pipeline. A single idea's review/route
    /// failure is logged (explicit telemetry — no silent fallback) and never
    /// aborts the batch.
    fn create_candidate(
        &self,
        raw_idea: &RawIdea,
        inputs: &GenerationInputs,
        ctx: &mut ThreadContext<'_>,
        report: &mut GenerationReport,
    ) -> SimardResult<()> {
        let mut idea = raw_to_creative_idea(raw_idea, inputs, ctx.now_epoch);

        if !ctx.dry_run {
            let store = ProspectiveCreativeIdeaStore::new(ctx.memory);
            idea.node_id = store.store(&idea)?;
            report.persisted += 1;
        }

        match self.pipeline.review_and_route(&mut idea, inputs, ctx) {
            Ok(RouteOutcome::Goal) => {
                report.reviewed += 1;
                report.routed_goal += 1;
            }
            Ok(RouteOutcome::Issue) => {
                report.reviewed += 1;
                report.routed_issue += 1;
            }
            Ok(RouteOutcome::Parked | RouteOutcome::DryRun) => {
                report.reviewed += 1;
            }
            Err(error) => {
                report.review_errors += 1;
                tracing::warn!(
                    target: "creative_ideas",
                    error = %error,
                    idea_id = %idea.idea_id,
                    "[simard] creative-ideas review/route failed for one idea"
                );
            }
        }
        Ok(())
    }

    /// Manual, on-demand generation entrypoint for the operator dashboard's
    /// **Run now** control.
    ///
    /// Runs ONE generation pass unconditionally — **bypassing the default-ON/opt-out
    /// `enabled()` gate and the 24h schedule** — and RETURNS the outcome (unlike
    /// the total [`Self::tick`], which folds errors into a `failed` outcome). Side
    /// effects are identical to a scheduled tick: it persists via `ctx.memory` and
    /// may route accepted ideas to goals/issues. Surfaces failures loudly as `Err`
    /// (never a silent no-op); honors `ctx.shutdown` and `ctx.dry_run`.
    pub fn run_now(&mut self, ctx: &mut ThreadContext<'_>) -> SimardResult<GenerationReport> {
        self.run_tick(ctx)
    }

    /// Assemble the (read-only) observation window from the goal board, recent
    /// journal/OODA activity, episodic memory, the Overseer's observations,
    /// conversation insights, and previously-generated ideas.
    ///
    /// Best-effort: each source that fails to read emits explicit `tracing`
    /// telemetry (never a silent fallback) and generation proceeds on whatever
    /// context is available — the generator reasons about partial context.
    fn assemble_inputs(&self, ctx: &ThreadContext<'_>) -> GenerationInputs {
        let mut inputs = GenerationInputs::default();

        match crate::goal_curation::load_goal_board(ctx.memory) {
            Ok(board) => {
                inputs.current_goals = board
                    .active
                    .iter()
                    .map(|g| g.description.clone())
                    .chain(board.backlog.iter().map(|b| b.description.clone()))
                    .filter(|d| !d.trim().is_empty())
                    .collect();
                inputs.works_in_progress = board
                    .active
                    .iter()
                    .filter_map(|g| {
                        let assignee = g.assigned_to.as_deref()?;
                        let activity = g.current_activity.as_deref().unwrap_or("in progress");
                        Some(format!("{} — {assignee}: {activity}", g.description))
                    })
                    .collect();
            }
            Err(error) => tracing::warn!(
                target: "creative_ideas",
                error = %error,
                "[simard] creative-ideas could not load the goal board for inputs"
            ),
        }

        match ctx.memory.list_all_episodes(EPISODE_SCAN_LIMIT) {
            Ok(episodes) => {
                inputs.episodic_summaries = episodes
                    .iter()
                    .take(OBSERVATION_LIMIT)
                    .map(|e| e.content.clone())
                    .filter(|c| !c.trim().is_empty())
                    .collect();
            }
            Err(error) => tracing::warn!(
                target: "creative_ideas",
                error = %error,
                "[simard] creative-ideas could not read episodic memory for inputs"
            ),
        }

        match crate::journal::store::all_entries(ctx.memory) {
            Ok(entries) => {
                let mut window = ActivityWindow {
                    window_secs: self.cfg.interval_secs,
                    entries: Vec::new(),
                };
                window.entries = entries
                    .iter()
                    .rev()
                    .take(OBSERVATION_LIMIT)
                    .filter(|e| !e.quiet_day && !e.narrative.trim().is_empty())
                    .map(|e| format!("{}: {}", e.date, e.narrative))
                    .collect();
                inputs.recent_activity = window;
            }
            Err(error) => tracing::warn!(
                target: "creative_ideas",
                error = %error,
                "[simard] creative-ideas could not read the journal for inputs"
            ),
        }

        let activity_path = ctx.state_root.join("overseer").join("activity.json");
        if let Some(activity) = crate::overseer::activity::read(&activity_path) {
            inputs.overseer_observations = activity
                .recent
                .iter()
                .take(OBSERVATION_LIMIT)
                .filter_map(|record| {
                    serde_json::to_string(&record.report)
                        .ok()
                        .map(|report| format!("{}: {report}", record.timestamp))
                })
                .collect();
        }

        match ctx.memory.search_episodes_by_keywords(
            &[
                "meeting".to_string(),
                "conversation".to_string(),
                "decision".to_string(),
            ],
            OBSERVATION_LIMIT as u32,
        ) {
            Ok(episodes) => {
                inputs.conversation_insights = episodes
                    .iter()
                    .map(|e| e.content.clone())
                    .filter(|c| !c.trim().is_empty())
                    .collect();
            }
            Err(error) => tracing::warn!(
                target: "creative_ideas",
                error = %error,
                "[simard] creative-ideas could not read conversation insights for inputs"
            ),
        }

        let store = ProspectiveCreativeIdeaStore::new(ctx.memory);
        match store.list(PREVIOUS_IDEAS_LIMIT) {
            Ok(previous) => inputs.previous_ideas = previous,
            Err(error) => tracing::warn!(
                target: "creative_ideas",
                error = %error,
                "[simard] creative-ideas could not load previous ideas for dedup"
            ),
        }

        inputs
    }

    fn record_success(&mut self, now_epoch: u64) {
        self.last_run_epoch = Some(now_epoch);
        self.next_run_epoch = Some(now_epoch.saturating_add(self.cfg.interval_secs));
        self.last_success = Some(true);
        self.consecutive_errors = 0;
    }

    fn record_failure(&mut self, now_epoch: u64) {
        self.last_run_epoch = Some(now_epoch);
        self.next_run_epoch = Some(now_epoch.saturating_add(self.cfg.interval_secs));
        self.last_success = Some(false);
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
    }
}

/// Structured result of one successful generation tick.
///
/// Public so the operator dashboard's manual "Run now" control
/// ([`CreativeIdeasThread::run_now`]) can report what a run produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct GenerationReport {
    /// Raw ideas the source produced this run.
    pub generated: usize,
    /// Ideas surviving the batch bound (before the semantic gate).
    pub surviving: usize,
    /// Ideas persisted to the store as `New` (0 under dry-run).
    pub persisted: usize,
    /// Ideas that completed review (regardless of routing target).
    pub reviewed: usize,
    /// Accepted ideas routed to a new goal.
    pub routed_goal: usize,
    /// Human-review-flagged ideas routed to an issue.
    pub routed_issue: usize,
    /// Per-idea review/route failures (logged, non-fatal).
    pub review_errors: usize,
    /// Candidates the semantic gate judged true duplicates and dropped (#2925).
    pub skipped: usize,
    /// Candidates merged into an existing idea (ENHANCE; 0 new nodes) (#2925).
    pub enhanced: usize,
    /// Candidates dropped this cycle because the dedup reasoner failed
    /// (fail-closed; surfaced, retried next run) (#2925).
    pub dedup_errors: usize,
    /// Whether the run was a dry-run (nothing persisted/routed). Internal-only;
    /// the dashboard "Run now" report never dry-runs, so it is not serialized.
    #[serde(skip)]
    pub dry_run: bool,
}

impl CognitiveThread for CreativeIdeasThread {
    fn id(&self) -> &str {
        CREATIVE_IDEAS_ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::BackgroundThought
    }

    fn purpose(&self) -> &'static str {
        "Generate divergent creative ideas in the background"
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::from_secs(self.cfg.interval_secs))
    }

    fn priority(&self) -> Priority {
        Priority::Low
    }

    fn enabled(&self) -> bool {
        self.cfg.enabled()
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let start = Instant::now();

        // Opt-out gate (default-ON): do no work when explicitly disabled.
        if !self.cfg.enabled() {
            return ThreadOutcome::skipped();
        }
        // Cooperative cancellation: bail promptly on shutdown.
        if ctx.shutdown.load(Ordering::Relaxed) {
            return ThreadOutcome::skipped();
        }

        match self.run_tick(ctx) {
            Ok(report) => {
                self.record_success(ctx.now_epoch);
                let verb = if report.dry_run {
                    "generated (dry-run)"
                } else {
                    "generated"
                };
                let summary = format!(
                    "creative_ideas: {verb} {} idea(s), {} survived batch, {} persisted, \
                     {} skipped, {} enhanced, {} reviewed ({} → goal, {} → issue), \
                     {} review error(s), {} dedup error(s)",
                    report.generated,
                    report.surviving,
                    report.persisted,
                    report.skipped,
                    report.enhanced,
                    report.reviewed,
                    report.routed_goal,
                    report.routed_issue,
                    report.review_errors,
                    report.dedup_errors,
                );
                let detail = json!({
                    "generated": report.generated,
                    "surviving": report.surviving,
                    "persisted": report.persisted,
                    "skipped": report.skipped,
                    "enhanced": report.enhanced,
                    "reviewed": report.reviewed,
                    "routed_goal": report.routed_goal,
                    "routed_issue": report.routed_issue,
                    "review_errors": report.review_errors,
                    "dedup_errors": report.dedup_errors,
                    "dry_run": report.dry_run,
                });
                ThreadOutcome::ok(summary, start.elapsed()).with_detail(detail)
            }
            Err(error) => {
                tracing::warn!(
                    target: "creative_ideas",
                    error = %error,
                    "[simard] creative-ideas tick failed"
                );
                self.record_failure(ctx.now_epoch);
                ThreadOutcome::failed(
                    format!("creative-ideas tick failed: {error}"),
                    start.elapsed(),
                )
            }
        }
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: CREATIVE_IDEAS_ID.to_string(),
            enabled: self.cfg.enabled(),
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            consecutive_errors: self.consecutive_errors,
            backoff_until_epoch: None,
            purpose: self.purpose().to_string(),
            cadence_secs: self.policy().cadence_secs(),
        }
    }
}

/// Convert a raw idea into a `New` [`CreativeIdea`] carrying its links + context.
fn raw_to_creative_idea(raw: &RawIdea, inputs: &GenerationInputs, now_epoch: u64) -> CreativeIdea {
    let context = IdeaContext {
        source: CREATIVE_IDEAS_ID.to_string(),
        goals_snapshot: inputs.current_goals.clone(),
        observation_digest: format!("entries={}", inputs.recent_activity.entries.len()),
        rationale: raw.rationale.clone(),
    };
    let mut idea = CreativeIdea::new(raw.idea.clone(), context, now_epoch);
    idea.links = raw.links.clone();
    idea
}

/// Build the semantic dedup + enhance reasoner for the production thread (issue
/// #2925). Tries the recipe-runner-backed [`crate::ooda_brain::RecipeBrain`]
/// bound to `creative-idea-dedup.yaml`; when that is unavailable (no
/// recipe-runner-rs, no agent binary, or the recipe asset is missing) it falls
/// back **loudly** to a deterministic word-set brain with the semantic layer
/// OFF — never silently. The returned `bool` is whether the agentic layer is
/// active (also gated by the `SIMARD_CREATIVE_IDEAS_SEMANTIC_DEDUP` kill-switch).
fn build_dedup_brain() -> (Box<dyn OodaBrain>, bool) {
    let repo_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match crate::ooda_brain::RecipeBrain::new(
        &repo_root,
        "creative-idea-dedup.yaml",
        "recipe-idea-dedup-brain",
    ) {
        Some(brain) => {
            let enabled = dedup_gate::semantic_dedup_enabled();
            tracing::info!(
                target: "creative_ideas",
                semantic_enabled = enabled,
                "[simard] creative-ideas: semantic dedup = RecipeBrain (creative-idea-dedup.yaml) (#2925)"
            );
            (Box::new(brain), enabled)
        }
        None => {
            tracing::warn!(
                target: "creative_ideas",
                "[simard] creative-ideas: semantic dedup recipe/brain unavailable — falling back to deterministic word-set dedup (no enhance) (#2925)"
            );
            (Box::new(DeterministicLifecycleBrain), false)
        }
    }
}

/// Register the Creative Ideas thread with the `Mind` scheduler.
///
/// The daemon reaches this through [`register_creative_ideas_if_enabled`], which
/// owns the default-ON/opt-out gate. Builds the production idea source +
/// review/route pipeline + semantic dedup reasoner (#2925); the thread's
/// `enabled()` gate is defence-in-depth that still makes an opted-out thread
/// inert even if it were registered directly.
pub fn register(mind: &mut Mind, config: CreativeIdeasConfig) {
    let (brain, semantic_enabled) = build_dedup_brain();
    let thread = CreativeIdeasThread::with_pipeline_and_brain(
        config,
        Box::new(AgenticIdeaSource::from_env()),
        Box::new(AgenticIdeaPipeline::from_env()),
        brain,
        semantic_enabled,
    );
    mind.register(Box::new(thread));
}

/// Register the Creative Ideas thread **only when its config gate is ON**,
/// returning whether it was registered.
///
/// This is the daemon's startup seam (issue #2647). It mirrors the
/// Overseer/Journal default-ON opt-out pattern: an opted-out subsystem registers
/// nothing, so "opted out ⇒ not registered" is a direct `mind.len()` /
/// `mind.health()` assertion rather than relying on a registered-but-inert
/// thread. The gate is independent of the generic `SIMARD_COGNITIVE_THREADS_ENABLED`
/// master switch.
pub fn register_creative_ideas_if_enabled(mind: &mut Mind, cfg: &CreativeIdeasConfig) -> bool {
    if cfg.enabled() {
        register(mind, cfg.clone());
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_if_enabled_registers_when_default_on() {
        let mut mind = Mind::new();
        let registered =
            register_creative_ideas_if_enabled(&mut mind, &CreativeIdeasConfig::default());
        assert!(registered, "default-ON config must register the thread");
        assert_eq!(mind.len(), 1);
    }

    #[test]
    fn register_if_enabled_skips_when_opted_out() {
        let mut mind = Mind::new();
        let cfg = CreativeIdeasConfig {
            enabled: false,
            ..CreativeIdeasConfig::default()
        };
        let registered = register_creative_ideas_if_enabled(&mut mind, &cfg);
        assert!(!registered, "opted-out config must register nothing");
        assert!(mind.is_empty());
    }

    #[test]
    fn register_if_enabled_registers_when_env_unset() {
        // Env seam (hermetic — no real process env): an unset
        // `SIMARD_CREATIVE_IDEAS_ENABLED` is default-ON, so the daemon's startup
        // seam registers the thread. Drives the full env → gate → register path.
        let cfg = CreativeIdeasConfig::from_lookup(|_| None);
        let mut mind = Mind::new();
        let registered = register_creative_ideas_if_enabled(&mut mind, &cfg);
        assert!(registered, "unset env is default-ON ⇒ thread registered");
        assert_eq!(mind.len(), 1);
    }

    #[test]
    fn register_if_enabled_skips_when_disabled_via_env() {
        // Env seam (hermetic): `SIMARD_CREATIVE_IDEAS_ENABLED=0` opts out, so the
        // startup seam registers nothing — "disabled via env ⇒ not registered".
        let cfg = CreativeIdeasConfig::from_lookup(|k| {
            (k == crate::creative_ideas::ENABLED_ENV).then(|| "0".to_string())
        });
        let mut mind = Mind::new();
        let registered = register_creative_ideas_if_enabled(&mut mind, &cfg);
        assert!(
            !registered,
            "SIMARD_CREATIVE_IDEAS_ENABLED=0 ⇒ nothing registered"
        );
        assert!(mind.is_empty());
    }
}
