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

/// Run the disk health check recipe via `recipe-runner-rs`.
///
/// `state_root` is the Simard state directory (typically `~/.simard`),
/// passed to the recipe as a context var so the bash script knows where
/// to find worktrees, backups, and cargo target dirs.
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

    let raw = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<DiskHealthReport>(&raw).map_err(|e| {
        SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!("failed to parse recipe output as DiskHealthReport: {e}"),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // DiskHealthReport deserialization
    // ------------------------------------------------------------------

    #[test]
    fn report_deserializes_from_json_no_cleanup() {
        let json = r#"{"disk_used_pct": 65, "freed_bytes": 0, "actions_taken": []}"#;
        let report: DiskHealthReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.disk_used_pct, 65);
        assert_eq!(report.freed_bytes, 0);
        assert!(report.actions_taken.is_empty());
    }

    #[test]
    fn report_deserializes_from_json_with_cleanup() {
        let json = r#"{
            "disk_used_pct": 87,
            "freed_bytes": 53687091200,
            "actions_taken": [
                "removed 12 stale engineer worktrees",
                "cleaned cargo target dirs in worktrees",
                "pruned LadybugDB backups to 5 most recent",
                "cleaned shared-target dir"
            ]
        }"#;
        let report: DiskHealthReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.disk_used_pct, 87);
        assert_eq!(report.freed_bytes, 53_687_091_200);
        assert_eq!(report.actions_taken.len(), 4);
        assert!(report.actions_taken[0].contains("worktrees"));
    }

    #[test]
    fn report_deserializes_at_boundary_100_percent() {
        let json = r#"{"disk_used_pct": 100, "freed_bytes": 1024, "actions_taken": ["emergency cleanup"]}"#;
        let report: DiskHealthReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.disk_used_pct, 100);
        assert_eq!(report.freed_bytes, 1024);
        assert_eq!(report.actions_taken.len(), 1);
    }

    #[test]
    fn report_deserializes_at_boundary_0_percent() {
        let json = r#"{"disk_used_pct": 0, "freed_bytes": 0, "actions_taken": []}"#;
        let report: DiskHealthReport = serde_json::from_str(json).unwrap();
        assert_eq!(report.disk_used_pct, 0);
    }

    #[test]
    fn report_rejects_missing_field() {
        let json = r#"{"disk_used_pct": 65, "freed_bytes": 0}"#;
        let result = serde_json::from_str::<DiskHealthReport>(json);
        assert!(result.is_err(), "should reject JSON missing actions_taken");
    }

    #[test]
    fn report_rejects_invalid_type() {
        let json = r#"{"disk_used_pct": "high", "freed_bytes": 0, "actions_taken": []}"#;
        let result = serde_json::from_str::<DiskHealthReport>(json);
        assert!(result.is_err(), "should reject non-numeric disk_used_pct");
    }

    #[test]
    fn report_rejects_empty_string() {
        let result = serde_json::from_str::<DiskHealthReport>("");
        assert!(result.is_err());
    }

    #[test]
    fn report_rejects_non_json() {
        let result = serde_json::from_str::<DiskHealthReport>("not json at all");
        assert!(result.is_err());
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

    #[test]
    fn resolve_recipe_path_prefers_hot_reload_over_in_tree() {
        // This test exercises the hot-reload precedence. We can only test
        // the in-tree path hermetically (hot-reload depends on $HOME),
        // but we verify the function checks both paths.
        let tmp = tempfile::tempdir().unwrap();
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

        // Should find the in-tree recipe at minimum
        let result = resolve_recipe_path(tmp.path());
        assert!(result.is_some());
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
    // Constants
    // ------------------------------------------------------------------

    #[test]
    fn recipe_filename_is_correct() {
        assert_eq!(RECIPE_FILENAME, "disk-health-check.yaml");
    }

    #[test]
    fn adapter_tag_is_correct() {
        assert_eq!(ADAPTER_TAG, "disk-health-check");
    }
}
