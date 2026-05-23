//! Recipe-runner-backed [`ProgressEvidenceChecker`] — delegates the LLM
//! call to `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/progress-assessment.yaml` recipe.
//!
//! This replaces [`super::progress_reviewer::LlmReviewerProgressChecker`]
//! for deployments where recipe-runner-rs is available, aligning with the
//! architectural direction in issue #1971: Simard should use the amplihack
//! recipe-runner as a design component rather than hand-coding Rust structs
//! that wrap LLM calls.
//!
//! The downward/no-change auto-accept fast path is preserved identically.
//! For upward claims the shim invokes `recipe-runner-rs` as a subprocess
//! with `-c` context vars and parses its stdout using the same
//! `parse_reviewer_response` logic from `progress_reviewer`.
//!
//! Fallback on recipe failure: accept with diagnostic (matches the
//! existing `LlmReviewerProgressChecker` behaviour so goals are never
//! blocked on infrastructure issues).

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::process::Command;

use super::progress_evidence::{EvidenceDecision, ProgressEvidenceChecker};
use super::types::ActiveGoal;

const ADAPTER_TAG: &str = "recipe-progress-checker";
const RECIPE_FILENAME: &str = "progress-assessment.yaml";

/// Max chars retained from the rationale before truncation.
const RATIONALE_MAX_CHARS: usize = 240;

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
fn resolve_recipe_path(repo_root: &std::path::Path) -> Option<PathBuf> {
    // Hot-reload path
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    // In-tree fallback
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Recipe-runner-backed progress evidence checker.
pub struct RecipeProgressChecker {
    recipe_path: PathBuf,
}

impl RecipeProgressChecker {
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        // Verify recipe-runner-rs is available
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

impl ProgressEvidenceChecker for RecipeProgressChecker {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        _since: DateTime<Utc>,
    ) -> EvidenceDecision {
        // Downward/no-change is always accepted (no recipe call needed).
        if new_percent <= old_percent {
            return EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: downward / no-change ({old_percent} -> {new_percent}) auto-accepted"
                ),
            };
        }

        let plan = goal
            .current_activity
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let wip_summary = render_wip_summary(goal);

        let result = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("-c")
            .arg(format!("goal_id={}", goal.id))
            .arg("-c")
            .arg(format!("problem={}", goal.description))
            .arg("-c")
            .arg(format!("plan={plan}"))
            .arg("-c")
            .arg(format!("prior_pct={old_percent}"))
            .arg("-c")
            .arg(format!("claimed_pct={new_percent}"))
            .arg("-c")
            .arg(format!("wip_summary={wip_summary}"))
            .output();

        let output = match result {
            Ok(o) => o,
            Err(e) => {
                return EvidenceDecision::Accept {
                    reason: format!(
                        "{ADAPTER_TAG}: recipe-runner-rs spawn failed ({e}); accepting to avoid blocking goal"
                    ),
                };
            }
        };

        let raw = String::from_utf8_lossy(&output.stdout).to_string();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: recipe exited with {}; accepting to avoid blocking goal. stderr: {}",
                    output.status,
                    truncate(&stderr, 200)
                ),
            };
        }

        // Reuse the same parse logic from progress_reviewer
        match super::progress_reviewer::parse_reviewer_response(&raw) {
            Ok(parsed) => decision_from_response(parsed),
            Err(parse_err) => EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: parse error ({parse_err}); accepting to avoid blocking goal"
                ),
            },
        }
    }
}

fn render_wip_summary(goal: &ActiveGoal) -> String {
    if goal.wip_refs.is_empty() {
        return String::new();
    }
    use std::fmt::Write;
    let mut s = String::new();
    for (i, w) in goal.wip_refs.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{:?}", w);
    }
    s
}

/// Reuse the same ReviewerResponse → EvidenceDecision mapping.
fn decision_from_response(r: super::progress_reviewer::ReviewerResponse) -> EvidenceDecision {
    let trimmed = r.rationale.trim();
    let rationale = {
        let mut chars = trimmed.chars();
        let prefix: String = chars.by_ref().take(RATIONALE_MAX_CHARS).collect();
        if chars.next().is_some() {
            prefix + "…"
        } else {
            prefix
        }
    };
    let verdict_lc = r.verdict.trim().to_ascii_lowercase();
    if verdict_lc == "accept" {
        EvidenceDecision::Accept {
            reason: format!("{ADAPTER_TAG}: accept — {rationale}"),
        }
    } else if verdict_lc == "reject" {
        EvidenceDecision::Reject {
            reason: format!("{ADAPTER_TAG}: reject — {rationale}"),
        }
    } else {
        EvidenceDecision::Accept {
            reason: format!(
                "{ADAPTER_TAG}: unknown verdict {:?}; accepting to avoid blocking goal",
                r.verdict
            ),
        }
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
    use crate::goal_curation::types::GoalProgress;

    fn goal_with_activity(activity: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            id: "test-goal".to_string(),
            description: "do the thing".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 10 },
            assigned_to: None,
            current_activity: activity.map(String::from),
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    #[test]
    fn downward_move_is_auto_accepted_without_recipe_call() {
        // RecipeProgressChecker::new may fail in test env (no binary) so
        // test the trait method directly via a constructed instance.
        // We only need to test the fast path which doesn't invoke the binary.
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
        };
        let g = goal_with_activity(None);
        match checker.check(&g, 80, 50, Utc::now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("downward"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn no_change_is_auto_accepted() {
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
        };
        let g = goal_with_activity(None);
        assert!(matches!(
            checker.check(&g, 60, 60, Utc::now()),
            EvidenceDecision::Accept { .. }
        ));
    }

    #[test]
    fn upward_claim_with_missing_binary_falls_back_to_accept() {
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
        };
        let g = goal_with_activity(Some("working on it"));
        match checker.check(&g, 10, 20, Utc::now()) {
            EvidenceDecision::Accept { reason } => {
                // Should fail at spawn or recipe exit, either way accept
                assert!(
                    reason.contains("recipe") || reason.contains("spawn"),
                    "got: {reason}"
                );
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on infra failure"),
        }
    }
}
