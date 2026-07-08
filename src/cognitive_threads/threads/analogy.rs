//! Cognitive thread 9 (issue #5) — [`AnalogyThread`]: mine memory for structural cross-domain mappings into `analogy:<target>` facts (validated concept keys), possibly reinforcing a recalled procedure.
//!
//! A thin `CognitiveThread` rail over the agentic recipe `analogy-map`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `analogy:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5, TDD):** the config/type surface + the constructors are
//! real studs; [`AnalogyThread::tick`] is a `todo!()` pinned RED by
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
pub const ID: &str = "analogy";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "analogy-map";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_ANALOGY_ENABLED";

/// Tunables for [`AnalogyThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalogyConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for AnalogyConfig {
    fn default() -> Self {
        Self {
            interval_secs: 9000,
            dry_run: false,
        }
    }
}

/// Cognitive thread 9.
pub struct AnalogyThread {
    cfg: AnalogyConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl AnalogyThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = AnalogyConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_ANALOGY_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: AnalogyConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

impl CognitiveThread for AnalogyThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Analogy
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

        // INPUT — recallable source material + prior analogies, fenced.
        let sources = ctx
            .memory
            .search_facts("bug-pattern", 15, 0.0)
            .map(|facts| {
                facts
                    .iter()
                    .map(|f| format!("{}: {}", f.concept, f.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let prior = ctx
            .memory
            .search_facts("analogy:", 20, 0.0)
            .map(|facts| {
                facts
                    .iter()
                    .map(|f| f.content.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let ctx_vars: Vec<(&str, String)> = vec![
            ("state_root", ctx.state_root.display().to_string()),
            ("source_material", recipe_rail::fence_untrusted(&sources)),
            ("prior_analogies", recipe_rail::fence_untrusted(&prior)),
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

        // WRITE — one `analogy:<target>` fact per structural mapping. The
        // LLM-derived target is key-validated (S6: rejected on a separator/`..`).
        let mut written = 0usize;
        if let Some(analogies) = envelope.get("analogies").and_then(|v| v.as_array()) {
            for a in analogies {
                let source = a.get("source").and_then(|v| v.as_str()).unwrap_or("");
                let target = a.get("target").and_then(|v| v.as_str()).unwrap_or("");
                let mapping = a
                    .get("structural_mapping")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let insight = a
                    .get("transferable_insight")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let Some(key) = recipe_rail::validate_concept_key(target) else {
                    continue;
                };
                if dry_run {
                    written += 1;
                    continue;
                }
                let concept = format!("analogy:{key}");
                let content = recipe_rail::secret_scrub(&format!(
                    "{source} → {target}: {mapping} | {insight}"
                ));
                if let Err(e) = ctx.memory.store_fact_with_caller_key(
                    &concept,
                    &concept,
                    &content,
                    0.55,
                    &["analogy".to_string()],
                    ID,
                ) {
                    self.note_run(ctx.now_epoch, false);
                    return ThreadOutcome::failed(
                        format!("analogy fact write failed: {e}"),
                        start.elapsed(),
                    );
                }
                written += 1;
            }
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(format!("analogy: {written} mapping(s)"), start.elapsed())
            .with_detail(json!({ "analogies_written": written, "dry_run": dry_run }))
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
