//! Cognitive thread 7 (issue #5) — [`SalienceThread`]: appraise what matters most now; write the free-text reason to durable `salience:<goal_id>` facts and the numeric-only validated ranking to state/salience_signal.json (S1).
//!
//! A thin `CognitiveThread` rail over the agentic recipe `salience-appraise`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `salience:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5):** implemented; [`SalienceThread::tick`] is covered by
//! the hermetic offline unit tests in `tests_catalog` (fake recipe invoker)
//! plus a gated live-smoke check.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde_json::json;

use super::super::recipe_rail::{self, RecipeInvoker};
use super::super::salience_signal::{self, SalienceEntry, SalienceSignal};
use super::super::schedule;
use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};

/// Stable telemetry id.
pub const ID: &str = "salience";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "salience-appraise";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_SALIENCE_ENABLED";

/// Tunables for [`SalienceThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SalienceConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for SalienceConfig {
    fn default() -> Self {
        Self {
            interval_secs: 1800,
            dry_run: false,
        }
    }
}

/// Cognitive thread 7.
pub struct SalienceThread {
    cfg: SalienceConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl SalienceThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = SalienceConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_SALIENCE_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: SalienceConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

impl CognitiveThread for SalienceThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Salience
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::from_secs(schedule::clamp_interval_secs(
            self.cfg.interval_secs,
        )))
    }

    fn priority(&self) -> Priority {
        Priority::Normal
    }

    fn enabled(&self) -> bool {
        recipe_rail::thread_enabled(GATE_ENV)
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let start = Instant::now();

        // INPUT — active goals (the appraisal targets) + health facts, fenced.
        let board = crate::goal_board_store::load(ctx.state_root).board;
        let goals = board
            .active
            .iter()
            .map(|g| format!("{}: {}", g.id, g.description))
            .collect::<Vec<_>>()
            .join("\n");
        let health = ctx
            .memory
            .search_facts("interocept:", 10, 0.0)
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
            ("active_goals", recipe_rail::fence_untrusted(&goals)),
            ("health_facts", recipe_rail::fence_untrusted(&health)),
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
        if dry_run {
            self.note_run(ctx.now_epoch, true);
            return ThreadOutcome::ok("salience: dry-run", start.elapsed());
        }

        // WRITE — the two projections of one appraisal (S1):
        //  1) durable `salience:<goal_id>` facts hold the free-text `reason`;
        //  2) a numeric-only signal file feeds OODA Decide.
        let mut entries: Vec<SalienceEntry> = Vec::new();
        let mut facts_written = 0usize;
        if let Some(ranking) = envelope.get("ranking").and_then(|v| v.as_array()) {
            for e in ranking {
                let goal_id = e.get("goal_id").and_then(|v| v.as_str()).unwrap_or("");
                let valence = e.get("valence").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let urgency = e.get("urgency").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let reason = e.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                let Some(key) = recipe_rail::validate_concept_key(goal_id) else {
                    continue;
                };
                // Free-text rationale → durable fact (NEVER routed into a prompt).
                let concept = format!("salience:{key}");
                let content = recipe_rail::secret_scrub(reason);
                if let Err(e) = ctx.memory.store_fact_with_caller_key(
                    &concept,
                    &concept,
                    &content,
                    0.5,
                    &["salience".to_string()],
                    ID,
                ) {
                    self.note_run(ctx.now_epoch, false);
                    return ThreadOutcome::failed(
                        format!("salience fact write failed: {e}"),
                        start.elapsed(),
                    );
                }
                facts_written += 1;
                entries.push(
                    SalienceEntry {
                        goal_id: key,
                        valence,
                        urgency,
                    }
                    .clamped(),
                );
            }
        }

        // Numeric-only Decide projection: only ids validated against the live
        // board reach the file (S1). Absent board => empty ranking, file present.
        let valid_ids: Vec<String> = board.active.iter().map(|g| g.id.clone()).collect();
        let signal = SalienceSignal {
            generated_epoch: ctx.now_epoch,
            ranking: entries,
        };
        if let Err(e) = salience_signal::write_signal(ctx.state_root, &signal, &valid_ids) {
            self.note_run(ctx.now_epoch, false);
            return ThreadOutcome::failed(
                format!("salience signal write failed: {e}"),
                start.elapsed(),
            );
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("salience: {facts_written} rationale fact(s), signal written"),
            start.elapsed(),
        )
        .with_detail(json!({ "facts_written": facts_written }))
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
