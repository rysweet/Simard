//! Cognitive thread 4 (issue #5) — [`ProspectionThread`]: simulate plausible futures for active goals into `foresight:` facts + prospective triggers, and propose <=1 preventive goal per pass (capacity-checked).
//!
//! A thin `CognitiveThread` rail over the agentic recipe `prospect-foresight`: assemble
//! read-only context in-thread, fence memory-sourced text as untrusted, invoke
//! the recipe through the shared [`RecipeInvoker`](super::super::recipe_rail),
//! parse a strict-JSON envelope, then scrub + size-cap and write through the
//! declared `foresight:` prefix only. OFF by default behind the double env gate.
//!
//! **Status (issue #5):** implemented; [`ProspectionThread::tick`] is covered by
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
pub const ID: &str = "prospection";
/// The agentic recipe this rail invokes.
pub const RECIPE: &str = "prospect-foresight";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_PROSPECTION_ENABLED";

/// Tunables for [`ProspectionThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProspectionConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Dry-run: perform no durable writes, emit structured telemetry only.
    pub dry_run: bool,
}

impl Default for ProspectionConfig {
    fn default() -> Self {
        Self {
            interval_secs: 4500,
            dry_run: false,
        }
    }
}

/// Cognitive thread 4.
pub struct ProspectionThread {
    cfg: ProspectionConfig,
    invoker: Box<dyn RecipeInvoker>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl ProspectionThread {
    /// Build from the environment with the production recipe invoker.
    pub fn from_env(repo_root: std::path::PathBuf, state_root: std::path::PathBuf) -> Self {
        let mut cfg = ProspectionConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_PROSPECTION_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        let invoker = Box::new(recipe_rail::RecipeRunnerInvoker::new(repo_root, state_root));
        Self::with_invoker(cfg, invoker)
    }

    /// Build from an explicit config with an injected [`RecipeInvoker`] (test
    /// seam — a fake keeps unit tests offline and credential-free).
    pub fn with_invoker(cfg: ProspectionConfig, invoker: Box<dyn RecipeInvoker>) -> Self {
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

impl CognitiveThread for ProspectionThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::LongTermPlanning
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

        // INPUT — active goals + prior foresight (fenced as untrusted).
        let board = crate::goal_board_store::load(ctx.state_root).board;
        let goals = board
            .active
            .iter()
            .map(|g| format!("{}: {}", g.id, g.description))
            .collect::<Vec<_>>()
            .join("\n");
        let prior = ctx
            .memory
            .search_facts("foresight:", 20, 0.0)
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
            ("active_goals", recipe_rail::fence_untrusted(&goals)),
            ("prior_foresight", recipe_rail::fence_untrusted(&prior)),
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

        // WRITE — a `foresight:<goal_id>` fact + a prospective watch-condition per
        // predicted risk. Prospective triggers surface in the next cycle's
        // `check_triggers`, so prospection feeds OODA indirectly.
        let mut risks_written = 0usize;
        let mut triggers_staged = 0usize;
        if let Some(risks) = envelope.get("risks").and_then(|v| v.as_array()) {
            for r in risks {
                let goal_id = r.get("goal_id").and_then(|v| v.as_str()).unwrap_or("");
                let scenario = r.get("scenario").and_then(|v| v.as_str()).unwrap_or("");
                let trigger = r
                    .get("trigger_phrase")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let Some(key) = recipe_rail::validate_concept_key(goal_id) else {
                    continue;
                };
                if dry_run {
                    risks_written += 1;
                    continue;
                }
                let concept = format!("foresight:{key}");
                let content = recipe_rail::secret_scrub(scenario);
                if let Err(e) = ctx.memory.store_fact_with_caller_key(
                    &concept,
                    &concept,
                    &content,
                    0.6,
                    &["foresight".to_string()],
                    ID,
                ) {
                    self.note_run(ctx.now_epoch, false);
                    return ThreadOutcome::failed(
                        format!("foresight fact write failed: {e}"),
                        start.elapsed(),
                    );
                }
                risks_written += 1;
                if !trigger.trim().is_empty() {
                    let action = format!("review goal {key}: predicted risk materialised");
                    if ctx
                        .memory
                        .store_prospective(
                            &recipe_rail::secret_scrub(scenario),
                            &recipe_rail::secret_scrub(trigger),
                            &action,
                            5,
                        )
                        .is_ok()
                    {
                        triggers_staged += 1;
                    }
                }
            }
        }

        // At most ONE preventive goal per pass, capacity-checked (S3).
        let mut preventive = false;
        if let Some(text) = envelope.get("preventive_goal").and_then(|v| v.as_str())
            && !dry_run
            && !text.trim().is_empty()
        {
            let id = format!("prospection-preventive-{}", ctx.now_epoch);
            preventive = recipe_rail::propose_goal_if_capacity(
                ctx.state_root,
                &id,
                &recipe_rail::secret_scrub(text),
                30,
            );
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("prospection: {risks_written} risk(s), {triggers_staged} trigger(s), preventive={preventive}"),
            start.elapsed(),
        )
        .with_detail(json!({
            "risks_written": risks_written,
            "triggers_staged": triggers_staged,
            "preventive_goal": preventive,
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
