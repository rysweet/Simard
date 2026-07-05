//! The Creative Ideas generator thread (design spike #2419).
//!
//! `CreativeIdeasThread` implements [`CognitiveThread`] and reuses
//! [`ThreadKind::BackgroundThought`] (Decision 2). It is **gated OFF by
//! default** (`enabled()` reads [`CreativeIdeasConfig::enabled`], default
//! `false`), so the scheduler never ticks it unless explicitly turned on. It is
//! **not registered** with the `Mind` during the spike — [`register`] is a
//! marked `// FUTURE:` seam.
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
use crate::error::{SimardError, SimardResult};

/// Stable telemetry id.
pub const CREATIVE_IDEAS_ID: &str = "creative_ideas";

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

/// Production idea source — a marked `// FUTURE:` stub for the spike.
///
/// FUTURE (M2): the real LLM-backed generator producing diverse ideas from the
/// observation window. Until then `generate` returns
/// [`SimardError::ReviewUnavailable`]; the thread is gated OFF so it never runs.
#[derive(Clone, Debug, Default)]
pub struct LlmIdeaSource;

impl IdeaSource for LlmIdeaSource {
    fn generate(&self, _inputs: &GenerationInputs, _n: usize) -> SimardResult<Vec<RawIdea>> {
        Err(SimardError::ReviewUnavailable {
            reason: "LlmIdeaSource is not wired during the spike (M2)".to_string(),
        })
    }
}

/// The Creative Ideas generator cognitive thread.
pub struct CreativeIdeasThread {
    cfg: CreativeIdeasConfig,
    source: Box<dyn IdeaSource>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl CreativeIdeasThread {
    /// Build the thread from an explicit config + idea source (test seam).
    #[must_use]
    pub fn new(cfg: CreativeIdeasConfig, source: Box<dyn IdeaSource>) -> Self {
        Self {
            cfg,
            source,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }

    /// Build from the environment with the production (FUTURE) idea source.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(CreativeIdeasConfig::from_env(), Box::new(LlmIdeaSource))
    }

    /// The core, fallible tick body. `tick` wraps this and folds any `Err` into
    /// a [`ThreadOutcome::failed`] so the public contract stays infallible.
    fn run_tick(&mut self, ctx: &mut ThreadContext<'_>) -> SimardResult<GenerationReport> {
        let inputs = self.assemble_inputs(ctx);

        let raw = self.source.generate(&inputs, self.cfg.batch)?;
        let generated = raw.len();

        let deduped =
            dedup::reject_duplicates(raw, &inputs.previous_ideas, dedup::DEFAULT_DEDUP_THRESHOLD);
        let selected = dedup::select_balanced(deduped, self.cfg.batch);
        let surviving = selected.len();

        let mut persisted = 0usize;
        if !ctx.dry_run {
            let store = ProspectiveCreativeIdeaStore::new(ctx.memory);
            for raw_idea in &selected {
                if ctx.shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let idea = raw_to_creative_idea(raw_idea, &inputs, ctx.now_epoch);
                store.store(&idea)?;
                persisted += 1;
            }
        }

        Ok(GenerationReport {
            generated,
            surviving,
            persisted,
            dry_run: ctx.dry_run,
        })
    }

    /// Assemble the (read-only) observation window.
    ///
    /// FUTURE (M2): populate from the goal store / Journal / OODA / Overseer /
    /// conversation insights / previous ideas. During the spike this is empty
    /// so the gated-OFF thread has no external reads.
    fn assemble_inputs(&self, _ctx: &ThreadContext<'_>) -> GenerationInputs {
        GenerationInputs::default()
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
struct GenerationReport {
    generated: usize,
    surviving: usize,
    persisted: usize,
    dry_run: bool,
}

impl CognitiveThread for CreativeIdeasThread {
    fn id(&self) -> &str {
        CREATIVE_IDEAS_ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::BackgroundThought
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

        // Gated OFF: never do work unless explicitly enabled.
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
                    "creative_ideas: {verb} {} idea(s), {} survived dedup, {} persisted",
                    report.generated, report.surviving, report.persisted,
                );
                let detail = json!({
                    "generated": report.generated,
                    "surviving": report.surviving,
                    "persisted": report.persisted,
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

/// Register the thread with the `Mind` scheduler.
///
/// FUTURE (M2): call site in the daemon `main`, still behind
/// `SIMARD_CREATIVE_IDEAS_ENABLED`. **NOT** called during the spike; the
/// production idea source ([`LlmIdeaSource`]) is itself a not-wired stub, so a
/// registered-but-enabled thread would only ever emit `ThreadOutcome::failed`.
pub fn register(mind: &mut Mind, config: CreativeIdeasConfig) {
    let thread = CreativeIdeasThread::new(config, Box::new(LlmIdeaSource));
    mind.register(Box::new(thread));
}
