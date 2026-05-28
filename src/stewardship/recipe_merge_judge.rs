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
use super::merge_judge::{JudgeOutcome, MergeJudge, MergeJudgeKind, Verdict};

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
    agent_binary: &'static str,
}

impl RecipeMergeJudge {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
        })
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
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
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
        parse_merge_verdict_from_text(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
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

/// Parse recipe stdout text for merge-readiness verdict keywords.
///
/// The recipe runs an agent that produces natural language output.
/// We scan for verdict keywords and use the surrounding text as rationale.
/// No JSON parsing needed — the agent already makes the decision.
///
/// Scan rules (case-insensitive):
/// - "not_ready" or "not ready" → NotReady
/// - "unclear" → Unclear
/// - "ready" (without "not" prefix) → Ready
/// - None found → error
pub fn parse_merge_verdict_from_text(text: &str) -> Result<JudgeOutcome, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{ADAPTER_TAG}: recipe returned empty output"));
    }

    let lower = trimmed.to_ascii_lowercase();
    let rationale = truncate(trimmed, 500);

    if lower.contains("not_ready") || lower.contains("not ready") {
        Ok(JudgeOutcome {
            verdict: Verdict::NotReady,
            rationale,
            blockers: vec![],
        })
    } else if lower.contains("unclear") {
        Ok(JudgeOutcome {
            verdict: Verdict::Unclear,
            rationale,
            blockers: vec![],
        })
    } else if lower.contains("ready") {
        Ok(JudgeOutcome {
            verdict: Verdict::Ready,
            rationale,
            blockers: vec![],
        })
    } else {
        Err(format!(
            "{ADAPTER_TAG}: no verdict keyword (ready/not_ready/unclear) found in recipe output; raw={:?}",
            truncate(text, 200)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let judge = RecipeMergeJudge::new(std::path::Path::new("/nonexistent"));
        assert!(judge.is_none());
    }

    #[test]
    fn kind_returns_recipe() {
        let judge = RecipeMergeJudge {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        assert_eq!(judge.kind(), MergeJudgeKind::Recipe);
        assert!(judge.kind().is_configured());
    }

    // ------------------------------------------------------------------
    // Text-based merge verdict parser (issue #1980)
    // ------------------------------------------------------------------

    #[test]
    fn text_verdict_ready() {
        let text = "After reviewing the PR body, I find it ready for merge. All six sections are present and substantive.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
        assert!(out.rationale.contains("ready"));
    }

    #[test]
    fn text_verdict_not_ready() {
        let text = "The PR is not_ready because the Quality-audit section is missing.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    #[test]
    fn text_verdict_not_ready_with_space() {
        let text = "This PR is not ready — the test plan section is empty.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    #[test]
    fn text_verdict_unclear() {
        let text = "The PR body appears truncated. My verdict is unclear.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Unclear);
    }

    #[test]
    fn text_verdict_case_insensitive() {
        let text = "READY - all criteria met";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
    }

    #[test]
    fn text_verdict_not_ready_wins_over_ready() {
        // "not_ready" contains "ready" but should match not_ready first
        let text = "The PR is not_ready due to missing sections.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    #[test]
    fn text_verdict_empty_is_error() {
        let result = parse_merge_verdict_from_text("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn text_verdict_no_keyword_is_error() {
        let result = parse_merge_verdict_from_text("The PR looks interesting but I cannot decide.");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no verdict keyword"));
    }

    #[test]
    fn text_verdict_multiline_response() {
        let text = "## Merge Readiness Assessment\n\nAfter reviewing all sections:\n\n- Problem statement: ✓\n- Solution: ✓\n- Test plan: ✓\n\nVerdict: ready\n";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
    }

    #[test]
    fn text_verdict_rationale_is_full_text() {
        let text = "Comprehensive analysis shows this PR is ready.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert!(
            out.rationale.contains("Comprehensive"),
            "rationale should include full text: {}",
            out.rationale
        );
    }
}
