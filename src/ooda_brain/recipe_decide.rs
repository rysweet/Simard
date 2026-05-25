//! Recipe-runner-backed [`OodaDecideBrain`] — delegates the LLM call to
//! `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/ooda-decide.yaml` recipe.
//!
//! This replaces [`super::decide::RustyClawdDecideBrain`] for deployments
//! where recipe-runner-rs is available, aligning with the architectural
//! direction in issue #1971.
//!
//! The shim invokes `recipe-runner-rs` as a subprocess with `-c` context
//! vars and parses its stdout using keyword scanning for action-kind tokens.
//!
//! Fallback on recipe failure: propagates as `SimardError` (callers must
//! fall back to `DeterministicFallbackDecideBrain` or `RustyClawdDecideBrain`).

use std::path::PathBuf;
use std::process::Command;

use crate::error::{SimardError, SimardResult};

use super::decide::{DecideContext, DecideJudgment, OodaDecideBrain};

const ADAPTER_TAG: &str = "recipe-decide-brain";
const RECIPE_FILENAME: &str = "ooda-decide.yaml";

/// The 10 recognised action-kind tokens, ordered so longer/more-specific
/// variants match before shorter substrings (e.g. `poll_developer_activity`
/// before a hypothetical prefix match).
type KeywordEntry = (&'static str, fn(String) -> DecideJudgment);
const ACTION_KEYWORDS: &[KeywordEntry] = &[
    ("advance_goal", |r| DecideJudgment::AdvanceGoal {
        rationale: r,
    }),
    ("consolidate_memory", |r| {
        DecideJudgment::ConsolidateMemory { rationale: r }
    }),
    ("run_improvement", |r| DecideJudgment::RunImprovement {
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
    ("poll_developer_activity", |r| {
        DecideJudgment::PollDeveloperActivity { rationale: r }
    }),
    ("extract_ideas", |r| DecideJudgment::ExtractIdeas {
        rationale: r,
    }),
    ("safe_update", |r| DecideJudgment::SafeUpdate {
        rationale: r,
    }),
];

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

/// Recipe-runner-backed OODA Decide brain.
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
        parse_decide_verdict_from_text(&raw).map_err(|reason| {
            SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason,
            }
        })
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

/// Parse recipe stdout text for action-kind keywords.
///
/// The recipe runs an agent that produces natural language output. We scan
/// the full text (case-insensitive) for any of the 10 recognised action-kind
/// tokens. This replaces the brittle `DECISION:` first-line parser with a
/// more resilient keyword scan that tolerates varied agent output formats.
///
/// Scan rules:
/// - Case-insensitive search across the full response text
/// - First matching keyword wins
/// - The full text (truncated) becomes the rationale
/// - If no keyword found → error
pub fn parse_decide_verdict_from_text(text: &str) -> Result<DecideJudgment, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{ADAPTER_TAG}: recipe returned empty output"));
    }

    let lower = trimmed.to_ascii_lowercase();
    let rationale = truncate(trimmed, 500);

    for &(keyword, ref constructor) in ACTION_KEYWORDS {
        if lower.contains(keyword) {
            return Ok(constructor(rationale));
        }
    }

    Err(format!(
        "{ADAPTER_TAG}: no action-kind keyword found in recipe output; raw={:?}",
        truncate(text, 200)
    ))
}

/// Build a recipe-backed Decide brain if recipe-runner-rs and the recipe
/// file are available. Returns `None` when either is missing so the caller
/// can fall back to `RustyClawdDecideBrain` or `DeterministicFallbackDecideBrain`.
pub fn build_recipe_decide_brain(repo_root: &std::path::Path) -> Option<Box<dyn OodaDecideBrain>> {
    RecipeDecideBrain::new(repo_root).map(|b| Box::new(b) as Box<dyn OodaDecideBrain>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::ActionKind;

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let brain = RecipeDecideBrain::new(std::path::Path::new("/nonexistent"));
        assert!(brain.is_none());
    }

    // ------------------------------------------------------------------
    // Keyword-scan parser tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_advance_goal() {
        let text = "DECISION: advance_goal\nordinary goal slug, default routing";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    #[test]
    fn parse_consolidate_memory() {
        let text = "The action should be consolidate_memory because __memory__ is present.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn parse_run_improvement() {
        let text = "DECISION: run_improvement\nimprovement cycle needed";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::RunImprovement);
    }

    #[test]
    fn parse_research_query() {
        let text = "Based on the reason, research_query is appropriate.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::ResearchQuery);
    }

    #[test]
    fn parse_run_gym_eval() {
        let text = "DECISION: run_gym_eval\nlow score detected";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::RunGymEval);
    }

    #[test]
    fn parse_build_skill() {
        let text = "We should build_skill for this goal.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::BuildSkill);
    }

    #[test]
    fn parse_launch_session() {
        let text = "DECISION: launch_session\nnew session needed";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::LaunchSession);
    }

    #[test]
    fn parse_poll_developer_activity() {
        let text = "The synthetic ID maps to poll_developer_activity.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::PollDeveloperActivity);
    }

    #[test]
    fn parse_extract_ideas() {
        let text = "DECISION: extract_ideas\nmine recent activity";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::ExtractIdeas);
    }

    #[test]
    fn parse_safe_update() {
        let text = "Conditions met for safe_update. Divergence >= 3.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
    }

    #[test]
    fn parse_case_insensitive() {
        let text = "ADVANCE_GOAL — the goal should proceed.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    #[test]
    fn parse_multiline_response() {
        let text = "## Analysis\n\nAfter reviewing the priority:\n\nDECISION: consolidate_memory\n\nThe __memory__ ID requires consolidation.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn parse_empty_is_error() {
        let result = parse_decide_verdict_from_text("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn parse_no_keyword_is_error() {
        let result = parse_decide_verdict_from_text("I think we should do something.");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no action-kind keyword"));
    }

    #[test]
    fn parse_rationale_contains_full_text() {
        let text = "DECISION: advance_goal\nThe goal should proceed because evidence supports it.";
        let j = parse_decide_verdict_from_text(text).unwrap();
        assert!(
            j.rationale().contains("evidence supports"),
            "rationale should include full text: {}",
            j.rationale()
        );
    }

    #[test]
    fn all_action_kinds_parse() {
        let variants = vec![
            ("advance_goal", ActionKind::AdvanceGoal),
            ("run_improvement", ActionKind::RunImprovement),
            ("consolidate_memory", ActionKind::ConsolidateMemory),
            ("research_query", ActionKind::ResearchQuery),
            ("run_gym_eval", ActionKind::RunGymEval),
            ("build_skill", ActionKind::BuildSkill),
            ("launch_session", ActionKind::LaunchSession),
            ("poll_developer_activity", ActionKind::PollDeveloperActivity),
            ("extract_ideas", ActionKind::ExtractIdeas),
            ("safe_update", ActionKind::SafeUpdate),
        ];
        for (keyword, expected_kind) in variants {
            let text = format!("The action is {keyword}. Rationale here.");
            let j = parse_decide_verdict_from_text(&text)
                .unwrap_or_else(|e| panic!("keyword {keyword} failed: {e}"));
            assert_eq!(
                j.action_kind(),
                expected_kind,
                "keyword {keyword} wrong kind"
            );
        }
    }
}
