//! Recipe-runner-backed disk health check (issue #2020; reworked in #4722).
//!
//! Two-tier approach:
//!   1. **Deterministic emergency cleanup** — pure Rust, no LLM, runs when disk
//!      is critically full (≥95%). Deletes known-safe build artifacts immediately.
//!   2. **Agentic recipe trigger** — when disk is moderately full, the daemon
//!      runs the `disk-health-check` recipe. The recipe *acts* through the
//!      safety-enforcing `simard disk` tool and prints no envelope; this module
//!      is a **thin trigger** that records success/failure by the recipe child's
//!      exit status alone (issue #4722). It no longer parses recipe stdout.
//!
//! The emergency tier exists because the recipe tier needs disk space to spawn
//! an agent process. At 100% disk, the recipe deadlocks.

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{info, warn};

use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;

const ADAPTER_TAG: &str = "disk-health-check";
const RECIPE_FILENAME: &str = "disk-health-check.yaml";

/// Deterministic result of the Tier-1 emergency cleanup (pure Rust, no recipe,
/// no LLM). This is **not** recipe-output scraping — it is the direct return of
/// the in-process `rm` pass, so it stays after the issue #4722 rework that
/// removed all recipe-stdout parsing from this file.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskHealthReport {
    /// Current disk usage percentage (0–100).
    pub disk_used_pct: u8,
    /// Total bytes freed during this check (0 if no cleanup needed).
    pub freed_bytes: u64,
    /// Human-readable list of cleanup actions taken.
    pub actions_taken: Vec<String>,
}

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// `home_override` allows tests to supply a fake home directory without
/// mutating the process-wide `HOME` environment variable.
fn resolve_recipe_path(repo_root: &Path, home_override: Option<&Path>) -> Option<PathBuf> {
    let home = home_override.map(PathBuf::from).or_else(dirs::home_dir);
    if let Some(home) = home {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// High-watermark: emergency cleanup TRIGGERS at or above this used-% (#4803).
pub(crate) const EMERGENCY_HIGH_WATERMARK_PCT: u8 = 95;

/// Low-watermark: the re-arm line of the hysteresis band (#4803). Documented
/// as the used-% the disk must fall back below before a fresh trigger is
/// semantically "re-armed". Anti-thrash is enforced concretely by the
/// persistent time-backoff below; this constant pins the band so `high > low`
/// is a real two-edge gate, never a single edge that re-fires every timer tick.
pub(crate) const EMERGENCY_LOW_WATERMARK_PCT: u8 = 85;

/// Default minimum seconds between two emergency cleanups (#4803). 15 min is
/// longer than the observed ~25-min refill cadence (one fire per timer tick),
/// so a single build window can never trigger a second delete-then-rebuild
/// storm. Override via `SIMARD_DISK_EMERGENCY_MIN_REFIRE_SECS`.
pub(crate) const DEFAULT_EMERGENCY_MIN_REFIRE_SECS: u64 = 900;

/// Upper clamp for the min-refire backoff (24h) so a fat-fingered override
/// can't wedge cleanup off for days.
const EMERGENCY_MIN_REFIRE_CEILING_SECS: u64 = 86_400;

/// Parse `SIMARD_DISK_EMERGENCY_MIN_REFIRE_SECS`, clamping to `[0, 86400]`.
/// Unset / empty / unparseable / negative → the 900s default. An explicit `0`
/// disables the backoff gate (documented escape hatch).
pub(crate) fn emergency_refire_min_secs_from(raw: Option<&str>) -> u64 {
    match raw.map(str::trim) {
        Some(s) if !s.is_empty() => match s.parse::<u64>() {
            Ok(n) => n.min(EMERGENCY_MIN_REFIRE_CEILING_SECS),
            Err(_) => DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
        },
        _ => DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
    }
}

/// Pure hysteresis + time-backoff decision for the emergency cleanup (#4803).
///
/// Returns `true` iff cleanup should fire now:
///   - `pct` must be at or above [`EMERGENCY_HIGH_WATERMARK_PCT`] (95%); below
///     that (including at the 85% low watermark) it never fires.
///   - If `min_refire_secs == 0` the backoff is disabled and any at/above-high
///     tick fires.
///   - Otherwise a prior run within `min_refire_secs` SUPPRESSES the re-fire
///     (the anti-thrash contract); once the window elapses — or there is no
///     prior run — a genuinely-full disk may fire again.
pub(crate) fn should_fire_emergency_cleanup(
    pct: u8,
    last_run: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
    min_refire_secs: u64,
) -> bool {
    if pct < EMERGENCY_HIGH_WATERMARK_PCT {
        return false;
    }
    if min_refire_secs == 0 {
        return true;
    }
    match last_run {
        None => true,
        Some(prev) => match now.duration_since(prev) {
            Ok(elapsed) => elapsed.as_secs() >= min_refire_secs,
            // Clock skew (prev in the future): treat as "just ran" and
            // suppress, so a backwards clock can't reopen the thrash loop.
            Err(_) => false,
        },
    }
}

/// Path of the persistent last-emergency-cleanup marker under
/// `<state_root>/disk-health/`.
fn emergency_marker_path(state_root: &Path) -> PathBuf {
    state_root
        .join("disk-health")
        .join("last-emergency-cleanup")
}

/// Read the last-run timestamp (unix secs) from the marker. Fail-open: any
/// I/O or parse error is treated as "no prior run" (returns `None`) so a
/// missing/corrupt marker can never wedge a genuinely-needed cleanup.
fn read_emergency_marker(path: &Path) -> Option<std::time::SystemTime> {
    let raw = std::fs::read_to_string(path).ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

/// Persist `now` as the last-run timestamp. Fail-open with a warn log: if the
/// marker cannot be written the next tick simply lacks backoff (it stays gated
/// by the high watermark), which is strictly safer than aborting the cleanup
/// that just freed space.
fn write_emergency_marker(path: &Path, now: std::time::SystemTime) {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(
            target: "simard::disk_health",
            path = %parent.display(),
            error = %e,
            "could not create disk-health marker dir; emergency backoff not persisted this cycle",
        );
        return;
    }
    if let Err(e) = std::fs::write(path, secs.to_string()) {
        warn!(
            target: "simard::disk_health",
            path = %path.display(),
            error = %e,
            "could not write emergency-cleanup marker; backoff not persisted this cycle",
        );
    }
}

/// Guard every destructive `remove_dir_all` (#4803): refuse to delete a path
/// that is a symlink (following it could redirect the delete outside the tree)
/// or that is not contained within one of the `allowed_roots`
/// (`repo_root` / `state_root`). Uses `symlink_metadata` so a symlink is
/// detected WITHOUT being followed.
fn safe_to_remove(path: &Path, allowed_roots: &[&Path]) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => {
            warn!(
                target: "simard::disk_health",
                path = %path.display(),
                "refusing to remove symlinked build-artifact path (containment guard)",
            );
            false
        }
        Ok(_) => allowed_roots.iter().any(|root| path.starts_with(root)),
        Err(_) => false,
    }
}

/// Deterministic emergency disk cleanup — no LLM, no recipe, just rm.
///
/// Runs when disk usage is critically high (≥95%) AND the anti-thrash backoff
/// window is clear (#4803). Deletes known-safe build artifacts that can always
/// be regenerated by `cargo build`:
///   - `repo_root/target/debug/` (main build cache)
///   - `repo_root/worktrees/*/target/` (engineer worktree build caches)
///   - `repo_root/target/llvm-cov-target/` (coverage artifacts)
///   - `state_root/cargo-target/` and `state_root/shared-target/`
///   - stale backups beyond the 2 most recent
///
/// Returns a report of what was done, or None if disk is below threshold or the
/// min-refire backoff suppressed this tick.
pub fn emergency_cleanup(repo_root: &Path, state_root: &Path) -> Option<DiskHealthReport> {
    let pct = get_disk_usage_pct(repo_root)?;

    // Issue #4803: hysteresis high watermark + persistent time-backoff. The
    // old `pct < 95` edge re-fired every timer tick, deleting target/debug +
    // target/llvm-cov-target only for cargo to instantly rebuild and refill
    // `/` within one cycle (~25-min thrash). The backoff marker makes a second
    // fire within one build window impossible.
    let min_refire = emergency_refire_min_secs_from(
        std::env::var("SIMARD_DISK_EMERGENCY_MIN_REFIRE_SECS")
            .ok()
            .as_deref(),
    );
    let marker = emergency_marker_path(state_root);
    let last_run = read_emergency_marker(&marker);
    let now = std::time::SystemTime::now();

    if !should_fire_emergency_cleanup(pct, last_run, now, min_refire) {
        return None;
    }

    warn!(
        disk_pct = pct,
        min_refire_secs = min_refire,
        high_watermark = EMERGENCY_HIGH_WATERMARK_PCT,
        low_watermark = EMERGENCY_LOW_WATERMARK_PCT,
        "Emergency disk cleanup triggered (≥95% high watermark; backoff window clear)"
    );
    let allowed_roots: [&Path; 2] = [repo_root, state_root];
    let mut freed: u64 = 0;
    let mut actions: Vec<String> = Vec::new();

    // 1. Main target/debug/ — the single biggest consumer
    let debug_dir = repo_root.join("target/debug");
    if debug_dir.is_dir() && safe_to_remove(&debug_dir, &allowed_roots) {
        let size = dir_size_bytes(&debug_dir);
        if std::fs::remove_dir_all(&debug_dir).is_ok() {
            freed += size;
            actions.push(format!("Removed target/debug/ ({} MB)", size / 1_000_000));
        }
    }

    // 2. target/llvm-cov-target/
    let cov_dir = repo_root.join("target/llvm-cov-target");
    if cov_dir.is_dir() && safe_to_remove(&cov_dir, &allowed_roots) {
        let size = dir_size_bytes(&cov_dir);
        if std::fs::remove_dir_all(&cov_dir).is_ok() {
            freed += size;
            actions.push(format!(
                "Removed target/llvm-cov-target/ ({} MB)",
                size / 1_000_000
            ));
        }
    }

    // 3. Engineer worktree target/ dirs
    let worktrees_dir = repo_root.join("worktrees");
    if worktrees_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&worktrees_dir)
    {
        for entry in entries.flatten() {
            let target = entry.path().join("target");
            if target.is_dir() && safe_to_remove(&target, &allowed_roots) {
                let size = dir_size_bytes(&target);
                if std::fs::remove_dir_all(&target).is_ok() {
                    freed += size;
                    actions.push(format!(
                        "Removed worktrees/{}/target/ ({} MB)",
                        entry.file_name().to_string_lossy(),
                        size / 1_000_000
                    ));
                }
            }
        }
    }

    // 4. State root cargo dirs
    for name in &["cargo-target", "shared-target"] {
        let dir = state_root.join(name);
        if dir.is_dir() && safe_to_remove(&dir, &allowed_roots) {
            let size = dir_size_bytes(&dir);
            if std::fs::remove_dir_all(&dir).is_ok() {
                freed += size;
                actions.push(format!("Removed {name}/ ({} MB)", size / 1_000_000));
            }
        }
    }

    // 5. Stale backups (keep 2 most recent)
    let backup_dir = state_root.join("backups");
    if backup_dir.is_dir()
        && let Ok(mut entries) = std::fs::read_dir(&backup_dir)
    {
        let mut files: Vec<_> = entries
            .by_ref()
            .flatten()
            .filter(|e| e.path().is_file())
            .collect();
        files.sort_by_key(|e| {
            std::cmp::Reverse(
                e.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        });
        for old in files.into_iter().skip(2) {
            let size = old.metadata().map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(old.path()).is_ok() {
                freed += size;
            }
        }
        if freed > 0 {
            actions.push("Pruned old backups (kept 2 most recent)".to_string());
        }
    }

    let final_pct = get_disk_usage_pct(repo_root).unwrap_or(pct);

    // Persist the run time so the next timer tick honors the min-refire
    // backoff and cannot thrash within this build window (#4803).
    write_emergency_marker(&marker, now);

    info!(
        freed_mb = freed / 1_000_000,
        before_pct = pct,
        after_pct = final_pct,
        "Emergency cleanup complete"
    );

    Some(DiskHealthReport {
        disk_used_pct: final_pct,
        freed_bytes: freed,
        actions_taken: actions,
    })
}

/// Get disk usage percentage for the filesystem containing `path`.
fn get_disk_usage_pct(path: &Path) -> Option<u8> {
    let output = Command::new("df")
        .arg("--output=pcent")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .nth(1)?
        .trim()
        .trim_end_matches('%')
        .parse::<u8>()
        .ok()
}

/// Estimate directory size in bytes (non-recursive stat, uses du).
fn dir_size_bytes(path: &Path) -> u64 {
    Command::new("du")
        .arg("-sb")
        .arg(path)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0)
}

/// Run the disk-health-check recipe via `recipe-runner-rs` as a thin trigger
/// (issue #4722).
///
/// The recipe now *acts* through the `simard disk` tool (which enforces the
/// disk-safety heuristic internally) and prints **no** JSON envelope. This
/// function therefore no longer parses recipe stdout: it records success/failure
/// by the child's **exit status alone**.
///
/// `state_root` is the Simard state directory (typically `~/.simard`), passed to
/// the recipe as a context var. `repo_root` is used to locate the recipe YAML.
/// `home_override` lets tests supply a fake home directory without mutating the
/// process-wide `HOME`.
///
/// Returns:
///   - `Ok(true)`  — the recipe child exited `0`.
///   - `Ok(false)` — the recipe child exited non-zero (recorded as a failure;
///     stderr is warn-logged for diagnostics but never parsed).
///   - `Err(..)`   — the recipe file could not be resolved or the child could
///     not be spawned. These are structural failures, distinct from a recipe
///     that ran and reported failure.
pub fn run_disk_health_check(
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
) -> SimardResult<bool> {
    let recipe_path = resolve_recipe_path(repo_root, home_override).ok_or_else(|| {
        SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "recipe file {RECIPE_FILENAME} not found in hot-reload or in-tree paths"
            ),
        }
    })?;

    let agent_binary = RuntimeConfig::load()?.llm_provider.agent_binary_value();

    // No `--output-format json`: the recipe acts via the `simard disk` tool and
    // prints no envelope. We interpret the run by exit status only.
    let output = Command::new("recipe-runner-rs")
        .arg(recipe_path.as_os_str())
        .env("AMPLIHACK_AGENT_BINARY", agent_binary)
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
        .arg("-c")
        .arg(format!("repo_path={}", repo_root.display()))
        .output()
        .map_err(|e| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!("recipe-runner-rs spawn failed: {e}"),
        })?;

    let success = child_exit_indicates_success(&output.status);
    if !success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            status = %output.status,
            stderr = %truncate(stderr.trim(), 500),
            "disk-health recipe exited non-zero"
        );
    }
    Ok(success)
}

/// The exit-status → success contract for the reworked thin trigger (issue
/// #4722). `run_disk_health_check` records success/failure by the recipe child's
/// **exit status alone** — it no longer parses recipe stdout. The recipe acts
/// via the `simard disk` tool and prints no JSON envelope.
///
/// `true` iff the child exited `0`; `false` for any non-zero exit (including
/// signals). A spawn failure is a distinct `Err` at the call site, never mapped
/// here.
pub(crate) fn child_exit_indicates_success(status: &std::process::ExitStatus) -> bool {
    status.success()
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ------------------------------------------------------------------
    // Thin exit-status trigger contract (issue #4722 — TDD, written first)
    //
    // After the rework the disk-health trigger records success/failure by the
    // recipe child's EXIT STATUS alone; it no longer parses recipe stdout. These
    // tests pin that contract on the pure `child_exit_indicates_success` seam so
    // the reworked `run_disk_health_check` can adopt `SimardResult<bool>` without
    // reintroducing any output scraping.
    // ------------------------------------------------------------------

    #[test]
    fn child_exit_zero_is_success() {
        let status = std::process::Command::new("true")
            .status()
            .expect("spawn `true`");
        assert!(
            child_exit_indicates_success(&status),
            "exit 0 must record success"
        );
    }

    #[test]
    fn child_exit_nonzero_is_failure() {
        let status = std::process::Command::new("false")
            .status()
            .expect("spawn `false`");
        assert!(
            !child_exit_indicates_success(&status),
            "a non-zero exit must record failure"
        );
    }

    // ------------------------------------------------------------------
    // resolve_recipe_path
    // ------------------------------------------------------------------

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_dir() {
        let guard = tempfile::tempdir().unwrap();
        let result = resolve_recipe_path(Path::new("/nonexistent/repo"), Some(guard.path()));
        assert!(result.is_none());
    }

    #[test]
    fn resolve_recipe_path_finds_in_tree_recipe() {
        let tmp = tempfile::tempdir().unwrap();
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

        let result = resolve_recipe_path(tmp.path(), Some(tmp.path()));
        assert!(result.is_some());
        assert!(result.unwrap().ends_with(RECIPE_FILENAME));
    }

    // ------------------------------------------------------------------
    // run_disk_health_check — error paths (no recipe-runner-rs needed)
    // ------------------------------------------------------------------

    #[test]
    fn run_returns_error_when_recipe_not_found() {
        let guard = tempfile::tempdir().unwrap();
        let result = run_disk_health_check(
            Path::new("/nonexistent/repo"),
            Path::new("/nonexistent/state"),
            Some(guard.path()),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        match &err {
            SimardError::AdapterInvocationFailed { base_type, reason } => {
                assert_eq!(base_type, ADAPTER_TAG);
                assert!(
                    reason.contains("not found"),
                    "reason should mention not found: {reason}"
                );
            }
            other => panic!("expected AdapterInvocationFailed, got: {other:?}"),
        }
    }

    #[test]
    // #2360: mutates SIMARD_LLM_PROVIDER (process-global). Keyed into the
    // cognitive_memory serial group so it never runs concurrently with a
    // provider/state-root env reader.
    #[serial(cognitive_memory)]
    fn run_records_failure_when_recipe_runner_unavailable_or_recipe_invalid() {
        // Create a syntactically-invalid recipe file. Under the thin-trigger
        // contract (issue #4722): if recipe-runner-rs is missing the spawn fails
        // -> Err; if it's installed it rejects the recipe with a non-zero exit
        // -> Ok(false). Either way this is a *failure* outcome, never Ok(true).
        let tmp = tempfile::tempdir().unwrap();
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

        // Ensure RuntimeConfig::load() succeeds (CI has no config.toml).
        // SAFETY: SIMARD_LLM_PROVIDER is not related to HOME; it controls
        // which LLM provider the runtime selects. This is still env-var
        // mutation but not the HOME-related UB that Finding 2 targets.
        unsafe { std::env::set_var("SIMARD_LLM_PROVIDER", "copilot") };
        let result = run_disk_health_check(tmp.path(), tmp.path(), Some(tmp.path()));
        unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };

        match result {
            // recipe-runner-rs missing: spawn failure surfaces as a structural Err.
            Err(SimardError::AdapterInvocationFailed { base_type, reason }) => {
                assert_eq!(base_type, ADAPTER_TAG);
                assert!(
                    reason.contains("spawn failed"),
                    "spawn-failure reason expected: {reason}"
                );
            }
            // recipe-runner-rs present: the invalid recipe exits non-zero, which
            // the thin trigger records as Ok(false) (no stdout parsing).
            Ok(false) => {}
            other => panic!("expected Err(spawn failed) or Ok(false), got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // truncate helper
    // ------------------------------------------------------------------

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn truncate_zero_max() {
        let result = truncate("hello", 0);
        assert_eq!(result, "…");
    }

    // ------------------------------------------------------------------
    // Issue #4803 — anti-thrash hysteresis + persistent backoff (TDD).
    //
    // The emergency cleanup currently re-fires every timer tick above 95%,
    // deleting target/debug + target/llvm-cov-target and pruning backups,
    // only for cargo to instantly rebuild and refill `/` within one cycle
    // (~25 min thrash observed 21:58→00:42). Post-relocation `/` is no
    // longer the fill target, but the cleanup itself must ALSO gain a
    // durable low-watermark + backoff so it can never thrash within one
    // build window. These pin the pure decision seams:
    //   - `should_fire_emergency_cleanup` — hysteresis + time-backoff gate
    //   - `emergency_refire_min_secs_from` — env parse + clamp [0, 86400]
    //   - watermark band constants (high=95 / low=85)
    // ------------------------------------------------------------------

    use std::time::{Duration, SystemTime};

    #[test]
    fn watermark_band_is_valid_hysteresis() {
        // High must strictly exceed low so the two form a real hysteresis
        // band (fire high, re-arm only after dropping below low), never a
        // single edge that re-triggers on every tick. Bound to locals so the
        // comparison is evaluated at runtime (clippy::assertions_on_constants).
        let high = EMERGENCY_HIGH_WATERMARK_PCT;
        let low = EMERGENCY_LOW_WATERMARK_PCT;
        assert!(
            high > low,
            "high watermark ({high}) must exceed low watermark ({low})"
        );
        assert_eq!(
            EMERGENCY_HIGH_WATERMARK_PCT, 95,
            "high watermark must remain the documented ≥95% trigger"
        );
        assert_eq!(
            EMERGENCY_LOW_WATERMARK_PCT, 85,
            "low watermark must remain the documented 85% re-arm line"
        );
    }

    #[test]
    fn below_high_watermark_never_fires() {
        let now = SystemTime::now();
        // No prior run and 94% is still below the 95% trigger.
        assert!(
            !should_fire_emergency_cleanup(94, None, now, DEFAULT_EMERGENCY_MIN_REFIRE_SECS),
            "94% (below high watermark) must not trigger cleanup even on first look"
        );
        assert!(
            !should_fire_emergency_cleanup(
                EMERGENCY_LOW_WATERMARK_PCT,
                None,
                now,
                DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
            ),
            "at the low watermark cleanup must not fire"
        );
    }

    #[test]
    fn at_or_above_high_watermark_fires_on_first_run() {
        let now = SystemTime::now();
        assert!(
            should_fire_emergency_cleanup(95, None, now, DEFAULT_EMERGENCY_MIN_REFIRE_SECS),
            "≥95% with no prior run must fire (no backoff marker yet)"
        );
        assert!(
            should_fire_emergency_cleanup(100, None, now, DEFAULT_EMERGENCY_MIN_REFIRE_SECS),
            "100% with no prior run must fire"
        );
    }

    #[test]
    fn backoff_suppresses_refire_within_window() {
        // This is the core anti-thrash contract: even at 99% full, if the
        // last cleanup ran more recently than the min-refire window, the
        // next tick must be SUPPRESSED — otherwise we recreate the observed
        // delete-then-rebuild-then-delete crash-loop.
        let now = SystemTime::now();
        let last_run = now - Duration::from_secs(60); // 1 min ago
        assert!(
            !should_fire_emergency_cleanup(99, Some(last_run), now, 900),
            "a cleanup 60s ago with a 900s window must be suppressed (anti-thrash)"
        );
    }

    #[test]
    fn backoff_allows_refire_after_window() {
        let now = SystemTime::now();
        let last_run = now - Duration::from_secs(1_000); // > 900s window
        assert!(
            should_fire_emergency_cleanup(99, Some(last_run), now, 900),
            "once the min-refire window has elapsed, a genuinely-full disk may fire again"
        );
    }

    #[test]
    fn zero_min_refire_disables_backoff() {
        // An explicit 0 disables the backoff gate: every tick at/above the
        // high watermark fires. This is the documented escape hatch.
        let now = SystemTime::now();
        let last_run = now - Duration::from_secs(1);
        assert!(
            should_fire_emergency_cleanup(96, Some(last_run), now, 0),
            "min_refire_secs == 0 must disable the backoff gate"
        );
    }

    #[test]
    fn refire_secs_default_when_unset_or_invalid() {
        assert_eq!(
            emergency_refire_min_secs_from(None),
            DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
            "unset env must yield the 900s default"
        );
        assert_eq!(
            emergency_refire_min_secs_from(Some("")),
            DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
            "empty string must fall back to default"
        );
        assert_eq!(
            emergency_refire_min_secs_from(Some("not-a-number")),
            DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
            "unparseable value must fall back to default"
        );
        assert_eq!(
            emergency_refire_min_secs_from(Some("-5")),
            DEFAULT_EMERGENCY_MIN_REFIRE_SECS,
            "negative value must fall back to default (u64 parse fails)"
        );
    }

    #[test]
    fn refire_secs_parses_and_clamps() {
        assert_eq!(
            emergency_refire_min_secs_from(Some("300")),
            300,
            "a valid in-range value must be honored"
        );
        assert_eq!(
            emergency_refire_min_secs_from(Some("0")),
            0,
            "an explicit 0 (disable backoff) is in-range and honored"
        );
        assert_eq!(
            emergency_refire_min_secs_from(Some("99999999")),
            86_400,
            "values above the 86400s (24h) ceiling must clamp down"
        );
    }
}
