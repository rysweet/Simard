//! Recurring **monthly self-quality-audit** periodic task (issue #2419).
//!
//! A thin Rust shim modeled on [`crate::disk_health`] — a **pure recipe
//! invoker** with no memory RPCs — that fires on its own env-gated interval,
//! spawns `recipe-runner-rs` to run the five-wave, crusty-gated self-audit
//! recipe against Simard's own repository, deserializes the JSON envelope,
//! parses text markers into a [`SelfQualityAuditReport`], and — uniquely among
//! the daemon's periodic tasks — **persists its last-run timestamp to disk** so
//! a ~30-day cadence survives daemon restarts.
//!
//! Split of labor: this Rust hook owns the interval gate, disk-backed last-run
//! persistence, subprocess spawn, marker parsing, and logging. The recipe (a
//! `recipe-runner-rs` subprocess) owns all LLM judgment — the five
//! SEEK→VALIDATE→FIX quality-audit waves, the bounded `crusty-old-engineer`
//! proxy-review loop, and the self-merge decisions.
//!
//! Unlike `brain_introspection` (best-effort/graceful), the self-audit follows
//! the `disk_health` **no-fallback** contract: any recipe failure propagates as
//! [`SimardError::AdapterInvocationFailed`]; the daemon WARNs and continues, and
//! persists last-run regardless (on `Ok` AND `Err`) to prevent hot-looping a
//! failing recipe for a full month.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;
use crate::self_quality_audit_record::read_verified_self_quality_audit;

/// Stable adapter tag used in error envelopes and logs.
const ADAPTER_TAG: &str = "monthly-self-quality-audit";
/// Recipe asset filename (resolved hot-reload-first, then in-tree).
const RECIPE_FILENAME: &str = "monthly-self-quality-audit.yaml";
/// Basename of the disk-backed last-run marker file under `state_root`.
pub const LAST_RUN_FILENAME: &str = "self_quality_audit_last_run";

/// Default cadence: run the self-audit once every ~30 days.
pub const DEFAULT_INTERVAL_SECS: u64 = 2_592_000; // 30 * 24 * 60 * 60

// ───────────────────────────────────────────────────────────────────────────
// Config knobs — env parsing + scheduling gate
// ───────────────────────────────────────────────────────────────────────────

/// Parse `SIMARD_SELF_AUDIT_INTERVAL` (value in seconds). A valid `0` is
/// honored as "disabled" (does NOT fall back to the default); empty or
/// unparseable input → the default. Surrounding whitespace is tolerated.
pub fn interval_secs_from_env(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

/// Daemon interval gate. `interval_secs == 0` disables the audit entirely;
/// otherwise it is due once `elapsed >= interval` (inclusive boundary).
pub fn should_run_self_audit(elapsed: Duration, interval_secs: u64) -> bool {
    interval_secs > 0 && elapsed >= Duration::from_secs(interval_secs)
}

// ───────────────────────────────────────────────────────────────────────────
// Disk-backed last-run persistence — the one capability sibling tasks lack
// ───────────────────────────────────────────────────────────────────────────

/// Current wall-clock time as unix epoch seconds (the quantity persisted by
/// [`write_last_run`]). Returns 0 on the impossible pre-epoch clock case.
pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read the persisted last-run epoch seconds. An absent file or unparseable
/// contents both yield `None` (the daemon then initializes to now), so a
/// corrupt marker never crashes the loop.
pub fn read_last_run(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Persist the last-run epoch seconds, creating any missing parent directories
/// (the daemon may write before the state subtree exists).
pub fn write_last_run(path: &Path, epoch_secs: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, epoch_secs.to_string())
}

// ───────────────────────────────────────────────────────────────────────────
// Structured report
// ───────────────────────────────────────────────────────────────────────────

/// Structured result of one self-quality-audit run, built from the typed record
/// the recipe writes via its gated ACT step (read fail-closed by
/// [`read_verified_self_quality_audit`](crate::self_quality_audit_record::read_verified_self_quality_audit)).
#[derive(Debug, Clone, PartialEq)]
pub struct SelfQualityAuditReport {
    /// Number of SEEK→VALIDATE→FIX waves that reached completion
    /// (`WAVE_COMPLETE=` marker count, not the numeric wave label).
    pub waves_completed: u32,
    /// Pull request URLs opened across all waves (`PR_OPENED=`).
    pub prs_opened: Vec<String>,
    /// Pull request URLs self-merged (`PR_MERGED=`).
    pub prs_merged: Vec<String>,
    /// Pull request URLs crusty-old-engineer approved (`CRUSTY_APPROVED=`).
    pub crusty_approved: Vec<String>,
    /// Pull request URLs left open after the bounded crusty loop gave up
    /// (`CRUSTY_UNRESOLVED=`) — surfaced for human follow-up.
    pub crusty_unresolved: Vec<String>,
    /// The agent's own terminal one-line summary (`AUDIT_COMPLETE=`).
    pub summary_line: String,
}

impl SelfQualityAuditReport {
    /// One-line completion summary suitable for the daemon log.
    pub fn summary(&self) -> String {
        format!(
            "self quality-audit complete: {} wave(s), {} PR(s) opened, {} merged, \
             {} crusty-approved, {} crusty-unresolved — {}",
            self.waves_completed,
            self.prs_opened.len(),
            self.prs_merged.len(),
            self.crusty_approved.len(),
            self.crusty_unresolved.len(),
            self.summary_line,
        )
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Recipe invocation (disk_health no-fallback model) — typed-record read path
// ───────────────────────────────────────────────────────────────────────────

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// `home_override` lets tests supply a fake home directory without mutating the
/// process-wide `HOME` environment variable. Returns `None` when neither path
/// holds the recipe file (before any subprocess spawn or config load).
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

/// Build the `recipe-runner-rs` [`Command`] for the self-quality-audit recipe.
///
/// Sets the recipe path, JSON output format, the `state_root` / `repo_path` /
/// `record_path` context vars. Exports `AMPLIHACK_AGENT_BINARY` (Copilot/Claude
/// parity) and
/// [`WORKFLOW_PR_LABELS_ENV`](crate::overseer::config::WORKFLOW_PR_LABELS_ENV) =
/// [`SIMARD_ENGINEER_PR_LABEL`](crate::overseer::config::SIMARD_ENGINEER_PR_LABEL)
/// so the PRs this monthly audit opens against rysweet/Simard carry the durable
/// engineer marker and are visible to the self-merge queue (#4097). Inert until
/// the amplihack publish consumer (#979) lands.
///
/// `record_path` is the ABSOLUTE path the recipe's gated ACT step
/// (`simard cognition record-self-quality-audit --record-path {{record_path}}`)
/// writes its typed record to, and which the rail then reads fail-closed.
///
/// Extracted as a seam so the env contract is unit-testable via
/// [`Command::get_envs`] without spawning `recipe-runner-rs`.
fn build_audit_command(
    recipe_path: &Path,
    state_root: &Path,
    repo_root: &Path,
    record_path: &Path,
    agent_binary: &str,
) -> Command {
    let mut cmd = Command::new("recipe-runner-rs");
    cmd.arg(recipe_path.as_os_str())
        .arg("--output-format")
        .arg("json")
        .env("AMPLIHACK_AGENT_BINARY", agent_binary)
        .env(
            crate::overseer::config::WORKFLOW_PR_LABELS_ENV,
            crate::overseer::config::SIMARD_ENGINEER_PR_LABEL,
        )
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
        .arg("-c")
        .arg(format!("repo_path={}", repo_root.display()))
        .arg("-c")
        .arg(format!("record_path={}", record_path.display()));
    cmd
}

/// The per-run record path: one file under the state root, pre-truncated each
/// invocation so a prior month's record can never be read as current.
fn audit_record_path(state_root: &Path) -> PathBuf {
    state_root.join("self_quality_audit").join("record.json")
}

/// Run the monthly self-quality-audit recipe via `recipe-runner-rs`.
///
/// `repo_root` locates the recipe YAML and is passed to the recipe as the
/// `repo_path` context var. `state_root` (typically `~/.simard`) is passed as
/// the `state_root` context var. `home_override` lets tests point at a fake
/// home for recipe resolution.
///
/// No-fallback contract (mirrors [`crate::disk_health::run_disk_health_check`]):
/// a missing recipe, a spawn failure, a non-zero exit, or a fail-closed record
/// read (R1–R7) all become [`SimardError::AdapterInvocationFailed`]. The result
/// is sourced ONLY from the typed record the recipe wrote via its gated tool
/// call — NEVER scraped from stdout.
pub fn run_self_quality_audit(
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
) -> SimardResult<SelfQualityAuditReport> {
    let recipe_path = resolve_recipe_path(repo_root, home_override).ok_or_else(|| {
        SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "recipe file {RECIPE_FILENAME} not found in hot-reload or in-tree paths"
            ),
        }
    })?;

    let agent_binary = RuntimeConfig::load()?.llm_provider.agent_binary_value();

    // Anti-replay: derive + PRE-TRUNCATE the record path, then capture
    // `invoke_start` BEFORE spawn so a record written this run has
    // `mtime >= invoke_start` (R7).
    let record_path = audit_record_path(state_root);
    let _ = std::fs::remove_file(&record_path);
    let invoke_start = SystemTime::now();

    let status = build_audit_command(
        &recipe_path,
        state_root,
        repo_root,
        &record_path,
        agent_binary,
    )
    .status()
    .map_err(|e| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!("recipe-runner-rs spawn failed: {e}"),
    })?;

    if !status.success() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!("recipe exited with {status}"),
        });
    }

    // The recipe exited 0 — the ONLY source of truth is the typed record it
    // wrote via its gated tool call. Read it FAIL-CLOSED (R1–R7): a recipe that
    // "ran" but wrote no valid record is a hard failure, never a silent default.
    let record = read_verified_self_quality_audit(&record_path, invoke_start)?;

    Ok(SelfQualityAuditReport {
        waves_completed: record.waves_completed,
        prs_opened: record.prs_opened,
        prs_merged: record.prs_merged,
        crusty_approved: record.crusty_approved,
        crusty_unresolved: record.crusty_unresolved,
        summary_line: record.summary_line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// Seam (d): the monthly self-quality-audit runs `recipe-runner-rs` and
    /// opens PRs against rysweet/Simard, so its Command must export
    /// `WORKFLOW_PR_LABELS` alongside the existing `AMPLIHACK_AGENT_BINARY`.
    /// Extracting `build_audit_command` makes the env contract unit-testable via
    /// `Command::get_envs()` without spawning `recipe-runner-rs`.
    #[test]
    fn build_audit_command_exports_workflow_pr_labels() {
        let cmd = build_audit_command(
            Path::new("/tmp/recipe.yaml"),
            Path::new("/home/agent/.simard"),
            Path::new("/home/agent/src/Simard"),
            Path::new("/home/agent/.simard/self_quality_audit/record.json"),
            "copilot",
        );

        assert_eq!(
            cmd.get_program(),
            OsStr::new("recipe-runner-rs"),
            "the audit command must invoke recipe-runner-rs"
        );

        let labels_set = cmd.get_envs().any(|(k, v)| {
            k == OsStr::new(crate::overseer::config::WORKFLOW_PR_LABELS_ENV)
                && v == Some(OsStr::new(
                    crate::overseer::config::SIMARD_ENGINEER_PR_LABEL,
                ))
        });
        assert!(
            labels_set,
            "build_audit_command must set WORKFLOW_PR_LABELS=simard-autonomous \
             via the shared constants"
        );

        // The pre-existing agent-binary contract must be preserved.
        let agent_binary_set = cmd.get_envs().any(|(k, v)| {
            k == OsStr::new("AMPLIHACK_AGENT_BINARY") && v == Some(OsStr::new("copilot"))
        });
        assert!(
            agent_binary_set,
            "build_audit_command must still forward AMPLIHACK_AGENT_BINARY"
        );
    }
}
