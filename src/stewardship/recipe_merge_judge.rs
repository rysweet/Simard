//! Recipe-runner-backed [`MergeJudge`] — delegates the LLM call to
//! `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/merge-readiness-judge.yaml` recipe.
//!
//! This replaces [`super::merge_judge::LlmMergeJudge`] for deployments
//! where recipe-runner-rs is available, aligning with the architectural
//! direction in issue #1971.
//!
//! The shim invokes `recipe-runner-rs` as a subprocess with `-c` context
//! vars and parses its stdout using the same `parse_judge_response` logic
//! from `merge_judge`.
//!
//! Fallback on recipe failure: propagates as `SimardError` (same as
//! `LlmMergeJudge` — the merge authority handles the error).

use std::path::PathBuf;
use std::process::Command;

use crate::error::{SimardError, SimardResult};

use super::merge_authority::PrSnapshot;
use super::merge_judge::{JudgeOutcome, MergeJudge, MergeJudgeKind, parse_judge_response};

const ADAPTER_TAG: &str = "recipe-merge-judge";
const RECIPE_FILENAME: &str = "merge-readiness-judge.yaml";

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
fn resolve_recipe_path(repo_root: &std::path::Path) -> Option<PathBuf> {
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

/// Recipe-runner-backed merge-readiness judge.
pub struct RecipeMergeJudge {
    recipe_path: PathBuf,
}

impl RecipeMergeJudge {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self { recipe_path })
    }
}

impl MergeJudge for RecipeMergeJudge {
    fn judge(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome> {
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("-c")
            .arg(format!("pr_number={pr_number}"))
            .arg("-c")
            .arg(format!("repo={repo}"))
            .arg("-c")
            .arg(format!("pr_body={}", snapshot.body))
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

        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        parse_judge_response(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })
    }

    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Recipe
    }
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

    #[test]
    fn new_returns_none_when_recipe_missing() {
        // Non-existent dir — no recipe file can be found
        let judge = RecipeMergeJudge::new(std::path::Path::new("/nonexistent"));
        assert!(judge.is_none());
    }

    #[test]
    fn kind_returns_recipe() {
        let judge = RecipeMergeJudge {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
        };
        assert_eq!(judge.kind(), MergeJudgeKind::Recipe);
        assert!(judge.kind().is_configured());
    }
}
