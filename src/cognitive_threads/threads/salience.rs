//! Cognitive thread 7 (issue #5) — [`SalienceThread`]: appraise what matters most now. The recipe itself writes the free-text reason to durable `salience:<goal_id>` facts (`simard memory remember`) and the numeric-only validated ranking to state/salience_signal.json (`simard cognition salience-signal`) (S1).
//!
//! A thin `CognitiveThread` rail over the agentic recipe `salience-appraise`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, then
//! trigger the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail)
//! and record ran/health from its EXIT STATUS alone. The thread parses NOTHING
//! back and performs NO durable write itself — the recipe's `simard …` tool
//! calls ARE the effect. ENABLED by default (opt-out) behind the default-ON double env gate.
//!
//! **Status (issue #5):** implemented; [`SalienceThread::tick`] is covered by
//! the hermetic offline unit tests in `tests_catalog` (fake recipe invoker)
//! plus a gated live-smoke check.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use super::super::recipe_rail::{self, RecipeInvoker};
use super::super::schedule;
use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
use crate::ooda_brain::ThreadName;

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

    fn purpose(&self) -> &'static str {
        "Appraise what matters most now and rank goal salience"
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

        // The global safety switch prevents the durable-writing recipe from
        // running at all — the recipe's own `simard memory remember` /
        // `simard cognition salience-signal` calls are the only effect — so a
        // dry-run tick triggers ZERO recipe subprocesses.
        if ctx.dry_run || self.cfg.dry_run {
            self.note_run(ctx.now_epoch, true);
            return ThreadOutcome::ok(
                format!("{RECIPE}: dry-run (no recipe triggered)"),
                start.elapsed(),
            );
        }

        // INPUT — active goals (the appraisal targets) + health facts, fenced.
        // The thread parses NOTHING back and performs NO durable write itself:
        // the free-text `salience:` rationale facts AND the numeric Decide signal
        // (`state/salience_signal.json`) are both written by the recipe's own
        // `simard …` tool calls.
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

        // TRIGGER — record ran/health from the recipe's EXIT STATUS only.
        let outcome = recipe_rail::run_reflective_thread(
            self.invoker.as_ref(),
            RECIPE,
            ThreadName::Salience,
            ctx.state_root,
            ctx_vars,
            start,
        );
        self.note_run(ctx.now_epoch, outcome.success);
        outcome
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
            purpose: self.purpose().to_string(),
            cadence_secs: self.policy().cadence_secs(),
        }
    }
}

fn read_u64_env(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}
