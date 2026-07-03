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

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;

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
// Structured report + marker parser
// ───────────────────────────────────────────────────────────────────────────

/// Structured result of one self-quality-audit run, built by parsing text
/// markers from the recipe's stdout.
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

/// Parse the self-quality-audit text markers from recipe stdout.
///
/// Recognized markers (each on its own line; surrounding whitespace tolerated):
/// ```text
/// AUDIT_STARTED                 # advisory, ignored
/// WAVE_START=<n>                # advisory, ignored (does NOT count as complete)
/// WAVE_COMPLETE=<n>             # counted into waves_completed
/// PR_OPENED=<url>               # collected in order
/// PR_MERGED=<url>               # collected in order
/// CRUSTY_APPROVED=<url>         # collected in order
/// CRUSTY_UNRESOLVED=<url>       # collected in order
/// AUDIT_COMPLETE=<summary>      # REQUIRED, non-empty terminal marker
/// ```
///
/// A missing or empty `AUDIT_COMPLETE` marker is a hard parse error: without a
/// terminal marker the run is not trustworthy. Any unrecognized line is
/// silently ignored (forward-compatible with human-readable agent prose).
pub fn parse_self_quality_audit_text(stdout: &str) -> Result<SelfQualityAuditReport, String> {
    let mut waves_completed: u32 = 0;
    let mut prs_opened: Vec<String> = Vec::new();
    let mut prs_merged: Vec<String> = Vec::new();
    let mut crusty_approved: Vec<String> = Vec::new();
    let mut crusty_unresolved: Vec<String> = Vec::new();
    let mut summary_line: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(val) = trimmed.strip_prefix("AUDIT_COMPLETE=") {
            summary_line = Some(val.trim().to_string());
        } else if trimmed.strip_prefix("WAVE_COMPLETE=").is_some() {
            waves_completed += 1;
        } else if let Some(val) = trimmed.strip_prefix("PR_OPENED=") {
            push_url(&mut prs_opened, val);
        } else if let Some(val) = trimmed.strip_prefix("PR_MERGED=") {
            push_url(&mut prs_merged, val);
        } else if let Some(val) = trimmed.strip_prefix("CRUSTY_APPROVED=") {
            push_url(&mut crusty_approved, val);
        } else if let Some(val) = trimmed.strip_prefix("CRUSTY_UNRESOLVED=") {
            push_url(&mut crusty_unresolved, val);
        }
        // AUDIT_STARTED, WAVE_START=, and all unknown lines are ignored.
    }

    let summary_line =
        summary_line.ok_or_else(|| "missing AUDIT_COMPLETE marker in recipe output".to_string())?;
    if summary_line.is_empty() {
        return Err("AUDIT_COMPLETE marker has an empty summary".to_string());
    }

    Ok(SelfQualityAuditReport {
        waves_completed,
        prs_opened,
        prs_merged,
        crusty_approved,
        crusty_unresolved,
        summary_line,
    })
}

/// Trim a marker value and push it onto `dst` when non-empty.
fn push_url(dst: &mut Vec<String>, raw: &str) {
    let v = raw.trim();
    if !v.is_empty() {
        dst.push(v.to_string());
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Recipe invocation (disk_health no-fallback model)
// ───────────────────────────────────────────────────────────────────────────

/// JSON envelope returned by `recipe-runner-rs --output-format json`. Extra
/// fields (e.g. `step_id`) are ignored by serde.
#[derive(Debug, Deserialize)]
struct RecipeOutput {
    success: bool,
    step_results: Vec<StepResult>,
}

/// A single step's result inside the [`RecipeOutput`] envelope.
#[derive(Debug, Deserialize)]
struct StepResult {
    output: String,
}

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

/// Run the monthly self-quality-audit recipe via `recipe-runner-rs`.
///
/// `repo_root` locates the recipe YAML and is passed to the recipe as the
/// `repo_path` context var. `state_root` (typically `~/.simard`) is passed as
/// the `state_root` context var. `home_override` lets tests point at a fake
/// home for recipe resolution.
///
/// No-fallback contract (mirrors [`crate::disk_health::run_disk_health_check`]):
/// a missing recipe, a spawn failure, a non-zero exit, `success=false`, or an
/// unparseable marker set all become [`SimardError::AdapterInvocationFailed`].
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

    let output = Command::new("recipe-runner-rs")
        .arg(recipe_path.as_os_str())
        .arg("--output-format")
        .arg("json")
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

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "recipe exited with {}: {}",
                output.status,
                truncate(&stderr, 500)
            ),
        });
    }

    let envelope: RecipeOutput = serde_json::from_slice(&output.stdout).map_err(|e| {
        SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!("failed to deserialize recipe JSON output: {e}"),
        }
    })?;

    if !envelope.success {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: "recipe reported success=false in JSON output".to_string(),
        });
    }

    if envelope.step_results.is_empty() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: "no step results in recipe JSON output".to_string(),
        });
    }

    // Concatenate every step's output so terminal markers emitted by any step
    // (the orchestrator prints them last) are captured.
    let combined = envelope
        .step_results
        .iter()
        .map(|s| s.output.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    parse_self_quality_audit_text(&combined).map_err(|e| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!("failed to parse recipe text output: {e}"),
    })
}

/// Truncate `s` to at most `max` characters, appending an ellipsis if cut.
fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}
