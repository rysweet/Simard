//! Cognitive thread 1 (issue #5) — [`MetacognitionThread`]: compare stated confidence vs actual outcome, scan for error/bias signatures, publish calibration + decision-quality self-metrics and `metacog:` facts, and propose <=1 recalibration goal on threshold.
//!
//! A thin `CognitiveThread` rail over the agentic recipe `metacognition-appraise`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `metacog:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5, TDD):** the config/type surface + the constructors are
//! real studs; [`MetacognitionThread::tick`] is a `todo!()` pinned RED by
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
pub const ID: &str = "metacognition";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "metacognition-appraise";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_METACOGNITION_ENABLED";

/// Tunables for [`MetacognitionThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetacognitionConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for MetacognitionConfig {
    fn default() -> Self {
        Self {
            interval_secs: 3600,
            dry_run: false,
        }
    }
}

/// Cognitive thread 1.
pub struct MetacognitionThread {
    cfg: MetacognitionConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl MetacognitionThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = MetacognitionConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_METACOGNITION_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: MetacognitionConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
        Self {
            cfg,
            invoker,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }

    /// Update advisory heartbeat bookkeeping after a tick (the `Mind` keeps the
    /// authoritative copy; this feeds the thread's own `health()`).
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

impl CognitiveThread for MetacognitionThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Metacognition
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

        // INPUT — assemble read-only context and fence prior facts as untrusted.
        let prior = ctx
            .memory
            .search_facts("metacog:", 20, 0.0)
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
            ("prior_metacognition", recipe_rail::fence_untrusted(&prior)),
        ];

        // INVOKE — no-silent-degradation: both misses short-circuit to failed().
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

        // WRITE — one `metacog:<pattern>` fact per detected pattern (key-
        // validated, secret-scrubbed). A failed durable write fails the tick.
        let mut written = 0usize;
        if let Some(patterns) = envelope.get("patterns").and_then(|v| v.as_array()) {
            for p in patterns {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let evidence = p.get("evidence").and_then(|v| v.as_str()).unwrap_or("");
                let Some(key) = recipe_rail::validate_concept_key(name) else {
                    continue;
                };
                if dry_run {
                    written += 1;
                    continue;
                }
                let concept = format!("metacog:{key}");
                let content = recipe_rail::secret_scrub(evidence);
                if let Err(e) = ctx.memory.store_fact(
                    &concept,
                    &content,
                    0.7,
                    &["metacognition".to_string()],
                    ID,
                ) {
                    self.note_run(ctx.now_epoch, false);
                    return ThreadOutcome::failed(
                        format!("metacog fact write failed: {e}"),
                        start.elapsed(),
                    );
                }
                written += 1;
            }
        }

        // Self-metrics feed the hybrid self-measurement goal LIVE. Best-effort:
        // the durable self-metric sink lives under $HOME and a failure to append
        // must never turn a successful appraisal into a failed tick.
        let calibration_error = envelope.get("calibration_error").and_then(|v| v.as_f64());
        let decision_quality = envelope.get("decision_quality").and_then(|v| v.as_f64());
        if !dry_run {
            if let Some(ce) = calibration_error {
                let _ = crate::self_metrics::record_metric("confidence_calibration_error", ce, ID);
            }
            if let Some(dq) = decision_quality {
                let _ = crate::self_metrics::record_metric("decision_quality", dq, ID);
            }
        }

        self.note_run(ctx.now_epoch, true);
        let summary = format!(
            "metacognition: {written} pattern(s), calibration_error={:?}, decision_quality={:?}",
            calibration_error, decision_quality
        );
        ThreadOutcome::ok(summary, start.elapsed()).with_detail(json!({
            "patterns_written": written,
            "calibration_error": calibration_error,
            "decision_quality": decision_quality,
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
