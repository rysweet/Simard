//! Cognitive thread 3 (issue #5) — [`ReflectionThread`]: post-mortem completed goals and verified failures into `postmortem:` facts and, on recurrence, `lesson:` procedures via the existing reflection_lessons path.
//!
//! A thin `CognitiveThread` rail over the agentic recipe `reflect-postmortem`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `postmortem:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5):** implemented; [`ReflectionThread::tick`] is covered by
//! the hermetic offline unit tests in `tests_catalog` (fake recipe invoker)
//! plus a gated live-smoke check.
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
pub const ID: &str = "reflection";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "reflect-postmortem";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_REFLECTION_ENABLED";

/// Tunables for [`ReflectionThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflectionConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            interval_secs: 5400,
            dry_run: false,
        }
    }
}

/// Cognitive thread 3.
pub struct ReflectionThread {
    cfg: ReflectionConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl ReflectionThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = ReflectionConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_REFLECTION_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: ReflectionConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

    /// Cheap guard substituting for the unavailable `EventDriven` trigger: on the
    /// very first activation (no prior run) reflection always proceeds; on later
    /// ticks it only proceeds when the board actually holds work to reflect on,
    /// otherwise the tick is a near-zero-cost `skipped()`.
    fn has_reflectable_work(&self, ctx: &ThreadContext<'_>) -> bool {
        self.last_run_epoch.is_none()
            || !crate::goal_board_store::load(ctx.state_root)
                .board
                .active
                .is_empty()
    }
}

impl CognitiveThread for ReflectionThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Reflection
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

        // Interval-with-guard: skip cheaply when there is nothing to reflect on.
        if !self.has_reflectable_work(ctx) {
            return ThreadOutcome::skipped();
        }

        // INPUT — prior post-mortems (fenced as untrusted) + board digest.
        let prior = ctx
            .memory
            .search_facts("postmortem:", 20, 0.0)
            .map(|facts| {
                facts
                    .iter()
                    .map(|f| format!("{}: {}", f.concept, f.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let ctx_vars: Vec<(&str, String)> = vec![
            ("state_root", ctx.state_root.display().to_string()),
            ("repo_path", ctx.repo_root.display().to_string()),
            ("prior_postmortems", recipe_rail::fence_untrusted(&prior)),
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
        let postmortem = envelope
            .get("postmortem")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let goal_type = envelope
            .get("goal_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let error_class = envelope.get("error_class").and_then(|v| v.as_str());
        let type_key =
            recipe_rail::validate_concept_key(goal_type).unwrap_or_else(|| "general".to_string());

        // WRITE — a durable `postmortem:<goal_type>` fact.
        if !dry_run {
            let concept = format!("postmortem:{type_key}");
            let content = recipe_rail::secret_scrub(postmortem);
            if let Err(e) =
                ctx.memory
                    .store_fact(&concept, &content, 0.7, &["postmortem".to_string()], ID)
            {
                self.note_run(ctx.now_epoch, false);
                return ThreadOutcome::failed(
                    format!("postmortem fact write failed: {e}"),
                    start.elapsed(),
                );
            }
        }

        // On a classified failure with concrete steps, distil a durable lesson
        // procedure (best-effort — the fact above is the load-bearing output).
        let mut lesson_written = false;
        if !dry_run
            && let Some(error_class) = error_class
            && let Some(steps) = envelope.get("lesson_steps").and_then(|v| v.as_array())
        {
            let steps: Vec<String> = steps
                .iter()
                .filter_map(|s| s.as_str())
                .map(recipe_rail::secret_scrub)
                .collect();
            if !steps.is_empty()
                && let Some(class_key) = recipe_rail::validate_concept_key(error_class)
            {
                let name = format!("lesson:{type_key}:{class_key}");
                lesson_written = ctx.memory.store_procedure(&name, &steps, &[]).is_ok();
            }
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("reflection: postmortem for `{type_key}`, lesson={lesson_written}"),
            start.elapsed(),
        )
        .with_detail(json!({
            "goal_type": type_key,
            "error_class": error_class,
            "lesson_written": lesson_written,
            "dry_run": dry_run,
        }))
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
