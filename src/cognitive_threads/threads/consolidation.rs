//! Cognitive thread 2 (issue #5) — [`ConsolidationThread`]: replay undistilled episodes -> facts/procedures, form `schema:` facts, and advise (dry-run/class-protected/logged) forgetting of low-value memory; distillation itself reuses `distill-episodes.yaml`.
//!
//! A thin `CognitiveThread` rail over the agentic recipe `consolidate-sleep`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `schema:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5):** implemented; [`ConsolidationThread::tick`] is covered
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
pub const ID: &str = "consolidation";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "consolidate-sleep";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_CONSOLIDATION_ENABLED";

/// Tunables for [`ConsolidationThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsolidationConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            interval_secs: 21600,
            dry_run: false,
        }
    }
}

/// Cognitive thread 2.
pub struct ConsolidationThread {
    cfg: ConsolidationConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl ConsolidationThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = ConsolidationConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_CONSOLIDATION_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: ConsolidationConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

impl CognitiveThread for ConsolidationThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::MemoryConsolidation
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

        // INPUT — memory pressure + prior schemas (fenced as untrusted).
        let stats = ctx.memory.get_statistics().ok();
        let prior = ctx
            .memory
            .search_facts("schema:", 20, 0.0)
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
            (
                "episodic_count",
                stats
                    .as_ref()
                    .map(|s| s.episodic_count)
                    .unwrap_or(0)
                    .to_string(),
            ),
            ("prior_schemas", recipe_rail::fence_untrusted(&prior)),
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

        // Deepen the existing distillation loop (reused, never reimplemented) and
        // run forgetting in ADVISORY dry-run only — thread-initiated forgetting is
        // never a single-pass delete (invariant S4). Both best-effort.
        if !dry_run {
            let _ = ctx.memory.consolidate_episodes(20);
            let _ = ctx.memory.forget_low_value_facts(true);
        }

        // WRITE — one `schema:<cluster>` fact per higher-order schema.
        let mut written = 0usize;
        if let Some(schemas) = envelope.get("schemas").and_then(|v| v.as_array()) {
            for s in schemas {
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let summary = s.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                let members = s
                    .get("member_concepts")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let Some(key) = recipe_rail::validate_concept_key(name) else {
                    continue;
                };
                if dry_run {
                    written += 1;
                    continue;
                }
                let concept = format!("schema:{key}");
                let content = recipe_rail::secret_scrub(&format!("{summary} [members: {members}]"));
                if let Err(e) = ctx.memory.store_fact_with_caller_key(
                    &concept,
                    &concept,
                    &content,
                    0.6,
                    &["schema".to_string()],
                    ID,
                ) {
                    self.note_run(ctx.now_epoch, false);
                    return ThreadOutcome::failed(
                        format!("schema fact write failed: {e}"),
                        start.elapsed(),
                    );
                }
                written += 1;
            }
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("consolidation: {written} schema(s)"),
            start.elapsed(),
        )
        .with_detail(json!({ "schemas_written": written, "dry_run": dry_run }))
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
