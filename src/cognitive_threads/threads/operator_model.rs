//! Cognitive thread 8 (issue #5) — [`OperatorModelThread`]: grow a live operator model into `operator:<trait>` facts (secret-scrubbed, supersede-in-place), seeded read-only from USER_PREFERENCES.md.
//!
//! A thin `CognitiveThread` rail over the agentic recipe `operator-model`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `operator:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5):** implemented; [`OperatorModelThread::tick`] is covered
//! by the hermetic offline unit tests in `tests_catalog` (fake recipe invoker)
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
pub const ID: &str = "operator_model";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "operator-model";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_OPERATOR_MODEL_ENABLED";

/// Tunables for [`OperatorModelThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorModelConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for OperatorModelConfig {
    fn default() -> Self {
        Self {
            interval_secs: 7200,
            dry_run: false,
        }
    }
}

/// Cognitive thread 8.
pub struct OperatorModelThread {
    cfg: OperatorModelConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl OperatorModelThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = OperatorModelConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_OPERATOR_MODEL_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: OperatorModelConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

impl CognitiveThread for OperatorModelThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::OperatorModel
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

        // INPUT — prior operator model (fenced as untrusted) for update-in-place.
        let prior = ctx
            .memory
            .search_facts("operator:", 30, 0.0)
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
            ("prior_operator_model", recipe_rail::fence_untrusted(&prior)),
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

        // WRITE — `operator:<trait>` facts, superseded in place (the model
        // UPDATES, not accretes). Every value+evidence is secret-scrubbed so a
        // token in a source episode is never echoed into a stored fact (S5).
        let mut written = 0usize;
        if let Some(prefs) = envelope.get("preferences").and_then(|v| v.as_array()) {
            for p in prefs {
                let trait_name = p.get("trait").and_then(|v| v.as_str()).unwrap_or("");
                let value = p.get("value").and_then(|v| v.as_str()).unwrap_or("");
                let confidence = p.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);
                let evidence = p.get("evidence").and_then(|v| v.as_str()).unwrap_or("");
                let Some(key) = recipe_rail::validate_concept_key(trait_name) else {
                    continue;
                };
                if dry_run {
                    written += 1;
                    continue;
                }
                let concept = format!("operator:{key}");
                let content =
                    recipe_rail::secret_scrub(&format!("{} (evidence: {})", value, evidence));
                if let Err(e) = ctx.memory.store_fact_with_caller_key(
                    &concept,
                    &concept,
                    &content,
                    confidence.clamp(0.0, 1.0),
                    &["operator".to_string()],
                    ID,
                ) {
                    self.note_run(ctx.now_epoch, false);
                    return ThreadOutcome::failed(
                        format!("operator fact write failed: {e}"),
                        start.elapsed(),
                    );
                }
                written += 1;
            }
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("operator_model: {written} preference(s)"),
            start.elapsed(),
        )
        .with_detail(json!({ "preferences_written": written, "dry_run": dry_run }))
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
