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
    agent_binary: &'static str,
}

impl RecipeProgressChecker {
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        // Verify recipe-runner-rs is available
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
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
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

        let (decision, matched) = parse_verdict_outcome(&raw);
        crate::recipe_output::record_parse_outcome("progress_checker", matched);
        decision
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

/// Parse recipe stdout text for verdict keywords (accept/reject).
///
/// The recipe runs an agent that produces natural language output.
/// We scan for the verdict keyword and use the surrounding text as
/// the rationale. No JSON parsing needed — the agent already makes
/// the decision; we just need to read it.
///
/// Scan rules:
/// - Case-insensitive search for "accept" or "reject"
/// - First match wins
/// - The full text (truncated) becomes the rationale
/// - If neither keyword found, accept with diagnostic (same fallback
///   as the old JSON parse-error path)
pub fn parse_verdict_from_text(text: &str) -> EvidenceDecision {
    parse_verdict_outcome(text).0
}

/// Like [`parse_verdict_from_text`] but also returns whether a verdict keyword
/// was actually found (`true`) versus the permissive accept-to-avoid-blocking
/// default firing (`false`). The flag drives the `recipe_parse_*_total{phase}`
/// counter at the subprocess call site (issue #2484); the pure parser stays
/// metric-free so unit tests write no metrics.
pub fn parse_verdict_outcome(text: &str) -> (EvidenceDecision, bool) {
    // Strip ANSI escapes + drop whole tracing-log / runner-banner lines first
    // (shared #2484 extractor) so a noise-obscured verdict is not silently
    // dropped — e.g. a real "reject" must not be missed because it trailed an
    // ANSI-coloured log prefix, and an "already"/"accepted" substring inside a
    // dropped log line cannot fabricate a verdict.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);
    let lower = cleaned.to_ascii_lowercase();
    let rationale = truncate(cleaned.trim(), RATIONALE_MAX_CHARS);

    if lower.contains("reject") {
        (
            EvidenceDecision::Reject {
                reason: format!("{ADAPTER_TAG}: reject — {rationale}"),
            },
            true,
        )
    } else if lower.contains("accept") {
        (
            EvidenceDecision::Accept {
                reason: format!("{ADAPTER_TAG}: accept — {rationale}"),
            },
            true,
        )
    } else {
        (
            EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: no verdict keyword found in recipe output; accepting to avoid blocking goal"
                ),
            },
            false,
        )
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
            parent_goal_id: None,
            repo: None,
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
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
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
            agent_binary: "copilot",
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
            agent_binary: "copilot",
        };
        let g = goal_with_activity(Some("working on it"));
        match checker.check(&g, 10, 20, Utc::now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(
                    reason.contains("recipe") || reason.contains("spawn"),
                    "got: {reason}"
                );
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on infra failure"),
        }
    }

    // ------------------------------------------------------------------
    // Text-based verdict parser (issue #1980)
    // ------------------------------------------------------------------

    #[test]
    fn text_verdict_accept_detected() {
        let text = "After reviewing the evidence, I accept the claimed progress.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_reject_detected() {
        let text = "The progress jump is not supported. I reject the claim.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => panic!("expected reject"),
        }
    }

    #[test]
    fn text_verdict_case_insensitive() {
        let text = "ACCEPT - the evidence checks out";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_reject_takes_priority_over_accept() {
        // If both keywords appear, reject wins (safer default)
        let text = "I cannot accept this, I must reject the claim.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => panic!("expected reject when both keywords present"),
        }
    }

    #[test]
    fn text_verdict_no_keyword_falls_back_to_accept() {
        let text = "The progress looks reasonable for this stage.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("no verdict keyword"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => {
                panic!("expected accept fallback when no keyword found")
            }
        }
    }

    #[test]
    fn text_verdict_empty_falls_back_to_accept() {
        let text = "";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("no verdict keyword"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on empty text"),
        }
    }

    #[test]
    fn text_verdict_includes_rationale_from_text() {
        let text = "Based on the PR and commit history, I accept this progress claim.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(
                    reason.contains("PR and commit"),
                    "rationale should include text: {reason}"
                );
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_multiline_response() {
        let text = "Looking at the evidence:\n\n- PR #2018 has 3 commits\n- Tests pass\n\nI accept the claimed progress from 30% to 45%.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_recovers_reject_past_ansi_log_prefix() {
        // #2484: the verdict trails an ANSI-coloured tracing-log line. The
        // shared extractor strips both so the real reject is recovered and the
        // rationale carries no log/ANSI noise (a noise-obscured reject must
        // never be silently accepted-to-avoid-blocking).
        let esc = '\u{1b}';
        let text = format!(
            "{esc}[2m2026-06-28T08:08:58.151133Z{esc}[0m  INFO checker: scoring\n\
             Verdict: reject — no measurable progress this cycle."
        );
        match parse_verdict_from_text(&text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
                assert!(!reason.contains("INFO checker"), "log line must be dropped");
            }
            EvidenceDecision::Accept { .. } => panic!("noise-obscured reject must be recovered"),
        }
    }

    #[test]
    fn parse_verdict_outcome_reports_match_flag_for_counter() {
        // The `bool` drives the `recipe_parse_*_total{progress_checker}` counter:
        // a real keyword ⇒ true (success), the permissive default ⇒ false.
        let (decision, matched) = parse_verdict_outcome("just prose, no verdict keyword here");
        assert!(matches!(decision, EvidenceDecision::Accept { .. }));
        assert!(!matched, "permissive default must report matched=false");

        let (_, matched_reject) = parse_verdict_outcome("Verdict: reject — insufficient evidence");
        assert!(matched_reject, "a real verdict must report matched=true");
    }
}

/// Issue #2570: `is_copilot_launcher_line` / `strip_recipe_noise` is a shared
/// chokepoint. The distillation fact-yield fix exempts JSON structural-token
/// lines (`{`, `"`, `[`) from launcher-noise classification; this
/// progress-checker consumer must keep stripping the REAL launcher preamble
/// (which never begins with a JSON token) so a noise-obscured verdict is still
/// read. This is the progress-checker slice of the cross-consumer regression
/// coverage.
#[cfg(test)]
mod issue_2570_cross_consumer_tests {
    use super::*;

    fn launcher_preamble() -> String {
        "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: cfg\n\
         INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
         Run 'copilot update' to check for updates.\n"
            .to_string()
    }

    #[test]
    fn progress_checker_still_strips_real_launcher_preamble() {
        let raw = format!(
            "{}reject — no measurable progress this cycle",
            launcher_preamble()
        );
        let (decision, matched) = parse_verdict_outcome(&raw);
        assert!(
            matched,
            "the real reject must be read, not silently defaulted"
        );
        match decision {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
                assert!(
                    !reason.contains("launching copilot"),
                    "launcher preamble must be dropped from the rationale: {reason}"
                );
            }
            EvidenceDecision::Accept { .. } => {
                panic!("noise-obscured reject must be recovered after the #2570 guard")
            }
        }
    }

    #[test]
    fn shared_cleaner_preserves_pretty_fact_content_line_quoting_launcher_substring() {
        let content_line =
            "\"content\": \"the agent logged launching copilot binary=/x before answering\"";
        let cleaned = crate::recipe_output::strip_recipe_noise(content_line);
        assert_eq!(
            cleaned.as_ref(),
            content_line,
            "a JSON payload line must survive the shared cleaner"
        );
    }
}
