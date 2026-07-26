//! Exemplar 1 — [`MaintenanceThread`]: SAFE, conservative housekeeping
//! (design §8, security SR-5..SR-7).
//!
//! On a slow cadence it prunes stale quarantine/snapshot/backup artefacts under
//! the state root by reusing the existing `cmd_cleanup::disk` / `memory_backup`
//! helpers — never reimplementing them. Every destructive candidate must pass
//! the canonical allow/deny + symlink-refusal gate ([`is_safe_to_delete`]) and
//! honour `ctx.dry_run`. The behaviour is implemented; the config/type surface
//! and the safety contract are pinned by tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::json;

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
        let start = Instant::now();
        // Dry-run if EITHER the thread config or the global switch asks for it.
        let dry_run = self.cfg.dry_run || ctx.dry_run;
        let root = ctx.state_root;

        // Fail-closed allow-list: only ever consider paths strictly under the
        // injected state root. Deny-list: protected roots that must survive
        // even when they sit under the state root.
        let allow_roots = vec![root.to_path_buf()];
        let deny_paths = protected_paths(root, ctx.repo_root);

        let mut report = PruneReport::default();

        // 1) Quarantine dirs left by corrupt-store recovery.
        prune_prefixed(
            root,
            &["cognitive.corrupt-"],
            self.cfg.keep_corrupt,
            dry_run,
            &allow_roots,
            &deny_paths,
            &mut report,
        );
        // 2) Store snapshots / shadow-WAL copies staged for recovery.
        prune_prefixed(
            root,
            &["cognitive.snapshot-", "snapshot-", "shadow-wal-", "shadow-"],
            self.cfg.keep_snapshots,
            dry_run,
            &allow_roots,
            &deny_paths,
            &mut report,
        );
        // 3) Verified backups — keep the newest N.
        prune_prefixed(
            root,
            &["backup-", "verified-backup-"],
            self.cfg.keep_backups,
            dry_run,
            &allow_roots,
            &deny_paths,
            &mut report,
        );

        // 4) Read-only disk-pressure observation (reuses the shared helper —
        //    never reimplemented). Best-effort; failure is not fatal.
        let disk = crate::disk_pressure::check_with_default_threshold(root)
            .ok()
            .map(|r| {
                json!({
                    "free_bytes": r.free_bytes,
                    "total_bytes": r.total_bytes,
                    "should_refuse": r.should_refuse(),
                })
            })
            .unwrap_or(serde_json::Value::Null);

        self.last_run_epoch = Some(ctx.now_epoch);
        self.next_run_epoch = super::super::schedule::next_run_epoch(
            &self.policy(),
            self.last_run_epoch,
            ctx.now_epoch,
        );
        self.last_success = Some(true);
        self.consecutive_errors = 0;

        let verb = if dry_run { "would prune" } else { "pruned" };
        let summary = format!(
            "maintenance: {verb} {} artefact(s) ({} freed), {} refused by safety gate",
            report.removed,
            crate::disk_pressure::human_bytes(report.freed_bytes),
            report.refused,
        );
        let detail = json!({
            "dry_run": dry_run,
            "removed": report.removed,
            "refused": report.refused,
            "freed_bytes": report.freed_bytes,
            "actions": report.actions,
            "disk": disk,
        });
        ThreadOutcome::ok(summary, start.elapsed()).with_detail(detail)
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
    // 1. Must exist and NOT be a symlink. `symlink_metadata` does not follow
    //    the final component, so a swapped symlink is refused here (SR-5)
    //    before any canonicalization can be tricked into resolving it.
    let meta = match std::fs::symlink_metadata(candidate) {
        Ok(m) => m,
        Err(_) => return false, // fail-closed on any stat error
    };
    if meta.file_type().is_symlink() {
        return false;
    }

    // 2. Resolve to a real, canonical path (defeats `..`/traversal).
    let real = match std::fs::canonicalize(candidate) {
        Ok(p) => p,
        Err(_) => return false,
    };

    // 3. Must be strictly INSIDE at least one canonical allow-root. Equalling
    //    a root, or living outside every root, is refused (fail-closed).
    let inside_allow = allow_roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .map(|r| real != r && real.starts_with(&r))
            .unwrap_or(false)
    });
    if !inside_allow {
        return false;
    }

    // 4. Must not equal or sit under any protected deny path (SR-6).
    for deny in deny_paths {
        // Compare against the canonical deny path when it exists; otherwise
        // fall back to a literal prefix check so a not-yet-created protected
        // path still shields its future location.
        let denied = match std::fs::canonicalize(deny) {
            Ok(d) => real == d || real.starts_with(&d),
            Err(_) => real == *deny || real.starts_with(deny),
        };
        if denied {
            return false;
        }
    }

    true
}

/// Protected roots that must never be pruned even when they live under the
/// state root: the live repo checkout, the live cognitive store, `worktrees/`
/// (main + engineer worktrees), and the recovered-repo mirror.
fn protected_paths(state_root: &Path, repo_root: &Path) -> Vec<PathBuf> {
    vec![
        repo_root.to_path_buf(),
        state_root.join("repo"),
        state_root.join("cognitive.redb"),
        state_root.join("cognitive"),
        state_root.join("worktrees"),
        state_root.join("worktrees/main"),
        state_root.join("engineer-worktrees"),
    ]
}

/// Accumulated structured record of one maintenance pass.
#[derive(Default)]
struct PruneReport {
    removed: usize,
    refused: usize,
    freed_bytes: u64,
    actions: Vec<serde_json::Value>,
}

/// Prune stale entries directly under `root` whose file name starts with any of
/// `prefixes`, keeping the newest `keep` (by mtime). Every destructive
/// candidate must pass [`is_safe_to_delete`]; `dry_run` records the intended
/// action without touching the filesystem. Best-effort throughout — a single
/// I/O error is recorded and skipped, never propagated.
#[allow(clippy::too_many_arguments)]
fn prune_prefixed(
    root: &Path,
    prefixes: &[&str],
    keep: usize,
    dry_run: bool,
    allow_roots: &[PathBuf],
    deny_paths: &[PathBuf],
    report: &mut PruneReport,
) {
    let read = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return,
    };

    // Collect (path, mtime) for entries matching any prefix.
    let mut matches: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !prefixes.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        matches.push((entry.path(), mtime));
    }

    // Newest first; keep the retention floor, consider the rest for pruning.
    matches.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let keep = keep.max(1);
    if matches.len() <= keep {
        return;
    }

    for (path, _) in matches.into_iter().skip(keep) {
        if !is_safe_to_delete(&path, allow_roots, deny_paths) {
            report.refused += 1;
            report.actions.push(json!({
                "action": "refused",
                "path": path.display().to_string(),
                "reason": "failed safety gate",
            }));
            continue;
        }

        let size = crate::cmd_cleanup::disk::dir_size(&path).unwrap_or(0);
        if dry_run {
            report.actions.push(json!({
                "action": "would_prune",
                "path": path.display().to_string(),
                "bytes": size,
            }));
            report.removed += 1;
            report.freed_bytes = report.freed_bytes.saturating_add(size);
            continue;
        }

        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => {
                report.removed += 1;
                report.freed_bytes = report.freed_bytes.saturating_add(size);
                report.actions.push(json!({
                    "action": "pruned",
                    "path": path.display().to_string(),
                    "bytes": size,
                }));
            }
            Err(e) => {
                report.refused += 1;
                report.actions.push(json!({
                    "action": "error",
                    "path": path.display().to_string(),
                    "reason": e.to_string(),
                }));
            }
        }
    }
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
