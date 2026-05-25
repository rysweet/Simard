//! Recipe-runner-backed disk health check (issue #2020).
//!
//! Invokes `recipe-runner-rs` executing
//! `prompt_assets/simard/recipes/disk-health-check.yaml` as a subprocess,
//! checks disk usage, triggers cleanup when usage exceeds 80%, and returns
//! a structured [`DiskHealthReport`].
//!
//! The recipe YAML contains an agent step that adaptively decides what to
//! clean based on disk pressure; this module is a thin Rust shim that:
//!   1. Resolves the recipe path (hot-reload → in-tree fallback)
//!   2. Spawns `recipe-runner-rs` with `-c` context vars
//!   3. Parses stdout text markers into [`DiskHealthReport`]
//!   4. Logs results to daemon.log
//!
//! Follows the same pattern as `stewardship::recipe_merge_judge`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};

const ADAPTER_TAG: &str = "disk-health-check";
const RECIPE_FILENAME: &str = "disk-health-check.yaml";

/// Structured report returned by the disk-health-check recipe.
///
/// The recipe's agent step outputs text markers to stdout, parsed by
/// [`parse_disk_health_text`].
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

/// Run the disk health check recipe via `recipe-runner-rs`.
///
/// `state_root` is the Simard state directory (typically `~/.simard`),
/// passed to the recipe as a context var so the agent knows where to
/// find worktrees, backups, and cargo target dirs.
///
/// `repo_root` is used to locate the recipe YAML file.
///
/// Returns the parsed [`DiskHealthReport`] on success, or a
/// [`SimardError::AdapterInvocationFailed`] on any failure.
pub fn run_disk_health_check(
    repo_root: &Path,
    state_root: &Path,
) -> SimardResult<DiskHealthReport> {
    let recipe_path =
        resolve_recipe_path(repo_root).ok_or_else(|| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "recipe file {RECIPE_FILENAME} not found in hot-reload or in-tree paths"
            ),
        })?;

    let output = Command::new("recipe-runner-rs")
        .arg(recipe_path.as_os_str())
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
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

    let stdout_text = String::from_utf8_lossy(&output.stdout);
    parse_disk_health_text(&stdout_text).map_err(|e| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!("failed to parse recipe text output: {e}"),
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
/// Expected format (the agent step emits these markers):
/// ```text
/// DISK_USED_PCT=87
/// FREED_BYTES=1024
/// ACTION: removed stale worktrees
/// ACTION: cleaned cargo target dirs
/// ```
///
/// Non-marker lines are silently ignored, making this forward-compatible
/// with additional agent output.
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
    // run_disk_health_check — error paths (no recipe-runner-rs needed)
    // ------------------------------------------------------------------

    #[test]
    fn run_returns_error_when_recipe_not_found() {
        let result = run_disk_health_check(
            Path::new("/nonexistent/repo"),
            Path::new("/nonexistent/state"),
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
    fn run_returns_error_when_recipe_runner_unavailable_or_recipe_invalid() {
        // Create a syntactically-invalid recipe file. If recipe-runner-rs
        // is installed it will reject it (non-zero exit); if it's missing
        // the spawn itself fails. Either way we get AdapterInvocationFailed.
        let tmp = tempfile::tempdir().unwrap();
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

        let result = run_disk_health_check(tmp.path(), tmp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        match &err {
            SimardError::AdapterInvocationFailed { base_type, reason } => {
                assert_eq!(base_type, ADAPTER_TAG);
                // Either "spawn failed" (binary missing) or "recipe exited"
                // (binary found, recipe invalid).
                assert!(
                    reason.contains("spawn failed") || reason.contains("recipe exited"),
                    "reason should mention spawn failure or recipe exit: {reason}"
                );
            }
            other => panic!("expected AdapterInvocationFailed, got: {other:?}"),
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
    // Recipe YAML contract tests (TDD — written before implementation)
    //
    // These tests validate that disk-health-check.yaml satisfies the
    // agent-step contract defined in issue #2051. They read the in-tree
    // recipe file and assert structural + content properties.
    // ------------------------------------------------------------------

    fn load_recipe_yaml() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let recipe_path = std::path::Path::new(manifest_dir)
            .join("prompt_assets/simard/recipes/disk-health-check.yaml");
        std::fs::read_to_string(&recipe_path).unwrap_or_else(|e| {
            panic!(
                "failed to read recipe YAML at {}: {e}",
                recipe_path.display()
            )
        })
    }

    #[test]
    fn recipe_yaml_uses_agent_step_not_bash() {
        let yaml = load_recipe_yaml();
        assert!(
            !yaml.contains("type: \"bash\"") && !yaml.contains("type: 'bash'"),
            "recipe must NOT contain a bash step — should be agent-based"
        );
        assert!(
            yaml.contains("type: \"agent\"") || yaml.contains("type: 'agent'"),
            "recipe must contain an agent step"
        );
    }

    #[test]
    fn recipe_yaml_has_version_2() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("version: \"2.0.0\"") || yaml.contains("version: '2.0.0'"),
            "recipe version must be 2.0.0 (behavioral change: bash → agent)"
        );
    }

    #[test]
    fn recipe_yaml_is_single_step() {
        let yaml = load_recipe_yaml();
        // Count occurrences of "- id:" which marks step boundaries
        let step_count = yaml
            .lines()
            .filter(|l| {
                let trimmed = l.trim();
                trimmed.starts_with("- id:")
            })
            .count();
        assert_eq!(
            step_count, 1,
            "recipe must have exactly one step, found {step_count}"
        );
    }

    #[test]
    fn recipe_yaml_preserves_context_state_root() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("state_root"),
            "recipe must define state_root context variable"
        );
        assert!(
            yaml.contains("~/.simard"),
            "state_root default must be ~/.simard"
        );
    }

    #[test]
    fn recipe_yaml_prompt_instructs_disk_used_pct_marker() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("DISK_USED_PCT="),
            "agent prompt must instruct emission of DISK_USED_PCT= marker"
        );
    }

    #[test]
    fn recipe_yaml_prompt_instructs_freed_bytes_marker() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("FREED_BYTES="),
            "agent prompt must instruct emission of FREED_BYTES= marker"
        );
    }

    #[test]
    fn recipe_yaml_prompt_instructs_action_marker() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("ACTION:"),
            "agent prompt must instruct emission of ACTION: markers"
        );
    }

    #[test]
    fn recipe_yaml_prompt_mentions_cleanup_targets() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("engineer-worktrees"),
            "prompt must mention engineer-worktrees cleanup target"
        );
        assert!(
            yaml.contains("backups"),
            "prompt must mention backups cleanup target"
        );
        assert!(
            yaml.contains("cargo-target") || yaml.contains("shared-target"),
            "prompt must mention cargo/shared target cleanup targets"
        );
    }

    #[test]
    fn recipe_yaml_prompt_mentions_claim_file_check() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains(".simard-engineer-claim"),
            "prompt must describe .simard-engineer-claim PID-check pattern"
        );
    }

    #[test]
    fn recipe_yaml_prompt_mentions_df_measurement() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("df") || yaml.contains("disk usage"),
            "prompt must instruct df-based disk measurement"
        );
    }

    #[test]
    fn recipe_yaml_prompt_mentions_80_percent_threshold() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("80%") || yaml.contains("80 %") || yaml.contains("eighty"),
            "prompt must mention the 80% threshold"
        );
    }

    #[test]
    fn recipe_yaml_has_no_hardcoded_rm_or_find() {
        let yaml = load_recipe_yaml();
        // The recipe itself (outside the agent prompt) must not contain
        // hardcoded bash commands. The YAML "command:" field should be gone.
        assert!(
            !yaml.contains("command: |"),
            "recipe must not contain a bash command block — cleanup logic belongs to the agent"
        );
    }

    #[test]
    fn recipe_yaml_agent_step_has_prompt() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("prompt: |") || yaml.contains("prompt: >"),
            "agent step must have a multi-line prompt field"
        );
    }

    #[test]
    fn recipe_yaml_agent_step_uses_default_agent() {
        let yaml = load_recipe_yaml();
        assert!(
            yaml.contains("agent: \"default\"") || yaml.contains("agent: 'default'"),
            "agent step must use agent: \"default\""
        );
    }
}
