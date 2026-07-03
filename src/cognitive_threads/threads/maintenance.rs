//! Exemplar 1 — [`MaintenanceThread`]: SAFE, conservative housekeeping
//! (design §8, security SR-5..SR-7).
//!
//! On a slow cadence it prunes stale quarantine/snapshot/backup artefacts under
//! the state root by reusing the existing `cmd_cleanup::disk` / `memory_backup`
//! helpers — never reimplementing them. Every destructive candidate must pass
//! the canonical allow/deny + symlink-refusal gate ([`is_safe_to_delete`]) and
//! honour `ctx.dry_run`. Behaviour bodies are `todo!()` stubs during TDD; the
//! config/type surface is real so tests can pin the safety contract.
#![allow(dead_code, unused_variables)]

use std::path::{Path, PathBuf};

use super::super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};

/// Stable telemetry id.
const MAINTENANCE_ID: &str = "maintenance";

/// Tunables for [`MaintenanceThread`]. Retention counts are floors: the thread
/// always keeps at least this many of the newest copies before pruning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaintenanceConfig {
    /// Cadence (`SIMARD_MAINTENANCE_INTERVAL_SECS`, default daily).
    pub interval_secs: u64,
    /// Retention floor for `cognitive.corrupt-*` quarantine dirs (>= 1).
    pub keep_corrupt: usize,
    /// Retention floor for store snapshots / shadow WAL copies (>= 1).
    pub keep_snapshots: usize,
    /// Retention floor for verified backups (>= 1).
    pub keep_backups: usize,
    /// Cap for runaway cargo target dirs, in bytes.
    pub target_cap_bytes: u64,
    /// Dry-run: log intended actions, delete nothing
    /// (`SIMARD_MAINTENANCE_DRY_RUN`). Defaults to `true` (opt-in destructive).
    pub dry_run: bool,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            interval_secs: 24 * 60 * 60,
            keep_corrupt: 3,
            keep_snapshots: 5,
            keep_backups: 7,
            target_cap_bytes: 20 * 1024 * 1024 * 1024,
            dry_run: true,
        }
    }
}

/// The maintenance/cleanup cognitive thread (exemplar 1).
pub struct MaintenanceThread {
    cfg: MaintenanceConfig,
    last_run_epoch: Option<u64>,
    next_run_epoch: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
}

impl MaintenanceThread {
    /// Build from the environment with safe defaults (retention floors >= 1,
    /// `dry_run` unless explicitly disabled).
    pub fn from_env() -> Self {
        let mut cfg = MaintenanceConfig::default();
        if let Some(v) = read_u64_env("SIMARD_MAINTENANCE_INTERVAL_SECS") {
            cfg.interval_secs = super::super::schedule::clamp_interval_secs(v);
        }
        if let Some(v) = read_usize_env("SIMARD_MAINTENANCE_KEEP_CORRUPT") {
            cfg.keep_corrupt = v.max(1);
        }
        if let Some(v) = read_usize_env("SIMARD_MAINTENANCE_KEEP_SNAPSHOTS") {
            cfg.keep_snapshots = v.max(1);
        }
        if let Some(v) = read_usize_env("SIMARD_MAINTENANCE_KEEP_BACKUPS") {
            cfg.keep_backups = v.max(1);
        }
        if let Some(v) = read_bool_env("SIMARD_MAINTENANCE_DRY_RUN") {
            cfg.dry_run = v;
        }
        Self::new(cfg)
    }

    /// Build from an explicit config (test seam — no env).
    pub fn new(cfg: MaintenanceConfig) -> Self {
        Self {
            cfg,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
        }
    }
}

impl CognitiveThread for MaintenanceThread {
    fn id(&self) -> &str {
        MAINTENANCE_ID
    }

    fn kind(&self) -> ThreadKind {
        ThreadKind::Maintenance
    }

    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(std::time::Duration::from_secs(self.cfg.interval_secs))
    }

    fn priority(&self) -> Priority {
        Priority::Low
    }

    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        todo!("Step 7 TDD: maintenance body implemented by the implementation step")
    }

    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: MAINTENANCE_ID.to_string(),
            enabled: true,
            last_run_epoch: self.last_run_epoch,
            next_run_epoch: self.next_run_epoch,
            last_success: self.last_success,
            consecutive_errors: self.consecutive_errors,
            backoff_until_epoch: None,
        }
    }
}

/// SR-5/SR-6 destructive-op gate. Returns `true` only when `candidate` is safe
/// to remove:
/// - it canonicalizes to a path **inside** one of `allow_roots`;
/// - it is **not** a symlink (refuse `symlink_metadata().is_symlink()`), which
///   defeats `..`/symlink traversal and the TOCTOU swap;
/// - it does **not** equal or sit under any `deny_paths` entry (protected roots
///   such as `worktrees/main`, `~/.simard/repo`, the live store, engineer
///   worktrees).
///
/// Anything not provably inside an allow-root is refused (fail-closed).
pub(crate) fn is_safe_to_delete(
    candidate: &Path,
    allow_roots: &[PathBuf],
    deny_paths: &[PathBuf],
) -> bool {
    todo!("Step 7 TDD: SR-5/6 safety gate implemented by the implementation step")
}

fn read_u64_env(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}

fn read_usize_env(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|s| s.trim().parse().ok())
}

fn read_bool_env(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .map(|s| matches!(s.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
}
