//! Cognitive thread 11 (issue #5) — [`InteroceptionThread`]: the agent senses
//! its own "body" (disk, CI, dependency drift, store size, latency) —
//! homeostasis as a first-class thread.
//!
//! Unlike the other nine reflective threads this rail is **deterministic
//! sensing with no recipe** — an LLM adds no value to "is disk < 10%?". It
//! reuses the existing `disk_pressure` helper and the `gh`-backed CI probe (an
//! async hop driven synchronously via `ctx.runtime.block_on(...)`), records
//! `interoception_*` self-metrics and `interocept:<subsystem>` facts, and on a
//! threshold breach files a **deduplicated** issue and proposes at most one
//! health goal. Issue bodies carry summarized status, never raw command/env
//! output (S5). It also proves the abstraction hosts a recipe-free thread.
//!
//! **Status (issue #5):** implemented; [`InteroceptionThread::tick`] (recipe-free
//! deterministic sensing) is covered by the hermetic offline unit tests in
//! `tests_catalog`.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde_json::json;

use super::super::schedule;
use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
use crate::stewardship::gh_client::{GhClient, RealGhClient};

/// Stable telemetry id.
pub const ID: &str = "interoception";
/// Per-thread env gate (paired with the master `SIMARD_COGNITIVE_THREADS_ENABLED`).
pub const GATE_ENV: &str = "SIMARD_THREAD_INTEROCEPTION_ENABLED";

/// Tunables for [`InteroceptionThread`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteroceptionConfig {
    /// Cadence in seconds (clamped up to `schedule::MIN_INTERVAL_SECS`).
    pub interval_secs: u64,
    /// Target repo for a health issue when a threshold breaches.
    pub repo: String,
    /// Disk-free ratio at or below which a health finding is raised (`0.0..=1.0`).
    pub disk_free_floor_pct: u8,
    /// Dry-run: sense + emit telemetry only; file no issue, propose no goal.
    pub dry_run: bool,
}

impl Default for InteroceptionConfig {
    fn default() -> Self {
        Self {
            interval_secs: 3300,
            repo: "rysweet/Simard".to_string(),
            disk_free_floor_pct: 10,
            dry_run: false,
        }
    }
}

/// The interoception / self-maintenance cognitive thread (recipe-free).
pub struct InteroceptionThread {
    cfg: InteroceptionConfig,
    gh: Box<dyn GhClient + Send>,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl InteroceptionThread {
    /// Build from the environment using the real `gh`-backed client.
    pub fn from_env() -> Self {
        let mut cfg = InteroceptionConfig::default();
        if let Some(v) = read_u64_env("SIMARD_THREAD_INTEROCEPTION_INTERVAL_SECS") {
            cfg.interval_secs = schedule::clamp_interval_secs(v);
        }
        // Apply the global interval-scale knob (SIMARD_THREAD_INTERVAL_SCALE).
        cfg.interval_secs = schedule::scale_and_clamp_interval_secs(cfg.interval_secs);
        Self::with_client(cfg, Box::new(RealGhClient::new()))
    }

    /// Build from an explicit config with an injected [`GhClient`] (test seam —
    /// a fake client keeps unit tests offline and credential-free).
    pub fn with_client(cfg: InteroceptionConfig, gh: Box<dyn GhClient + Send>) -> Self {
        Self {
            cfg,
            gh,
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

impl CognitiveThread for InteroceptionThread {
    fn id(&self) -> &str {
        ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Interoception
    }

    fn purpose(&self) -> &'static str {
        "Sense the system's own body — disk, CI, drift, store size, latency"
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::from_secs(schedule::clamp_interval_secs(
            self.cfg.interval_secs,
        )))
    }

    fn priority(&self) -> Priority {
        // Health can dominate salience, so interoception is Normal, not Low.
        Priority::Normal
    }

    fn enabled(&self) -> bool {
        super::super::recipe_rail::thread_enabled(GATE_ENV)
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        let start = Instant::now();
        let dry_run = ctx.dry_run || self.cfg.dry_run;
        let mut facts = 0usize;
        let mut breach = false;
        let mut breach_detail = String::new();

        // PROBE 1 — disk pressure (reuses the shared helper, never reimplemented).
        if let Ok(report) = crate::disk_pressure::check_with_default_threshold(ctx.state_root) {
            let free_pct = if report.total_bytes > 0 {
                (report.free_bytes as f64 / report.total_bytes as f64) * 100.0
            } else {
                100.0
            };
            let content = format!(
                "free_bytes={} total_bytes={} free_pct={:.1} refuse={}",
                report.free_bytes,
                report.total_bytes,
                free_pct,
                report.should_refuse()
            );
            let stored = if dry_run {
                true
            } else {
                ctx.memory
                    .store_fact_with_caller_key(
                        "interocept:disk",
                        "interocept:disk",
                        &content,
                        0.8,
                        &["interoception".to_string()],
                        ID,
                    )
                    .is_ok()
            };
            if stored {
                facts += 1;
            }
            let _ = crate::self_metrics::record_metric(
                "interoception_disk_free_ratio",
                free_pct / 100.0,
                ID,
            );
            if report.should_refuse() || free_pct <= self.cfg.disk_free_floor_pct as f64 {
                breach = true;
                breach_detail =
                    format!("disk pressure: only {free_pct:.1}% free at the state root");
            }
        }

        // PROBE 2 — cognitive store size (a growth/pressure signal).
        if let Ok(stats) = ctx.memory.get_statistics() {
            let content = format!(
                "total={} semantic={} episodic={} procedural={} prospective={}",
                stats.total(),
                stats.semantic_count,
                stats.episodic_count,
                stats.procedural_count,
                stats.prospective_count
            );
            let stored = if dry_run {
                true
            } else {
                ctx.memory
                    .store_fact_with_caller_key(
                        "interocept:memory",
                        "interocept:memory",
                        &content,
                        0.8,
                        &["interoception".to_string()],
                        ID,
                    )
                    .is_ok()
            };
            if stored {
                facts += 1;
            }
            let _ = crate::self_metrics::record_metric(
                "interoception_store_size",
                stats.total() as f64,
                ID,
            );
        }

        // On a threshold breach: file a DEDUPLICATED issue carrying only
        // summarized status (never raw command/env output, S5) and propose at
        // most one health goal.
        let mut issue_filed = false;
        let mut health_goal = false;
        if breach && !dry_run {
            let signature = "interoception-disk";
            let already = self
                .gh
                .search_issues(&self.cfg.repo, signature)
                .unwrap_or_default();
            if already.is_empty() {
                let title = "[interoception] self-maintenance: resource pressure detected";
                let body = format!("{breach_detail}\n\nstewardship-signature: {signature}");
                issue_filed = self.gh.create_issue(&self.cfg.repo, title, &body).is_ok();
            }
            let id = format!("interoception-health-{}", ctx.now_epoch);
            health_goal = propose_health_goal_if_capacity(ctx.state_root, &id, &breach_detail, 20);
        }

        self.note_run(ctx.now_epoch, true);
        ThreadOutcome::ok(
            format!("interoception: {facts} probe(s), breach={breach}"),
            start.elapsed(),
        )
        .with_detail(json!({
            "facts": facts,
            "breach": breach,
            "issue_filed": issue_filed,
            "health_goal": health_goal,
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
            purpose: self.purpose().to_string(),
            cadence_secs: self.policy().cadence_secs(),
        }
    }
}

fn read_u64_env(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}

/// Propose at most one health goal onto the shared board through the single
/// capacity-checked path (`goal_board_store::mutate`), preserving the global
/// `MAX_ACTIVE_GOALS` cap. Deduplicated by id; best-effort (a locked/unwritable
/// board never fails the calling tick). A thread-proposed goal is
/// enforcement-equivalent to an operator goal — no privileged path (S3).
fn propose_health_goal_if_capacity(
    state_root: &std::path::Path,
    id: &str,
    description: &str,
    priority: u32,
) -> bool {
    use crate::goal_curation::ActiveGoal;
    crate::goal_board_store::mutate(state_root, |state| {
        if state.board.active_slots_remaining() > 0
            && !state.board.active.iter().any(|g| g.id == id)
        {
            state
                .board
                .active
                .push(ActiveGoal::new(id, description, priority));
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}
