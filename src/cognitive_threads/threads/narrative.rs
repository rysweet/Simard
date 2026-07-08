//! Cognitive thread 12 (issue #5) — [`NarrativeThread`]: maintain a singleton `narrative:identity` fact (supersede-in-place) plus append-only `narrative:chapter:<epoch>` milestones for identity continuity.
//!
//! A thin `CognitiveThread` rail over the agentic recipe `narrative-identity`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `narrative:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5, TDD):** the config/type surface + the constructors are
//! real studs; [`NarrativeThread::tick`] is a `todo!()` pinned RED by
//! `tests_catalog` until the implementation step fills it in.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde_json::json;

use super::super::recipe_rail::{self, RecipeInvoker};
use super::super::schedule;
use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};

/// Stable telemetry id.
pub const ID: &str = "narrative";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "narrative-identity";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_NARRATIVE_ENABLED";

/// Tunables for [`NarrativeThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NarrativeConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for NarrativeConfig {
    fn default() -> Self {
        Self {
            interval_secs: 43200,
            dry_run: false,
        }
    }
}

/// Cognitive thread 12.
pub struct NarrativeThread {
    cfg: NarrativeConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl NarrativeThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = NarrativeConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_NARRATIVE_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: NarrativeConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
        Self {
            cfg,
            invoker,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }

    /// Update advisory heartbeat bookkeeping after a tick.
    fn note_run(&mut self, now_epoch: u64, success: bool) {
        self.last_run_epoch = Some(now_epoch);
        self.next_run_epoch =
            schedule::next_run_epoch(&self.policy(), self.last_run_epoch, now_epoch);
        self.last_success = Some(success);
        if success {
            self.consecutive_errors = 0;
        } else {
            self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        }
    }
}

impl CognitiveThread for NarrativeThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Narrative
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::from_secs(schedule::clamp_interval_secs(
            self.cfg.interval_secs,
        )))
    }

    fn priority(&self) -> Priority {
        Priority::Low
    }

    fn enabled(&self) -> bool {
        recipe_rail::thread_enabled(GATE_ENV)
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let start = Instant::now();

        // INPUT — the current self-story + recent chapters, fenced as untrusted.
        let prior_identity = ctx
            .memory
            .search_facts("narrative:identity", 1, 0.0)
            .ok()
            .and_then(|f| f.into_iter().next())
            .map(|f| f.content)
            .unwrap_or_default();
        let ctx_vars: Vec<(&str, String)> = vec![
            ("state_root", ctx.state_root.display().to_string()),
            ("repo_path", ctx.repo_root.display().to_string()),
            (
                "prior_identity",
                recipe_rail::fence_untrusted(&prior_identity),
            ),
        ];

        let envelope =
            match recipe_rail::invoke_for_envelope(self.invoker.as_ref(), RECIPE, &ctx_vars, start)
            {
                Ok(v) => v,
                Err(failed) => {
                    self.note_run(ctx.now_epoch, false);
                    return failed;
                }
            };

        let dry_run = ctx.dry_run || self.cfg.dry_run;
        let identity = envelope
            .get("identity")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // WRITE — the identity fact is a SINGLETON: superseded in place through a
        // stable caller key so it is never duplicated across ticks.
        if !dry_run && !identity.trim().is_empty() {
            let content = recipe_rail::secret_scrub(identity);
            if let Err(e) = ctx.memory.store_fact_with_caller_key(
                "narrative:identity",
                "narrative:identity",
                &content,
                0.9,
                &["narrative".to_string()],
                ID,
            ) {
                self.note_run(ctx.now_epoch, false);
                return ThreadOutcome::failed(
                    format!("identity fact write failed: {e}"),
                    start.elapsed(),
                );
            }
        }

        // An optional new chapter is an APPEND-only, per-epoch fact (not a
        // singleton) — the story's continuity across time.
        let mut chapter_written = false;
        if let Some(chapter) = envelope.get("new_chapter").and_then(|v| v.as_str())
            && !dry_run
            && !chapter.trim().is_empty()
        {
            let concept = format!("narrative:chapter:{}", ctx.now_epoch);
            chapter_written = ctx
                .memory
                .store_fact(
                    &concept,
                    &recipe_rail::secret_scrub(chapter),
                    0.7,
                    &["narrative".to_string()],
                    ID,
                )
                .is_ok();
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("narrative: identity refreshed, chapter={chapter_written}"),
            start.elapsed(),
        )
        .with_detail(json!({ "chapter_written": chapter_written, "dry_run": dry_run }))
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: ID.to_string(),
            enabled: self.enabled(),
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            consecutive_errors: self.consecutive_errors,
            backoff_until_epoch: None,
        }
    }
}

fn read_u64_env(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}
