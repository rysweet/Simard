//! Cognitive thread 10 (issue #5) — [`ValuesDeliberationThread`]: weigh competing goods for hard tradeoffs into `values:` facts/procedures as advice only (no veto; overseer stays terminal).
//!
//! A thin `CognitiveThread` rail over the agentic recipe `values-deliberate`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `values:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5, TDD):** the config/type surface + the constructors are
//! real studs; [`ValuesDeliberationThread::tick`] is a `todo!()` pinned RED by
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
pub const ID: &str = "values_deliberation";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "values-deliberate";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_VALUES_ENABLED";

/// Tunables for [`ValuesDeliberationThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValuesDeliberationConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for ValuesDeliberationConfig {
    fn default() -> Self {
        Self {
            interval_secs: 10800,
            dry_run: false,
        }
    }
}

/// Cognitive thread 10.
pub struct ValuesDeliberationThread {
    cfg: ValuesDeliberationConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl ValuesDeliberationThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = ValuesDeliberationConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_VALUES_DELIBERATION_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: ValuesDeliberationConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

    /// Cheap guard: deliberate on first activation, else only when a hard-tradeoff
    /// marker (a goal on the board) is present — otherwise a near-zero `skipped()`.
    fn has_hard_tradeoff(&self, ctx: &ThreadContext<'_>) -> bool {
        self.last_run_epoch.is_none()
            || !crate::goal_board_store::load(ctx.state_root)
                .board
                .active
                .is_empty()
    }
}

impl CognitiveThread for ValuesDeliberationThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::ValuesDeliberation
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

        // Interval-with-guard: skip cheaply absent a hard tradeoff.
        if !self.has_hard_tradeoff(ctx) {
            return ThreadOutcome::skipped();
        }

        // INPUT — the professed identity anchors the weighing; prior values +
        // active goals as fenced untrusted context.
        let identity = ctx
            .memory
            .search_facts("narrative:identity", 1, 0.0)
            .ok()
            .and_then(|f| f.into_iter().next())
            .map(|f| f.content)
            .unwrap_or_default();
        let prior = ctx
            .memory
            .search_facts("values:", 10, 0.0)
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
            ("identity", recipe_rail::fence_untrusted(&identity)),
            ("prior_values", recipe_rail::fence_untrusted(&prior)),
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

        let competing = envelope
            .get("competing_goods")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let weighing = envelope
            .get("weighing")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let stance = envelope
            .get("recommended_stance")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // WRITE — an ADVISORY `values:` fact. Deliberation weighs; it never emits
        // an enforcement/veto artifact (separation of powers: that is the
        // overseer's job, not this thread's).
        let key = recipe_rail::validate_concept_key(stance)
            .unwrap_or_else(|| format!("deliberation-{}", ctx.now_epoch));
        if !dry_run {
            let concept = format!("values:{key}");
            let content = recipe_rail::secret_scrub(&format!(
                "competing goods: [{competing}]; weighing: {weighing}; stance: {stance}"
            ));
            if let Err(e) =
                ctx.memory
                    .store_fact(&concept, &content, 0.6, &["values".to_string()], ID)
            {
                self.note_run(ctx.now_epoch, false);
                return ThreadOutcome::failed(
                    format!("values fact write failed: {e}"),
                    start.elapsed(),
                );
            }
        }

        // At most ONE follow-up heuristic goal (advice, never a veto).
        let mut goal = false;
        if let Some(heuristic) = envelope.get("heuristic").and_then(|v| v.as_str())
            && !dry_run
            && !heuristic.trim().is_empty()
        {
            let id = format!("values-heuristic-{}", ctx.now_epoch);
            goal = recipe_rail::propose_goal_if_capacity(
                ctx.state_root,
                &id,
                &recipe_rail::secret_scrub(heuristic),
                40,
            );
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("values_deliberation: stance `{key}`, heuristic_goal={goal}"),
            start.elapsed(),
        )
        .with_detail(json!({ "stance": key, "heuristic_goal": goal, "dry_run": dry_run }))
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
