//! Invocation of the analysis-only `disk-reclaim.yaml` recipe, with a strict
//! marker parse and a **no-fallback** error path.
//!
//! The recipe is a single analysis-only agent step: it inspects `df` / `git
//! worktree list` / `gh pr` / `/proc` / `du`, reasons about reclaimable
//! candidates largest-first, and **emits** them as text markers — then stops.
//! It never deletes. Any recipe or parse failure yields
//! [`SimardError::AdapterInvocationFailed`] and propagates to the caller; there
//! is no silent fallback (garbage in ⇒ nothing deleted).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;

use super::candidate::{ReclaimCandidate, parse_candidates};

/// Adapter tag used in [`SimardError::AdapterInvocationFailed`].
const ADAPTER_TAG: &str = "disk-reclaim";

/// Recipe file name resolved via the hot-reload / in-tree precedence.
const RECIPE_FILENAME: &str = "disk-reclaim.yaml";

/// JSON envelope returned by `recipe-runner-rs --output-format json`. The
/// analysis-only reclaim proposal flow still scrapes candidate JSON out of the
/// first step's `output`, so this envelope stays here (relocated from
/// `disk_health.rs`, which no longer parses recipe output — issue #4722).
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RecipeOutput {
    pub success: bool,
    pub step_results: Vec<StepResult>,
}

/// A single step's result inside the [`RecipeOutput`] envelope.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct StepResult {
    #[allow(dead_code)] // Part of the JSON contract; used in tests.
    pub step_id: String,
    pub output: String,
}

/// Seam that produces the raw first-step output text of the analysis recipe.
/// Production shells `recipe-runner-rs`; tests substitute a deterministic
/// double so the no-fallback contract is verified hermetically.
pub trait RecipeInvoker {
    /// Return the first step's output text, or `Err` describing any failure.
    fn invoke(&self) -> Result<String, String>;
}

/// Production invoker: runs `recipe-runner-rs <recipe> --output-format json`,
/// deserializes the [`RecipeOutput`] envelope, and returns
/// `step_results[0].output`. Mirrors the disk-health resolver precedence.
pub struct RecipeRunnerInvoker {
    pub repo_root: PathBuf,
    pub state_root: PathBuf,
    pub home_override: Option<PathBuf>,
}

impl RecipeInvoker for RecipeRunnerInvoker {
    fn invoke(&self) -> Result<String, String> {
        let recipe_path = resolve_recipe_path(&self.repo_root, self.home_override.as_deref())
            .ok_or_else(|| {
                format!("recipe file {RECIPE_FILENAME} not found in hot-reload or in-tree paths")
            })?;

        let agent_binary = RuntimeConfig::load()
            .map_err(|e| format!("runtime config load failed: {e}"))?
            .llm_provider
            .agent_binary_value();

        let output = Command::new("recipe-runner-rs")
            .arg(recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .arg("-c")
            .arg(format!("state_root={}", self.state_root.display()))
            .arg("-c")
            .arg(format!("repo_path={}", self.repo_root.display()))
            .output()
            .map_err(|e| format!("recipe-runner-rs spawn failed: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "recipe exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let envelope: RecipeOutput = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("failed to deserialize recipe JSON output: {e}"))?;
        if !envelope.success {
            return Err("recipe reported success=false in JSON output".to_string());
        }
        envelope
            .step_results
            .first()
            .map(|s| s.output.clone())
            .ok_or_else(|| "no step results in recipe JSON output".to_string())
    }
}

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
pub fn resolve_recipe_path(repo_root: &Path, home_override: Option<&Path>) -> Option<PathBuf> {
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

/// Invoke the analysis recipe and parse its candidate proposal. **No fallback:**
/// any invocation or parse failure becomes [`SimardError::AdapterInvocationFailed`].
pub fn run_reclaim_recipe(
    invoker: &dyn RecipeInvoker,
) -> SimardResult<(Vec<ReclaimCandidate>, u8)> {
    let output = invoker
        .invoke()
        .map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })?;
    parse_candidates(&output).map_err(|reason| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::super::candidate::CandidateKind;
    use super::*;

    struct FixedInvoker(Result<String, String>);
    impl RecipeInvoker for FixedInvoker {
        fn invoke(&self) -> Result<String, String> {
            self.0.clone()
        }
    }

    #[test]
    fn happy_path_parses_candidates() {
        let out = "\
DISK_USED_PCT=88\n\
CANDIDATES_JSON=[{\"path\":\"/w\",\"kind\":\"tracked_worktree\"}]\n";
        let invoker = FixedInvoker(Ok(out.to_string()));
        let (cands, pct) = run_reclaim_recipe(&invoker).expect("ok");
        assert_eq!(pct, 88);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].kind, CandidateKind::TrackedWorktree);
    }

    #[test]
    fn invocation_failure_is_adapter_invocation_failed_no_fallback() {
        let invoker = FixedInvoker(Err("recipe-runner-rs spawn failed".to_string()));
        let err = run_reclaim_recipe(&invoker).expect_err("must propagate");
        match err {
            SimardError::AdapterInvocationFailed { base_type, .. } => {
                assert_eq!(base_type, ADAPTER_TAG);
            }
            other => panic!("expected AdapterInvocationFailed, got {other:?}"),
        }
    }

    #[test]
    fn malformed_output_is_adapter_invocation_failed_no_fallback() {
        // A malformed candidate ARRAY hard-errors; there is no fallback to
        // "reclaim nothing quietly" — it surfaces as an adapter failure.
        let out = "DISK_USED_PCT=90\nCANDIDATES_JSON=not-json\n";
        let invoker = FixedInvoker(Ok(out.to_string()));
        let err = run_reclaim_recipe(&invoker).expect_err("must propagate parse failure");
        assert!(matches!(err, SimardError::AdapterInvocationFailed { .. }));
    }

    #[test]
    fn missing_markers_is_adapter_invocation_failed() {
        let invoker = FixedInvoker(Ok("no markers here".to_string()));
        assert!(matches!(
            run_reclaim_recipe(&invoker),
            Err(SimardError::AdapterInvocationFailed { .. })
        ));
    }
}
