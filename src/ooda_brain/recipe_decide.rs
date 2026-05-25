//! Recipe-runner-backed [`OodaDecideBrain`] — delegates the LLM call to
//! `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/ooda-decide.yaml` recipe.
//!
//! This replaces [`super::decide::RustyClawdDecideBrain`] for deployments
//! where recipe-runner-rs is available, aligning with the architectural
//! direction in issue #1971: Simard should use the amplihack recipe-runner
//! as a design component rather than hand-coding Rust structs that wrap
//! LLM calls.
//!
//! The shim invokes `recipe-runner-rs` as a subprocess with `-c` context
//! vars, then scans stdout for any of the 10 known action keywords using
//! case-insensitive `contains()`. No structured output format is required —
//! the agent already returns the right answer in its prose; we just read it.
//!
//! Fallback on no keyword match: returns `advance_goal` as default (same as
//! the existing deterministic fallback for unrecognized goals).

use std::path::PathBuf;
use std::process::Command;

use super::decide::{DecideContext, DecideJudgment, OodaDecideBrain};
use crate::error::{SimardError, SimardResult};

const ADAPTER_TAG: &str = "recipe-decide-brain";
const RECIPE_FILENAME: &str = "ooda-decide.yaml";

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

/// Recipe-runner-backed decide brain.
pub struct RecipeDecideBrain {
    recipe_path: PathBuf,
}

impl RecipeDecideBrain {
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

impl OodaDecideBrain for RecipeDecideBrain {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("-c")
            .arg(format!("goal_id={}", ctx.goal_id))
            .arg("-c")
            .arg(format!("urgency={:.3}", ctx.urgency))
            .arg("-c")
            .arg(format!("reason={}", ctx.reason))
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
        Ok(parse_action_from_text(&raw))
    }
}

/// Parse recipe stdout text for action-kind keywords.
///
/// The recipe runs an agent that produces natural language output containing
/// one of the 10 known action keywords. We scan case-insensitively for each
/// keyword and return the first match. If no keyword is found, we default to
/// `advance_goal` (same as the deterministic fallback for unrecognized goals).
///
/// The 10 keywords are checked in a fixed order. No keyword is a substring of
/// another, so scan order only matters for the (unlikely) case where the agent
/// mentions multiple keywords in its prose.
pub fn parse_action_from_text(text: &str) -> DecideJudgment {
    let lower = text.to_ascii_lowercase();
    let rationale = truncate(text.trim(), 500);

    // Check keywords. Order: more-specific / less-common first, but since no
    // keyword is a substring of another, order only matters when multiple
    // keywords appear in the same text.
    type JudgmentCtor = fn(String) -> DecideJudgment;
    let pairs: &[(&str, JudgmentCtor)] = &[
        ("poll_developer_activity", |r| {
            DecideJudgment::PollDeveloperActivity { rationale: r }
        }),
        ("consolidate_memory", |r| {
            DecideJudgment::ConsolidateMemory { rationale: r }
        }),
        ("run_improvement", |r| DecideJudgment::RunImprovement {
            rationale: r,
        }),
        ("extract_ideas", |r| DecideJudgment::ExtractIdeas {
            rationale: r,
        }),
        ("safe_update", |r| DecideJudgment::SafeUpdate {
            rationale: r,
        }),
        ("research_query", |r| DecideJudgment::ResearchQuery {
            rationale: r,
        }),
        ("run_gym_eval", |r| DecideJudgment::RunGymEval {
            rationale: r,
        }),
        ("build_skill", |r| DecideJudgment::BuildSkill {
            rationale: r,
        }),
        ("launch_session", |r| DecideJudgment::LaunchSession {
            rationale: r,
        }),
        ("advance_goal", |r| DecideJudgment::AdvanceGoal {
            rationale: r,
        }),
    ];

    for (keyword, constructor) in pairs {
        if lower.contains(keyword) {
            return constructor(rationale);
        }
    }

    // Default: advance_goal (same as deterministic fallback for unrecognized goals)
    DecideJudgment::AdvanceGoal {
        rationale: format!(
            "{ADAPTER_TAG}: no action keyword found in recipe output; defaulting to advance_goal"
        ),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::ActionKind;

    // ===================================================================
    // parse_action_from_text — keyword scanner
    // ===================================================================

    // --- All 10 keywords are recognized ---------------------------------

    #[test]
    fn keyword_advance_goal() {
        let j = parse_action_from_text("The best action is advance_goal here.");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    #[test]
    fn keyword_consolidate_memory() {
        let j = parse_action_from_text("We should consolidate_memory now.");
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn keyword_run_improvement() {
        let j = parse_action_from_text("I recommend run_improvement for this goal.");
        assert_eq!(j.action_kind(), ActionKind::RunImprovement);
    }

    #[test]
    fn keyword_poll_developer_activity() {
        let j = parse_action_from_text("poll_developer_activity is warranted.");
        assert_eq!(j.action_kind(), ActionKind::PollDeveloperActivity);
    }

    #[test]
    fn keyword_extract_ideas() {
        let j = parse_action_from_text("Let's extract_ideas from the codebase.");
        assert_eq!(j.action_kind(), ActionKind::ExtractIdeas);
    }

    #[test]
    fn keyword_safe_update() {
        let j = parse_action_from_text("Conditions met for safe_update.");
        assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
    }

    #[test]
    fn keyword_research_query() {
        let j = parse_action_from_text("A research_query is the right call.");
        assert_eq!(j.action_kind(), ActionKind::ResearchQuery);
    }

    #[test]
    fn keyword_run_gym_eval() {
        let j = parse_action_from_text("Low scores warrant run_gym_eval.");
        assert_eq!(j.action_kind(), ActionKind::RunGymEval);
    }

    #[test]
    fn keyword_build_skill() {
        let j = parse_action_from_text("The agent needs to build_skill first.");
        assert_eq!(j.action_kind(), ActionKind::BuildSkill);
    }

    #[test]
    fn keyword_launch_session() {
        let j = parse_action_from_text("Time to launch_session for this task.");
        assert_eq!(j.action_kind(), ActionKind::LaunchSession);
    }

    // --- Exhaustive check: all 10 keywords map to correct ActionKind ----

    #[test]
    fn all_ten_keywords_map_to_correct_action_kind() {
        let cases = vec![
            ("advance_goal", ActionKind::AdvanceGoal),
            ("consolidate_memory", ActionKind::ConsolidateMemory),
            ("run_improvement", ActionKind::RunImprovement),
            ("poll_developer_activity", ActionKind::PollDeveloperActivity),
            ("extract_ideas", ActionKind::ExtractIdeas),
            ("safe_update", ActionKind::SafeUpdate),
            ("research_query", ActionKind::ResearchQuery),
            ("run_gym_eval", ActionKind::RunGymEval),
            ("build_skill", ActionKind::BuildSkill),
            ("launch_session", ActionKind::LaunchSession),
        ];
        for (keyword, expected) in cases {
            let text = format!("After analysis, my decision is {keyword}.");
            let j = parse_action_from_text(&text);
            assert_eq!(
                j.action_kind(),
                expected,
                "keyword '{keyword}' should map to {expected:?}"
            );
        }
    }

    // --- Case insensitivity ---------------------------------------------

    #[test]
    fn keyword_case_insensitive_upper() {
        let j = parse_action_from_text("CONSOLIDATE_MEMORY is needed.");
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn keyword_case_insensitive_mixed() {
        let j = parse_action_from_text("I suggest Run_Improvement.");
        assert_eq!(j.action_kind(), ActionKind::RunImprovement);
    }

    // --- Default fallback to advance_goal when no keyword found ---------

    #[test]
    fn no_keyword_defaults_to_advance_goal() {
        let j = parse_action_from_text("I think the goal should proceed normally.");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        assert!(
            j.rationale().contains("no action keyword"),
            "rationale should explain default: {}",
            j.rationale()
        );
    }

    #[test]
    fn empty_text_defaults_to_advance_goal() {
        let j = parse_action_from_text("");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        assert!(j.rationale().contains("no action keyword"));
    }

    #[test]
    fn whitespace_only_defaults_to_advance_goal() {
        let j = parse_action_from_text("   \n\t  ");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    // --- Keyword embedded in prose (not on its own line) -----------------

    #[test]
    fn keyword_embedded_in_multiline_prose() {
        let text = "Looking at the current state:\n\n\
                    - Goal urgency is 0.85\n\
                    - Memory is fragmented\n\n\
                    My recommendation: consolidate_memory to reduce\n\
                    context overhead before the next sprint.";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn keyword_at_end_of_prose() {
        let text = "After careful consideration, the action should be safe_update";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
    }

    #[test]
    fn keyword_at_start_of_text() {
        let text = "advance_goal — this is a straightforward code change.";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    // --- Multiple keywords: first match in scan order wins --------------

    #[test]
    fn multiple_keywords_first_in_scan_order_wins() {
        // poll_developer_activity is checked before advance_goal in scan order
        let text = "We could advance_goal or poll_developer_activity.";
        let j = parse_action_from_text(text);
        assert_eq!(
            j.action_kind(),
            ActionKind::PollDeveloperActivity,
            "poll_developer_activity is scanned before advance_goal"
        );
    }

    // --- Rationale includes the agent's text ----------------------------

    #[test]
    fn rationale_contains_agent_text() {
        let text = "The goal has stalled. I recommend run_improvement to unblock.";
        let j = parse_action_from_text(text);
        assert!(
            j.rationale().contains("stalled"),
            "rationale should include agent text: {}",
            j.rationale()
        );
    }

    #[test]
    fn rationale_truncated_for_long_text() {
        let long_text = format!("consolidate_memory because {}", "x".repeat(1000));
        let j = parse_action_from_text(&long_text);
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        // Rationale should be truncated (max 500 chars + "…" which is 3 bytes)
        assert!(
            j.rationale().chars().count() <= 501,
            "rationale should be truncated to ≤501 chars: got {} chars",
            j.rationale().chars().count()
        );
    }

    // --- No keyword substring collisions --------------------------------

    #[test]
    fn no_keyword_is_substring_of_another() {
        // Verify the design assumption: no keyword is a substring of another.
        // If this test fails, the scan order becomes safety-critical and needs
        // a more sophisticated matching strategy (e.g., longest match first).
        let keywords = [
            "advance_goal",
            "consolidate_memory",
            "run_improvement",
            "poll_developer_activity",
            "extract_ideas",
            "safe_update",
            "research_query",
            "run_gym_eval",
            "build_skill",
            "launch_session",
        ];
        for (i, a) in keywords.iter().enumerate() {
            for (j, b) in keywords.iter().enumerate() {
                if i != j {
                    assert!(
                        !a.contains(b),
                        "keyword '{a}' contains '{b}' — this violates the \
                         no-substring-overlap invariant"
                    );
                }
            }
        }
    }

    // --- RecipeDecideBrain constructor -----------------------------------

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let brain = RecipeDecideBrain::new(std::path::Path::new("/nonexistent"));
        assert!(brain.is_none());
    }

    // --- RecipeDecideBrain::judge_decision with missing binary -----------

    #[test]
    fn judge_decision_with_missing_binary_returns_error() {
        // Construct with a fake path to bypass the recipe file check
        let brain = RecipeDecideBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
        };
        let ctx = DecideContext {
            goal_id: "test-goal".to_string(),
            urgency: 0.7,
            reason: "test reason".to_string(),
        };
        let err = brain.judge_decision(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains(ADAPTER_TAG),
            "error should identify the adapter: {msg}"
        );
    }

    // --- Realistic LLM output patterns ----------------------------------

    #[test]
    fn realistic_llm_output_advance_goal_prose() {
        let text = "Based on the priority analysis:\n\n\
                    Goal: ship-v1\n\
                    Urgency: 0.850\n\n\
                    This is a standard development goal with active progress. \
                    The appropriate action is to advance_goal and continue the \
                    current engineering work.";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    #[test]
    fn realistic_llm_output_consolidate_memory_verbose() {
        let text = "## Decision Analysis\n\n\
                    The goal_id `__memory__` indicates this is a synthetic \
                    memory consolidation priority. The urgency of 0.600 is \
                    moderate, suggesting the memory store has grown but is \
                    not yet critical.\n\n\
                    **Action: consolidate_memory**\n\n\
                    Memory compaction will reduce context overhead.";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn realistic_llm_output_with_markdown_formatting() {
        let text = "# OODA Decide\n\n\
                    | Factor | Value |\n\
                    |--------|-------|\n\
                    | Goal   | __improvement__ |\n\
                    | Urgency | 0.500 |\n\n\
                    Given the synthetic priority indicator, I recommend \
                    `run_improvement` to address code quality.";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::RunImprovement);
    }

    #[test]
    fn realistic_llm_output_keyword_in_backticks() {
        let text = "The decision is `safe_update` since the binary is 5 commits behind.";
        let j = parse_action_from_text(text);
        assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
    }

    #[test]
    fn realistic_llm_output_no_keyword_just_prose() {
        // Agent rambles without using any action keyword
        let text = "The goal appears to be making steady progress. The engineer \
                    is actively working on it and the last commit was 2 hours ago. \
                    I think we should continue as planned.";
        let j = parse_action_from_text(text);
        assert_eq!(
            j.action_kind(),
            ActionKind::AdvanceGoal,
            "no keyword should default to advance_goal"
        );
    }
}
