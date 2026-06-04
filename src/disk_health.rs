//! Recipe-runner-backed disk health check (issue #2020).
//!
//! Invokes `recipe-runner-rs` executing
//! `prompt_assets/simard/recipes/disk-health-check.yaml` as a subprocess,
//! checks disk usage, triggers cleanup when usage exceeds 80%, and returns
//! a structured JSON report.
//!
//! The recipe YAML contains the deterministic bash cleanup logic; this
//! module is a thin Rust shim that:
//!   1. Resolves the recipe path (hot-reload → in-tree fallback)
//!   2. Spawns `recipe-runner-rs` with `-c` context vars
//!   3. Parses stdout JSON into [`DiskHealthReport`]
//!   4. Logs results to daemon.log
//!
//! Follows the same pattern as `stewardship::recipe_merge_judge`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;

const ADAPTER_TAG: &str = "disk-health-check";
const RECIPE_FILENAME: &str = "disk-health-check.yaml";

/// Structured report returned by the disk-health-check recipe.
///
/// The recipe's bash step outputs this as JSON to stdout.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DiskHealthReport {
    /// Current disk usage percentage (0–100).
    pub disk_used_pct: u8,
    /// Total bytes freed during this check (0 if no cleanup needed).
    pub freed_bytes: u64,
    /// Human-readable list of cleanup actions taken.
    pub actions_taken: Vec<String>,
}

impl DiskHealthReport {
    /// Whether cleanup was actually performed (usage was above threshold).
    pub fn cleanup_performed(&self) -> bool {
        self.freed_bytes > 0 || !self.actions_taken.is_empty()
    }

    /// One-line summary suitable for daemon log.
    pub fn summary(&self) -> String {
        if self.cleanup_performed() {
            format!(
                "disk health: {}% used, freed {} bytes, {} actions",
                self.disk_used_pct,
                self.freed_bytes,
                self.actions_taken.len()
            )
        } else {
            format!(
                "disk health: {}% used, no cleanup needed",
                self.disk_used_pct
            )
        }
    }
}

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
fn resolve_recipe_path(repo_root: &Path) -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
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

/// Run the disk health check. Uses recipe-runner-rs with JSON output to
/// extract the agent's step output, which contains the DISK_USED_PCT markers.
///
/// Falls back to a deterministic bash check only if recipe-runner-rs is
/// completely unavailable (binary missing, recipe file missing).
pub fn run_disk_health_check(
    repo_root: &Path,
    state_root: &Path,
) -> SimardResult<DiskHealthReport> {
    match run_recipe_disk_check(repo_root, state_root) {
        Ok(report) => Ok(report),
        Err(e) => {
            eprintln!(
                "[simard] disk health: recipe failed ({e}), falling back to deterministic check"
            );
            run_deterministic_disk_check(repo_root, state_root)
        }
    }
}

/// JSON envelope from recipe-runner-rs `--output-format json`.
#[derive(Debug, Deserialize)]
struct RecipeOutput {
    #[allow(dead_code)]
    success: bool,
    step_results: Vec<StepResult>,
}

#[derive(Debug, Deserialize)]
struct StepResult {
    #[allow(dead_code)]
    step_id: String,
    output: String,
}

/// Recipe-based disk health check via `recipe-runner-rs`.
///
/// Uses `--output-format json` to capture the agent's actual output from
/// `step_results[].output`, then parses the DISK_USED_PCT markers from that.
///
/// Previous bug: text-format stdout only contains recipe-runner's summary
/// (e.g. "Recipe: ... SUCCESS"), NOT the agent's conversation output where
/// the DISK_USED_PCT markers live. JSON format exposes step_results[].output.
fn run_recipe_disk_check(repo_root: &Path, state_root: &Path) -> SimardResult<DiskHealthReport> {
    let recipe_path =
        resolve_recipe_path(repo_root).ok_or_else(|| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "recipe file {RECIPE_FILENAME} not found in hot-reload or in-tree paths"
            ),
        })?;

    let agent_binary = RuntimeConfig::load()?.llm_provider.agent_binary_value();

    let output = Command::new("recipe-runner-rs")
        .arg(recipe_path.as_os_str())
        .env("AMPLIHACK_AGENT_BINARY", agent_binary)
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
        .arg("--output-format")
        .arg("json")
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

    let stdout_bytes = &output.stdout;
    let recipe_output: RecipeOutput =
        serde_json::from_slice(stdout_bytes).map_err(|e| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "failed to parse recipe JSON: {e}. stdout: {}",
                truncate(&String::from_utf8_lossy(stdout_bytes), 200)
            ),
        })?;

    let step_output = recipe_output
        .step_results
        .first()
        .map(|s| s.output.as_str())
        .unwrap_or("");

    parse_disk_health_text(step_output).map_err(|e| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!(
            "agent output missing markers: {e}. output: {}",
            truncate(step_output, 300)
        ),
    })
}

/// Deterministic disk health check using plain shell commands (no LLM).
/// Last-resort fallback when recipe-runner-rs is unavailable.
fn run_deterministic_disk_check(
    repo_root: &Path,
    state_root: &Path,
) -> SimardResult<DiskHealthReport> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home"));
    let script = format!(
        r#"set -e
USAGE_LINE=$(df -B1 --output=pcent,avail "{home}" 2>/dev/null | tail -1)
PCT=$(echo "$USAGE_LINE" | awk '{{gsub(/%/,""); print $1}}')
AVAIL_BEFORE=$(echo "$USAGE_LINE" | awk '{{print $2}}')
echo "DISK_USED_PCT=$PCT"
if [ "$PCT" -lt 80 ] 2>/dev/null; then
    echo "FREED_BYTES=0"
    exit 0
fi
for tgt in "{repo}"/worktrees/*/target/debug \
           "{repo}"/worktrees/*/worktrees/*/target/debug \
           "{state}"/engineer-worktrees/*/target/debug; do
    if [ -d "$tgt" ]; then
        rm -rf "$tgt" 2>/dev/null && echo "ACTION: removed $tgt"
    fi
done
if [ -d "{repo}"/worktrees/main/target/debug ]; then
    rm -rf "{repo}"/worktrees/main/target/debug 2>/dev/null && echo "ACTION: removed main debug target"
fi
cd "{repo}" && git gc --auto --quiet 2>/dev/null && echo "ACTION: git gc --auto"
AVAIL_AFTER=$(df -B1 --output=avail "{home}" 2>/dev/null | tail -1 | awk '{{print $1}}')
if [ "$AVAIL_AFTER" -gt "$AVAIL_BEFORE" ] 2>/dev/null; then
    FREED=$((AVAIL_AFTER - AVAIL_BEFORE))
else
    FREED=0
fi
echo "FREED_BYTES=$FREED"
"#,
        home = home.display(),
        repo = repo_root.display(),
        state = state_root.display(),
    );

    let output = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .output()
        .map_err(|e| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!("deterministic disk check: bash spawn failed: {e}"),
        })?;

    let stdout_text = String::from_utf8_lossy(&output.stdout);
    parse_disk_health_text(&stdout_text).map_err(|e| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!(
            "deterministic disk check: parse failed: {e}. stdout: {}",
            truncate(&stdout_text, 200)
        ),
    })
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

/// Parse key=value and ACTION: lines from recipe stdout text.
///
/// Expected format (bash recipe outputs this directly):
/// ```text
/// DISK_USED_PCT=87
/// FREED_BYTES=1024
/// ACTION: removed stale worktrees
/// ACTION: cleaned cargo target dirs
/// ```
///
/// This replaces the brittle `serde_json::from_slice::<DiskHealthReport>`
/// pattern — the recipe is a bash step, not an LLM. Bash can emit key=value
/// lines trivially; asking it to emit valid JSON was the source of fragility.
pub fn parse_disk_health_text(stdout: &str) -> Result<DiskHealthReport, String> {
    let mut disk_used_pct: Option<u8> = None;
    let mut freed_bytes: u64 = 0;
    let mut actions_taken: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(val) = trimmed.strip_prefix("DISK_USED_PCT=") {
            disk_used_pct = Some(
                val.trim()
                    .parse::<u8>()
                    .map_err(|e| format!("invalid DISK_USED_PCT value '{val}': {e}"))?,
            );
        } else if let Some(val) = trimmed.strip_prefix("FREED_BYTES=") {
            freed_bytes = val
                .trim()
                .parse::<u64>()
                .map_err(|e| format!("invalid FREED_BYTES value '{val}': {e}"))?;
        } else if let Some(action) = trimmed.strip_prefix("ACTION:") {
            let action = action.trim();
            if !action.is_empty() {
                actions_taken.push(action.to_string());
            }
        }
        // Unknown lines are silently ignored (forward-compat).
    }

    let disk_used_pct =
        disk_used_pct.ok_or_else(|| "missing DISK_USED_PCT line in recipe output".to_string())?;

    Ok(DiskHealthReport {
        disk_used_pct,
        freed_bytes,
        actions_taken,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Text-based parser (issue #1980 — replaces JSON deserialization)
    // ------------------------------------------------------------------

    #[test]
    fn text_parse_no_cleanup() {
        let text = "DISK_USED_PCT=65\nFREED_BYTES=0\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 65);
        assert_eq!(report.freed_bytes, 0);
        assert!(report.actions_taken.is_empty());
    }

    #[test]
    fn text_parse_with_cleanup_actions() {
        let text = "\
DISK_USED_PCT=87
FREED_BYTES=53687091200
ACTION: removed 12 stale engineer worktrees
ACTION: cleaned cargo target dirs in worktrees
ACTION: pruned LadybugDB backups to 5 most recent
ACTION: cleaned shared-target dir
";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 87);
        assert_eq!(report.freed_bytes, 53_687_091_200);
        assert_eq!(report.actions_taken.len(), 4);
        assert!(report.actions_taken[0].contains("worktrees"));
    }

    #[test]
    fn text_parse_boundary_100_percent() {
        let text = "DISK_USED_PCT=100\nFREED_BYTES=1024\nACTION: emergency cleanup\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 100);
        assert_eq!(report.freed_bytes, 1024);
        assert_eq!(report.actions_taken.len(), 1);
    }

    #[test]
    fn text_parse_boundary_0_percent() {
        let text = "DISK_USED_PCT=0\nFREED_BYTES=0\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 0);
    }

    #[test]
    fn text_parse_missing_disk_used_pct_is_error() {
        let text = "FREED_BYTES=0\n";
        let result = parse_disk_health_text(text);
        assert!(result.is_err(), "should reject missing DISK_USED_PCT");
        assert!(result.unwrap_err().contains("missing DISK_USED_PCT"));
    }

    #[test]
    fn text_parse_invalid_pct_value_is_error() {
        let text = "DISK_USED_PCT=high\nFREED_BYTES=0\n";
        let result = parse_disk_health_text(text);
        assert!(result.is_err(), "should reject non-numeric DISK_USED_PCT");
    }

    #[test]
    fn text_parse_empty_string_is_error() {
        let result = parse_disk_health_text("");
        assert!(result.is_err());
    }

    #[test]
    fn text_parse_freed_bytes_defaults_to_zero_when_absent() {
        let text = "DISK_USED_PCT=50\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.freed_bytes, 0);
        assert!(report.actions_taken.is_empty());
    }

    #[test]
    fn text_parse_ignores_unknown_lines() {
        let text = "DISK_USED_PCT=42\nSOME_OTHER_KEY=foo\nFREED_BYTES=100\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 42);
        assert_eq!(report.freed_bytes, 100);
    }

    #[test]
    fn text_parse_handles_whitespace_around_values() {
        let text = "  DISK_USED_PCT=42  \n  FREED_BYTES=100  \n  ACTION: did things  \n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 42);
        assert_eq!(report.freed_bytes, 100);
        assert_eq!(report.actions_taken, vec!["did things"]);
    }

    #[test]
    fn text_parse_skips_blank_lines() {
        let text = "\n\nDISK_USED_PCT=50\n\nFREED_BYTES=0\n\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.disk_used_pct, 50);
    }

    #[test]
    fn text_parse_action_without_text_is_skipped() {
        let text = "DISK_USED_PCT=50\nACTION:\nACTION: real action\n";
        let report = parse_disk_health_text(text).unwrap();
        assert_eq!(report.actions_taken, vec!["real action"]);
    }

    // ------------------------------------------------------------------
    // DiskHealthReport methods
    // ------------------------------------------------------------------

    #[test]
    fn cleanup_performed_true_when_freed_bytes_nonzero() {
        let report = DiskHealthReport {
            disk_used_pct: 85,
            freed_bytes: 1024,
            actions_taken: vec![],
        };
        assert!(report.cleanup_performed());
    }

    #[test]
    fn cleanup_performed_true_when_actions_nonempty() {
        let report = DiskHealthReport {
            disk_used_pct: 85,
            freed_bytes: 0,
            actions_taken: vec!["did something".to_string()],
        };
        assert!(report.cleanup_performed());
    }

    #[test]
    fn cleanup_performed_false_when_nothing_happened() {
        let report = DiskHealthReport {
            disk_used_pct: 50,
            freed_bytes: 0,
            actions_taken: vec![],
        };
        assert!(!report.cleanup_performed());
    }

    #[test]
    fn summary_no_cleanup() {
        let report = DiskHealthReport {
            disk_used_pct: 42,
            freed_bytes: 0,
            actions_taken: vec![],
        };
        let s = report.summary();
        assert!(s.contains("42%"), "summary should contain pct: {s}");
        assert!(
            s.contains("no cleanup"),
            "summary should say no cleanup: {s}"
        );
    }

    #[test]
    fn summary_with_cleanup() {
        let report = DiskHealthReport {
            disk_used_pct: 87,
            freed_bytes: 53_000_000_000,
            actions_taken: vec!["removed worktrees".to_string(), "cleaned cargo".to_string()],
        };
        let s = report.summary();
        assert!(s.contains("87%"), "summary should contain pct: {s}");
        assert!(s.contains("2 actions"), "summary should count actions: {s}");
        assert!(
            s.contains("53000000000"),
            "summary should contain freed bytes: {s}"
        );
    }

    // ------------------------------------------------------------------
    // resolve_recipe_path
    // ------------------------------------------------------------------

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_dir() {
        let result = resolve_recipe_path(Path::new("/nonexistent/repo"));
        assert!(result.is_none());
    }

    #[test]
    fn resolve_recipe_path_finds_in_tree_recipe() {
        let tmp = tempfile::tempdir().unwrap();
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

        let result = resolve_recipe_path(tmp.path());
        assert!(result.is_some());
        assert!(result.unwrap().ends_with(RECIPE_FILENAME));
    }

    // ------------------------------------------------------------------
    // run_disk_health_check — fallback behavior
    // ------------------------------------------------------------------

    #[test]
    fn run_falls_back_to_deterministic_when_recipe_not_found() {
        // With no recipe file, run_disk_health_check falls back to bash.
        let result = run_disk_health_check(
            Path::new("/nonexistent/repo"),
            Path::new("/nonexistent/state"),
        );
        assert!(result.is_ok(), "fallback should succeed: {result:?}");
        let report = result.unwrap();
        assert!(report.disk_used_pct > 0, "should report real disk usage");
    }

    #[test]
    fn run_falls_back_when_recipe_runner_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

        unsafe { std::env::set_var("SIMARD_LLM_PROVIDER", "copilot") };
        let result = run_disk_health_check(tmp.path(), tmp.path());
        unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };
        assert!(result.is_ok(), "fallback should succeed: {result:?}");
    }

    #[test]
    fn deterministic_check_returns_valid_report() {
        let result = run_deterministic_disk_check(Path::new("/tmp"), Path::new("/tmp"));
        assert!(
            result.is_ok(),
            "deterministic check should succeed: {result:?}"
        );
        let report = result.unwrap();
        assert!(report.disk_used_pct > 0);
        assert!(report.disk_used_pct <= 100);
    }

    #[test]
    fn recipe_check_returns_error_when_recipe_not_found() {
        let result = run_recipe_disk_check(
            Path::new("/nonexistent/repo"),
            Path::new("/nonexistent/state"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn parse_markers_from_agent_output_with_noise() {
        // Simulates real agent output: markers mixed with other text
        let agent_output = "● Measure disk usage (shell)\n  │ df -B1 ...\n  └ 2 lines...\n\n\
            Disk usage is at 46%. No cleanup needed.\n\n\
            DISK_USED_PCT=46\nFREED_BYTES=0\n\n\
            ✅ Copied bin\n✅ Copied agents/amplihack\n";
        let report = parse_disk_health_text(agent_output).unwrap();
        assert_eq!(report.disk_used_pct, 46);
        assert_eq!(report.freed_bytes, 0);
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
}
