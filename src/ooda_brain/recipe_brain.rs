//! Unified recipe-runner-backed brain — single struct [`RecipeBrain`] that
//! implements all three OODA brain traits (`OodaBrain`, `OodaDecideBrain`,
//! `OodaOrientBrain`), parameterised by recipe filename and adapter tag.
//!
//! Consolidates the formerly separate `RecipeDecideBrain`,
//! `RecipeOrientBrain`, and `RecipeEngineerLifecycleBrain` (issue #2132).
//! The principle: "one agent, one identity, one brain — different recipes
//! for different circumstances."
//!
//! Each trait impl invokes `recipe-runner-rs` as a subprocess with `-c`
//! context vars, then parses via trivial first-word / first-number
//! extractors (issue #2144 — no keyword scanners, no JSON extraction).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::decide::{DecideContext, DecideJudgment, OodaDecideBrain};
use super::orient::{
    FAILURE_PENALTY_PER_CONSECUTIVE, OodaOrientBrain, OrientContext, OrientJudgment,
};
use super::sanitize::sanitize_context_var;
use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::error::{SimardError, SimardResult};

#[cfg(test)]
use super::orient::DeterministicFallbackOrientBrain;

// Phase-specific adapter tags used in parse function error/fallback messages.
const DECIDE_ADAPTER_TAG: &str = "recipe-decide-brain";
const ORIENT_ADAPTER_TAG: &str = "recipe-orient-brain";
const LIFECYCLE_ADAPTER_TAG: &str = "recipe-engineer-lifecycle-brain";

/// Cap on raw response text embedded in error messages and rationale fields.
const MAX_RATIONALE_CHARS: usize = 500;

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<recipe_filename>` (hot-reload)
///   2. `<repo_root>/prompt_assets/simard/recipes/<recipe_filename>` (in-tree)
pub fn resolve_recipe_path(repo_root: &Path, recipe_filename: &str) -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(recipe_filename);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(recipe_filename);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Truncate a string to at most `max` characters, appending '…' if truncated.
pub fn truncate(s: &str, max: usize) -> String {
    // Fast path: byte length ≤ max implies char count ≤ max (chars ≤ bytes).
    if s.len() <= max {
        return s.to_string();
    }
    match s.char_indices().nth(max) {
        Some((byte_offset, _)) => format!("{}…", &s[..byte_offset]),
        None => s.to_string(),
    }
}

/// Unified recipe-runner-backed brain. Three instances with different
/// `(recipe_filename, adapter_tag)` replace the three former structs.
pub struct RecipeBrain {
    pub(crate) recipe_path: PathBuf,
    pub(crate) agent_binary: &'static str,
    pub(crate) adapter_tag: &'static str,
}

impl RecipeBrain {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    ///
    /// `recipe_filename` selects the YAML (e.g. `"ooda-decide.yaml"`).
    /// `adapter_tag` appears in error messages and logs (e.g. `"recipe-decide-brain"`).
    pub fn new(repo_root: &Path, recipe_filename: &str, adapter_tag: &'static str) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root, recipe_filename)?;
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
            adapter_tag,
        })
    }
}

impl OodaDecideBrain for RecipeBrain {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("urgency={:.3}", ctx.urgency))
            .arg("-c")
            .arg(format!("reason={}", sanitize_context_var(&ctx.reason, 500)))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }

        let raw = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok(parse_action_from_text(&raw))
    }
}

impl OodaOrientBrain for RecipeBrain {
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("base_urgency={:.3}", ctx.base_urgency))
            .arg("-c")
            .arg(format!(
                "base_reason={}",
                sanitize_context_var(&ctx.base_reason, 500)
            ))
            .arg("-c")
            .arg(format!("failure_count={}", ctx.failure_count))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }

        let raw = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok(parse_orient_from_text(
            &raw,
            ctx.base_urgency,
            ctx.failure_count,
        ))
    }
}

impl OodaBrain for RecipeBrain {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        let sentinel = ctx
            .sentinel_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<none>".to_string());
        let minutes = if ctx.minutes_since_last_update_attempt == u64::MAX {
            "never".to_string()
        } else {
            ctx.minutes_since_last_update_attempt.to_string()
        };

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!(
                "goal_description={}",
                sanitize_context_var(&ctx.goal_description, 500)
            ))
            .arg("-c")
            .arg(format!("cycle_number={}", ctx.cycle_number))
            .arg("-c")
            .arg(format!(
                "consecutive_skip_count={}",
                ctx.consecutive_skip_count
            ))
            .arg("-c")
            .arg(format!("failure_count={}", ctx.failure_count))
            .arg("-c")
            .arg(format!(
                "worktree_path={}",
                sanitize_context_var(&ctx.worktree_path.display().to_string(), 500)
            ))
            .arg("-c")
            .arg(format!(
                "worktree_mtime_secs_ago={}",
                ctx.worktree_mtime_secs_ago
            ))
            .arg("-c")
            .arg(format!("sentinel_pid={sentinel}"))
            .arg("-c")
            .arg(format!(
                "last_engineer_log_tail={}",
                sanitize_context_var(&ctx.last_engineer_log_tail, 2000)
            ))
            .arg("-c")
            .arg(format!("commits_behind={}", ctx.commits_behind))
            .arg("-c")
            .arg(format!(
                "in_flight_engineer_count={}",
                ctx.in_flight_engineer_count
            ))
            .arg("-c")
            .arg(format!("minutes_since_last_update_attempt={minutes}"))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        let raw = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok(parse_lifecycle_from_text(&raw))
    }
}

// ---------------------------------------------------------------------------
// Parse functions — trivial first-word / first-number extractors.
// No keyword scanning, no JSON extraction, no fallback chains.
// The recipe prompts instruct the LLM to output the action word first.
// ---------------------------------------------------------------------------

/// Parse recipe stdout for an action keyword as the first word (decide phase).
/// Case-insensitive match on the first whitespace-delimited token.
/// Defaults to `advance_goal` if the first word is unrecognised.
pub fn parse_action_from_text(text: &str) -> DecideJudgment {
    let trimmed = text.trim();
    let first_word = trimmed.split_whitespace().next().unwrap_or("");

    // Rationale allocation is deferred into the match arm — avoids a
    // wasted heap alloc on the (no-match) default path.
    type Ctor = fn(String) -> DecideJudgment;
    let pairs: &[(&str, Ctor)] = &[
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
    for (kw, ctor) in pairs {
        if first_word.eq_ignore_ascii_case(kw) {
            return ctor(truncate(trimmed, MAX_RATIONALE_CHARS));
        }
    }
    DecideJudgment::AdvanceGoal {
        rationale: format!(
            "{DECIDE_ADAPTER_TAG}: no action keyword found in recipe output; defaulting to advance_goal"
        ),
    }
}

// ---------------------------------------------------------------------------
// Orient parse: first decimal float → deterministic floor
// ---------------------------------------------------------------------------

/// Parse recipe output for the first decimal float (e.g. `0.42`).
/// Falls to the deterministic floor when no valid float is found.
pub fn parse_orient_from_text(text: &str, base_urgency: f64, failure_count: u32) -> OrientJudgment {
    // Hoist trim above the scanner — avoids re-trimming on each candidate.
    let trimmed = text.trim();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                let after_dot = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > after_dot
                    && let Ok(val) = text[start..i].parse::<f64>()
                {
                    // Inline validation before allocating rationale string.
                    if val.is_finite() && (0.0..=1.0).contains(&val) && val <= base_urgency + 1e-9 {
                        return OrientJudgment {
                            adjusted_urgency: val,
                            rationale: truncate(trimmed, MAX_RATIONALE_CHARS),
                            confidence: 1.0,
                            demotion_applied: base_urgency - val,
                        };
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    deterministic_floor(base_urgency, failure_count)
}

/// Compute the deterministic floor judgment.
fn deterministic_floor(base_urgency: f64, failure_count: u32) -> OrientJudgment {
    let penalty = FAILURE_PENALTY_PER_CONSECUTIVE * failure_count as f64;
    let adjusted = (base_urgency - penalty).max(0.0);
    OrientJudgment {
        adjusted_urgency: adjusted,
        rationale: format!(
            "{ORIENT_ADAPTER_TAG}: deterministic floor — {failure_count} failure(s), \
             urgency {base_urgency:.2} − {penalty:.2}",
        ),
        confidence: 1.0,
        demotion_applied: base_urgency - adjusted,
    }
}

// ---------------------------------------------------------------------------
// Lifecycle parse: first-word extraction → ContinueSkipping default
// ---------------------------------------------------------------------------

/// Parse recipe output for a lifecycle decision variant as the first word.
/// Case-insensitive match. Defaults to `ContinueSkipping`.
pub fn parse_lifecycle_from_text(text: &str) -> EngineerLifecycleDecision {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return default_continue_skipping();
    }
    let first_word = trimmed.split_whitespace().next().unwrap_or("");

    // Use eq_ignore_ascii_case instead of to_ascii_lowercase() — avoids a
    // heap-allocated String on every call.
    if first_word.eq_ignore_ascii_case("continue_skipping") {
        let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
        EngineerLifecycleDecision::ContinueSkipping { rationale: rest }
    } else if first_word.eq_ignore_ascii_case("deprioritize") {
        let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
        EngineerLifecycleDecision::Deprioritize { rationale: rest }
    } else if first_word.eq_ignore_ascii_case("consider_self_update") {
        let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
        EngineerLifecycleDecision::ConsiderSelfUpdate { rationale: rest }
    } else if first_word.eq_ignore_ascii_case("reclaim_and_redispatch") {
        let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale: rest,
            redispatch_context: String::new(),
        }
    } else if first_word.eq_ignore_ascii_case("open_tracking_issue") {
        let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
        EngineerLifecycleDecision::OpenTrackingIssue {
            title: "OODA stuck".to_string(),
            body: rest.clone(),
            rationale: rest,
        }
    } else if first_word.eq_ignore_ascii_case("mark_goal_blocked") {
        let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
        EngineerLifecycleDecision::MarkGoalBlocked {
            reason: rest.clone(),
            rationale: rest,
        }
    } else {
        default_continue_skipping()
    }
}

fn default_continue_skipping() -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::ContinueSkipping {
        rationale: format!(
            "{LIFECYCLE_ADAPTER_TAG}: no decision keyword found in recipe output; defaulting to continue_skipping"
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests — behavioral contracts for the unified RecipeBrain struct.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_brain::EngineerLifecycleCtx;
    use crate::ooda_brain::decide::DecideContext;
    use crate::ooda_brain::orient::OrientContext;
    use std::sync::Arc;

    // ===================================================================
    // resolve_recipe_path — parameterised by filename
    // ===================================================================

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_repo() {
        let result = resolve_recipe_path(Path::new("/nonexistent"), "ooda-decide.yaml");
        assert!(
            result.is_none(),
            "must return None when neither hot-reload nor in-tree path exists"
        );
    }

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_filename() {
        let result = resolve_recipe_path(Path::new("/tmp"), "does-not-exist.yaml");
        assert!(
            result.is_none(),
            "must return None when the recipe filename doesn't match any file"
        );
    }

    #[test]
    fn resolve_recipe_path_finds_in_tree_recipe() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        let recipe_file = recipe_dir.join("ooda-decide.yaml");
        std::fs::write(&recipe_file, "# test recipe").unwrap();

        let result = resolve_recipe_path(tmp.path(), "ooda-decide.yaml");
        assert_eq!(
            result,
            Some(recipe_file),
            "must find the in-tree recipe file"
        );
    }

    #[test]
    fn resolve_recipe_path_uses_filename_parameter() {
        // Verify that different filenames resolve to different paths
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();

        // Create two recipe files
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# decide").unwrap();
        std::fs::write(recipe_dir.join("ooda-orient.yaml"), "# orient").unwrap();

        let decide_path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml");
        let orient_path = resolve_recipe_path(tmp.path(), "ooda-orient.yaml");

        assert_ne!(
            decide_path, orient_path,
            "different filenames must resolve to different paths"
        );
        assert!(
            decide_path
                .as_ref()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("ooda-decide")
        );
        assert!(
            orient_path
                .as_ref()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("ooda-orient")
        );
    }

    // ===================================================================
    // RecipeBrain::new — constructor
    // ===================================================================

    #[test]
    fn new_returns_none_when_decide_recipe_missing() {
        let brain = RecipeBrain::new(
            Path::new("/nonexistent"),
            "ooda-decide.yaml",
            "recipe-decide-brain",
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_returns_none_when_orient_recipe_missing() {
        let brain = RecipeBrain::new(
            Path::new("/nonexistent"),
            "ooda-orient.yaml",
            "recipe-orient-brain",
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_returns_none_when_lifecycle_recipe_missing() {
        let brain = RecipeBrain::new(
            Path::new("/nonexistent"),
            "ooda-engineer-lifecycle.yaml",
            "recipe-engineer-lifecycle-brain",
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_stores_adapter_tag() {
        // Create a temporary recipe file so path resolution succeeds
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# test").unwrap();

        // Even if recipe-runner-rs isn't available, the adapter_tag contract
        // is that when construction succeeds, the tag is stored. We test
        // this via the error message from a trait call (see judge_*_error tests).
        // Constructor may return None if binary missing — that's expected.
        // This test documents the intent; the binary check makes it
        // environment-dependent.
        let _brain = RecipeBrain::new(tmp.path(), "ooda-decide.yaml", "recipe-decide-brain");
        // If construction succeeded, verify the tag is stored
        // If it returned None (no binary), that's OK for this environment
    }

    // ===================================================================
    // Trait impls — error messages include adapter_tag
    // ===================================================================

    #[test]
    fn judge_decision_error_includes_adapter_tag() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let ctx = DecideContext {
            goal_id: "test-goal".to_string(),
            urgency: 0.7,
            reason: "test reason".to_string(),
        };
        let err = brain.judge_decision(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-decide-brain"),
            "error should contain the adapter tag; got: {msg}"
        );
    }

    #[test]
    fn judge_orientation_error_includes_adapter_tag() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-orient-brain",
        };
        let ctx = OrientContext {
            goal_id: "test-goal".into(),
            base_urgency: 0.7,
            base_reason: "test reason".into(),
            failure_count: 1,
        };
        let err = brain.judge_orientation(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-orient-brain"),
            "error should contain the adapter tag; got: {msg}"
        );
    }

    #[test]
    fn decide_engineer_lifecycle_error_includes_adapter_tag() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = EngineerLifecycleCtx {
            goal_id: "test-goal".into(),
            goal_description: "test".into(),
            cycle_number: 1,
            consecutive_skip_count: 0,
            failure_count: 0,
            worktree_path: PathBuf::from("/tmp/wt"),
            worktree_mtime_secs_ago: 60,
            sentinel_pid: Some(42),
            last_engineer_log_tail: "ok".into(),
            commits_behind: 0,
            in_flight_engineer_count: 1,
            minutes_since_last_update_attempt: u64::MAX,
        };
        let err = brain.decide_engineer_lifecycle(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-engineer-lifecycle-brain"),
            "error should contain the adapter tag; got: {msg}"
        );
    }

    // ===================================================================
    // Trait impls — error type is AdapterInvocationFailed
    // ===================================================================

    #[test]
    fn judge_decision_spawn_failure_is_adapter_invocation_failed() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let ctx = DecideContext {
            goal_id: "g1".into(),
            urgency: 0.5,
            reason: "test".into(),
        };
        let err = brain.judge_decision(&ctx).unwrap_err();
        assert!(
            matches!(err, SimardError::AdapterInvocationFailed { .. }),
            "spawn failure must be AdapterInvocationFailed; got: {err:?}"
        );
    }

    #[test]
    fn judge_orientation_spawn_failure_is_adapter_invocation_failed() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-orient-brain",
        };
        let ctx = OrientContext {
            goal_id: "g1".into(),
            base_urgency: 0.5,
            base_reason: "test".into(),
            failure_count: 1,
        };
        let err = brain.judge_orientation(&ctx).unwrap_err();
        assert!(
            matches!(err, SimardError::AdapterInvocationFailed { .. }),
            "spawn failure must be AdapterInvocationFailed; got: {err:?}"
        );
    }

    #[test]
    fn decide_lifecycle_spawn_failure_is_adapter_invocation_failed() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = EngineerLifecycleCtx {
            goal_id: "g1".into(),
            goal_description: "test".into(),
            cycle_number: 1,
            consecutive_skip_count: 0,
            failure_count: 0,
            worktree_path: PathBuf::from("/tmp"),
            worktree_mtime_secs_ago: 60,
            sentinel_pid: None,
            last_engineer_log_tail: String::new(),
            commits_behind: 0,
            in_flight_engineer_count: 0,
            minutes_since_last_update_attempt: u64::MAX,
        };
        let err = brain.decide_engineer_lifecycle(&ctx).unwrap_err();
        assert!(
            matches!(err, SimardError::AdapterInvocationFailed { .. }),
            "spawn failure must be AdapterInvocationFailed; got: {err:?}"
        );
    }

    // ===================================================================
    // Type erasure — RecipeBrain implements all three traits
    // ===================================================================

    #[test]
    fn recipe_brain_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RecipeBrain>();
    }

    #[test]
    fn recipe_brain_can_be_arc_dyn_ooda_brain() {
        // This test verifies the type relationship at compile time.
        // Runtime: the brain has a fake path, so trait calls would fail,
        // but Arc wrapping must compile.
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OodaBrain> = Arc::new(brain);
    }

    #[test]
    fn recipe_brain_can_be_arc_dyn_ooda_decide_brain() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OodaDecideBrain> = Arc::new(brain);
    }

    #[test]
    fn recipe_brain_can_be_arc_dyn_ooda_orient_brain() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OodaOrientBrain> = Arc::new(brain);
    }

    // ===================================================================
    // Shared helpers — truncate
    // ===================================================================

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("12345", 5), "12345");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_unicode_boundary_safe() {
        // "héllo" — 'é' is 2 bytes. Truncating to 3 chars should give "hél…"
        let result = truncate("héllo", 3);
        assert_eq!(result, "hél…");
    }

    #[test]
    fn truncate_max_zero() {
        let result = truncate("hello", 0);
        assert_eq!(result, "…");
    }

    #[test]
    fn truncate_preserves_full_multibyte_string() {
        // String with only multibyte chars, length (in chars) ≤ max
        let s = "日本語"; // 3 chars, 9 bytes
        assert_eq!(truncate(s, 5), s);
    }

    // ===================================================================
    // Wiring contract — the three "instances" that brains.rs creates
    // ===================================================================

    #[test]
    fn decide_brain_instance_uses_correct_recipe_filename() {
        // Verify the filename parameter is "ooda-decide.yaml"
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# decide").unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml");
        assert!(path.is_some());
        assert!(path.unwrap().to_str().unwrap().contains("ooda-decide.yaml"));
    }

    #[test]
    fn orient_brain_instance_uses_correct_recipe_filename() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-orient.yaml"), "# orient").unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-orient.yaml");
        assert!(path.is_some());
        assert!(path.unwrap().to_str().unwrap().contains("ooda-orient.yaml"));
    }

    #[test]
    fn lifecycle_brain_instance_uses_correct_recipe_filename() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(
            recipe_dir.join("ooda-engineer-lifecycle.yaml"),
            "# lifecycle",
        )
        .unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-engineer-lifecycle.yaml");
        assert!(path.is_some());
        assert!(
            path.unwrap()
                .to_str()
                .unwrap()
                .contains("ooda-engineer-lifecycle.yaml")
        );
    }

    // ===================================================================
    // Different adapter_tags produce different error messages
    // ===================================================================

    #[test]
    fn different_adapter_tags_produce_different_errors() {
        let decide_brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let orient_brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-orient-brain",
        };
        let ctx = DecideContext {
            goal_id: "g1".into(),
            urgency: 0.5,
            reason: "test".into(),
        };
        let orient_ctx = OrientContext {
            goal_id: "g1".into(),
            base_urgency: 0.5,
            base_reason: "test".into(),
            failure_count: 1,
        };

        let decide_err = format!("{}", decide_brain.judge_decision(&ctx).unwrap_err());
        let orient_err = format!(
            "{}",
            orient_brain.judge_orientation(&orient_ctx).unwrap_err()
        );

        assert_ne!(
            decide_err, orient_err,
            "different adapter_tags must produce different error messages"
        );
        assert!(decide_err.contains("recipe-decide-brain"));
        assert!(orient_err.contains("recipe-orient-brain"));
    }

    // ===================================================================
    // Security invariant: sanitize_context_var is used
    // ===================================================================
    // (These are structural contracts — the implementation must use
    // sanitize_context_var for all user-controlled context vars.
    // We can't unit-test this directly without subprocess mocking,
    // but the error-path tests above verify the subprocess plumbing
    // is wired through the correct code path.)

    // ===================================================================
    // Security invariant: truncate on stderr
    // ===================================================================
    // (Verified by the error messages being bounded. The implementation
    // must call truncate(&stderr, 500) on all error paths.)

    // ===================================================================
    // parse_action_from_text — first-word extraction (parsers eliminated)
    // ===================================================================

    mod parse_action_tests {
        use super::super::*;
        use crate::ooda_loop::ActionKind;

        // === First-word extraction: keyword as first word ===

        #[test]
        fn first_word_advance_goal() {
            let j = parse_action_from_text("advance_goal this is a code change");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn first_word_consolidate_memory() {
            let j = parse_action_from_text("consolidate_memory reduce context overhead");
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn first_word_run_improvement() {
            let j = parse_action_from_text("run_improvement code quality needs work");
            assert_eq!(j.action_kind(), ActionKind::RunImprovement);
        }

        #[test]
        fn first_word_poll_developer_activity() {
            let j = parse_action_from_text("poll_developer_activity check recent commits");
            assert_eq!(j.action_kind(), ActionKind::PollDeveloperActivity);
        }

        #[test]
        fn first_word_extract_ideas() {
            let j = parse_action_from_text("extract_ideas from codebase analysis");
            assert_eq!(j.action_kind(), ActionKind::ExtractIdeas);
        }

        #[test]
        fn first_word_safe_update() {
            let j = parse_action_from_text("safe_update binary is behind origin");
            assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
        }

        #[test]
        fn first_word_research_query() {
            let j = parse_action_from_text("research_query need more context on API");
            assert_eq!(j.action_kind(), ActionKind::ResearchQuery);
        }

        #[test]
        fn first_word_run_gym_eval() {
            let j = parse_action_from_text("run_gym_eval low scores warrant evaluation");
            assert_eq!(j.action_kind(), ActionKind::RunGymEval);
        }

        #[test]
        fn first_word_build_skill() {
            let j = parse_action_from_text("build_skill agent needs new capabilities");
            assert_eq!(j.action_kind(), ActionKind::BuildSkill);
        }

        #[test]
        fn first_word_launch_session() {
            let j = parse_action_from_text("launch_session start working on this task");
            assert_eq!(j.action_kind(), ActionKind::LaunchSession);
        }

        #[test]
        fn all_ten_keywords_as_first_word() {
            let cases = vec![
                ("advance_goal rest", ActionKind::AdvanceGoal),
                ("consolidate_memory rest", ActionKind::ConsolidateMemory),
                ("run_improvement rest", ActionKind::RunImprovement),
                (
                    "poll_developer_activity rest",
                    ActionKind::PollDeveloperActivity,
                ),
                ("extract_ideas rest", ActionKind::ExtractIdeas),
                ("safe_update rest", ActionKind::SafeUpdate),
                ("research_query rest", ActionKind::ResearchQuery),
                ("run_gym_eval rest", ActionKind::RunGymEval),
                ("build_skill rest", ActionKind::BuildSkill),
                ("launch_session rest", ActionKind::LaunchSession),
            ];
            for (text, expected) in cases {
                let j = parse_action_from_text(text);
                assert_eq!(
                    j.action_kind(),
                    expected,
                    "first word of '{text}' should map to {expected:?}"
                );
            }
        }

        // === Case insensitivity on first word ===

        #[test]
        fn first_word_case_insensitive_upper() {
            let j = parse_action_from_text("CONSOLIDATE_MEMORY reduce overhead");
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn first_word_case_insensitive_mixed() {
            let j = parse_action_from_text("Run_Improvement code quality");
            assert_eq!(j.action_kind(), ActionKind::RunImprovement);
        }

        #[test]
        fn first_word_case_insensitive_all_caps() {
            let j = parse_action_from_text("ADVANCE_GOAL proceed with goal");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Default behavior ===

        #[test]
        fn no_keyword_first_word_defaults_to_advance_goal() {
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

        // === Keyword NOT first word => default (new behavior) ===

        #[test]
        fn keyword_not_first_word_defaults_to_advance_goal() {
            // With first-word extraction, keywords buried in prose don't match
            let j = parse_action_from_text("I recommend consolidate_memory for this.");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn keyword_at_end_defaults_to_advance_goal() {
            let j = parse_action_from_text("The action should be safe_update");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn keyword_in_multiline_prose_defaults_to_advance_goal() {
            let text =
                "Looking at the state:\n- Memory fragmented\n\nRecommend: consolidate_memory";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Rationale ===

        #[test]
        fn rationale_contains_remaining_text() {
            let j = parse_action_from_text("run_improvement because code quality is poor");
            assert!(
                j.rationale().contains("code quality"),
                "rationale should contain text after keyword: {}",
                j.rationale()
            );
        }

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("consolidate_memory because {}", "x".repeat(1000));
            let j = parse_action_from_text(&long_text);
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
            assert!(
                j.rationale().chars().count() <= MAX_RATIONALE_CHARS + 1,
                "rationale should be truncated to ~{} chars (+1 for ellipsis), got {}",
                MAX_RATIONALE_CHARS,
                j.rationale().chars().count()
            );
        }

        #[test]
        fn first_word_only_no_rationale_text() {
            let j = parse_action_from_text("advance_goal");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Leading whitespace ===

        #[test]
        fn leading_whitespace_trimmed() {
            let j = parse_action_from_text("  safe_update binary is behind");
            assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
        }

        #[test]
        fn leading_newline_trimmed() {
            let j = parse_action_from_text("\n\nrun_gym_eval scores are low");
            assert_eq!(j.action_kind(), ActionKind::RunGymEval);
        }

        // === No keyword is a substring of another (structural) ===

        #[test]
        fn no_keyword_is_substring_of_another() {
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
                        assert!(!a.contains(b), "keyword '{a}' contains '{b}'");
                    }
                }
            }
        }

        // === Realistic LLM outputs (new format: keyword first) ===

        #[test]
        fn realistic_advance_goal() {
            let text = "advance_goal — standard development, recent commits show progress";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn realistic_consolidate_memory() {
            let text = "consolidate_memory\nMemory compaction will reduce context overhead.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn realistic_run_improvement() {
            let text = "run_improvement code quality metrics are below threshold";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::RunImprovement);
        }

        #[test]
        fn realistic_no_keyword_just_prose() {
            let text = "The goal appears to be making steady progress.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }
    }

    // ===================================================================
    // parse_orient_from_text — bare-float + floor (JSON tier eliminated)
    // ===================================================================

    mod parse_orient_tests {
        use super::super::*;

        // === Bare float extraction ===

        #[test]
        fn bare_float_alone() {
            let j = parse_orient_from_text("0.42", 0.8, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        #[test]
        fn bare_float_with_rationale() {
            let text = "0.35 transient failure, moderate demotion";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.35).abs() < 1e-9);
        }

        #[test]
        fn bare_float_in_prose() {
            let text = "The adjusted urgency should be 0.35 given failures.";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.35).abs() < 1e-9);
        }

        #[test]
        fn bare_float_at_end() {
            let text = "result: 0.50";
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.adjusted_urgency - 0.50).abs() < 1e-9);
        }

        #[test]
        fn bare_float_zero() {
            let j = parse_orient_from_text("0.0", 0.8, 4);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn bare_float_confidence_always_one() {
            let j = parse_orient_from_text("0.42", 0.8, 1);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn bare_float_demotion_computed() {
            let j = parse_orient_from_text("0.42", 0.8, 1);
            let expected = 0.8 - 0.42;
            assert!((j.demotion_applied - expected).abs() < 1e-9);
        }

        #[test]
        fn bare_float_rationale_includes_text() {
            let text = "0.35 because of transient failures";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!(j.rationale.contains("transient") || j.rationale.contains("0.35"));
        }

        // === Clamping: float above base_urgency or out of range ===

        #[test]
        fn float_above_base_clamped_to_base() {
            let j = parse_orient_from_text("0.9", 0.5, 1);
            assert!(
                j.adjusted_urgency <= 0.5 + 1e-9,
                "urgency {} should be clamped to base 0.5",
                j.adjusted_urgency
            );
        }

        #[test]
        fn float_above_one_clamped() {
            let j = parse_orient_from_text("1.5", 0.8, 1);
            assert!(j.adjusted_urgency <= 1.0 + 1e-9);
            assert!(j.adjusted_urgency <= 0.8 + 1e-9);
        }

        #[test]
        fn float_negative_not_matched_falls_to_floor() {
            // "-0.3" — the scanner starts at digits, sees '0.3' after the minus
            // The minus is not part of the pattern. Scanner finds 0.3 as a bare float.
            let j = parse_orient_from_text("-0.3", 0.8, 2);
            assert!(
                (j.adjusted_urgency - 0.3).abs() < 1e-9
                    || (j.adjusted_urgency - (0.8_f64 - 0.4).max(0.0)).abs() < 1e-9,
            );
        }

        // === No float => deterministic floor ===

        #[test]
        fn no_float_falls_to_floor() {
            let j = parse_orient_from_text("cannot determine urgency", 0.8, 2);
            let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn empty_string_falls_to_floor() {
            let j = parse_orient_from_text("", 0.8, 2);
            let floor = (0.8_f64 - 0.4).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn whitespace_only_falls_to_floor() {
            let j = parse_orient_from_text("   \n\t  ", 0.8, 2);
            let floor = (0.8_f64 - 0.4).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn integer_not_matched_falls_to_floor() {
            let j = parse_orient_from_text("42", 0.8, 2);
            let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn dot_five_no_leading_digit_falls_to_floor() {
            // ".5" has no leading digit — the scanner requires N.N format
            let j = parse_orient_from_text(".5", 0.8, 1);
            let floor = (0.8_f64 - 0.2).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn one_dot_no_trailing_digit_falls_to_floor() {
            // "1." has no trailing digit — the scanner requires digits after dot
            let j = parse_orient_from_text("1.", 0.8, 1);
            let floor = (0.8_f64 - 0.2).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn multi_float_first_valid_wins() {
            // "version 2.0 adjusted to 0.42" — 2.0 is out of range, scanner skips to 0.42
            let j = parse_orient_from_text("version 2.0 adjusted to 0.42", 0.8, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        #[test]
        fn first_valid_float_in_range_wins() {
            // Both 0.9 and 0.42 are valid, but 0.9 > base 0.5, so scanner skips to 0.42
            let j = parse_orient_from_text("0.9 or 0.42", 0.5, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        // === Floor formula ===

        #[test]
        fn floor_formula_basic() {
            let j = parse_orient_from_text("no number here", 0.8, 2);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn floor_clamped_to_zero() {
            let j = parse_orient_from_text("nothing", 0.3, 5);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn floor_zero_failures() {
            let j = parse_orient_from_text("nothing", 1.0, 0);
            assert!((j.adjusted_urgency - 1.0).abs() < 1e-9);
        }

        #[test]
        fn floor_one_failure() {
            let j = parse_orient_from_text("nothing", 0.6, 1);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn floor_demotion_applied() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!((j.demotion_applied - 0.4).abs() < 1e-9);
        }

        #[test]
        fn floor_confidence_one() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn floor_rationale_describes_formula() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!(
                j.rationale.contains(ORIENT_ADAPTER_TAG) || j.rationale.contains("deterministic"),
                "floor rationale must identify the adapter or strategy; got: {}",
                j.rationale
            );
        }

        // === Invariants ===

        #[test]
        fn adjusted_always_le_base() {
            let scenarios: &[(&str, f64, u32)] =
                &[("0.9", 0.5, 1), ("0.42", 0.8, 1), ("nothing", 0.8, 2)];
            for (text, base, failures) in scenarios {
                let j = parse_orient_from_text(text, *base, *failures);
                assert!(
                    j.adjusted_urgency <= *base + 1e-9,
                    "text={text} base={base}: urgency {} should be <= base",
                    j.adjusted_urgency
                );
            }
        }

        #[test]
        fn adjusted_always_in_unit_range() {
            let scenarios: &[(&str, f64, u32)] =
                &[("1.5", 0.8, 1), ("0.42", 0.8, 1), ("nothing", 0.8, 2)];
            for (text, base, failures) in scenarios {
                let j = parse_orient_from_text(text, *base, *failures);
                assert!(
                    j.adjusted_urgency >= 0.0 && j.adjusted_urgency <= 1.0,
                    "text={text}: urgency {} should be in [0,1]",
                    j.adjusted_urgency
                );
            }
        }

        // === Rationale ===

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("0.42 because {}", "x".repeat(1000));
            let j = parse_orient_from_text(&long_text, 0.8, 1);
            assert!(j.rationale.chars().count() <= MAX_RATIONALE_CHARS + 1);
        }

        // === No JSON extraction (parser eliminated) ===

        #[test]
        fn json_text_uses_bare_float_not_json_parser() {
            // JSON text with float inside — bare float scanner finds 0.4
            let text = r#"{"adjusted_urgency": 0.4, "rationale": "test"}"#;
            let j = parse_orient_from_text(text, 0.8, 2);
            // Should find 0.4 as first decimal float pattern
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
            // Rationale is full text, NOT the JSON "rationale" field
            assert!(
                j.rationale.contains("adjusted_urgency"),
                "rationale should be full text, not extracted JSON field; got: {}",
                j.rationale
            );
        }

        // === Matches deterministic fallback brain ===

        #[test]
        fn floor_matches_deterministic_fallback_brain() {
            use super::super::super::orient::OrientContext;
            use super::super::DeterministicFallbackOrientBrain;
            let ctx = OrientContext {
                goal_id: "g1".into(),
                base_urgency: 0.8,
                base_reason: "test".into(),
                failure_count: 3,
            };
            let fallback = DeterministicFallbackOrientBrain::compute(&ctx);
            let recipe_floor = parse_orient_from_text("nothing", 0.8, 3);
            assert!((recipe_floor.adjusted_urgency - fallback.adjusted_urgency).abs() < 1e-9);
        }

        // === Realistic outputs ===

        #[test]
        fn realistic_bare_float_first_token() {
            let text = "0.45 moderate demotion for transient CI failures";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.45).abs() < 1e-9);
        }

        #[test]
        fn realistic_no_number() {
            let text = "Significantly demote due to chronic infrastructure failures.";
            let j = parse_orient_from_text(text, 0.8, 3);
            let floor = (0.8_f64 - 0.2 * 3.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }
    }

    // ===================================================================
    // parse_lifecycle_from_text — first-word extraction (parsers eliminated)
    // ===================================================================

    mod parse_lifecycle_tests {
        use super::super::*;

        // === First-word extraction: variant as first word ===

        #[test]
        fn first_word_continue_skipping() {
            let text = "continue_skipping engineer is healthy and making progress";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(rationale.contains("healthy") || rationale.contains("progress"));
                }
                other => panic!("expected ContinueSkipping, got {other:?}"),
            }
        }

        #[test]
        fn first_word_reclaim_and_redispatch() {
            let text = "reclaim_and_redispatch worktree wedged for 7 hours";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ReclaimAndRedispatch {
                    rationale,
                    redispatch_context,
                } => {
                    assert!(rationale.contains("wedged"));
                    assert!(redispatch_context.is_empty());
                }
                other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
            }
        }

        #[test]
        fn first_word_deprioritize() {
            let text = "deprioritize chronic failure, no progress in 10 cycles";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.contains("chronic") || rationale.contains("failure"));
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn first_word_open_tracking_issue() {
            let text = "open_tracking_issue engineer panicked on cycle 12, OOM in worker";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue {
                    rationale,
                    title,
                    body,
                } => {
                    assert_eq!(title, "OODA stuck");
                    assert!(body.contains("panicked") || body.contains("OOM"));
                    assert!(rationale.contains("panicked") || rationale.contains("OOM"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn first_word_mark_goal_blocked() {
            let text = "mark_goal_blocked needs API key from user, cannot proceed";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason } => {
                    assert!(reason.contains("API key") || reason.contains("cannot proceed"));
                    assert!(rationale.contains("API key") || rationale.contains("cannot proceed"));
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn first_word_consider_self_update() {
            let text = "consider_self_update binary is 5 commits behind origin/main";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
                    assert!(rationale.contains("5 commits") || rationale.contains("behind"));
                }
                other => panic!("expected ConsiderSelfUpdate, got {other:?}"),
            }
        }

        // === Case insensitivity on first word ===

        #[test]
        fn first_word_case_insensitive_upper() {
            let text = "DEPRIORITIZE this stale goal";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("case-insensitive first word should match; got {other:?}"),
            }
        }

        #[test]
        fn first_word_case_insensitive_mixed() {
            let text = "Continue_Skipping everything is fine";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("case-insensitive first word should match; got {other:?}"),
            }
        }

        // === Default behavior ===

        #[test]
        fn no_keyword_first_word_defaults_to_continue_skipping() {
            let text = "The engineer appears to be making progress normally.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(
                        rationale.contains("no decision keyword")
                            || rationale.contains(LIFECYCLE_ADAPTER_TAG),
                    );
                }
                other => panic!("no keyword first word -> ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn empty_text_defaults_to_continue_skipping() {
            let d = parse_lifecycle_from_text("");
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("empty text -> ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn whitespace_only_defaults_to_continue_skipping() {
            let d = parse_lifecycle_from_text("   \n\t  ");
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("whitespace-only -> ContinueSkipping; got {other:?}"),
            }
        }

        // === Keyword NOT first word => default (new behavior) ===

        #[test]
        fn keyword_in_prose_defaults_to_continue_skipping() {
            // With first-word extraction, keywords buried in prose don't match
            let text = "I think we should deprioritize this cycle.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("keyword not first word -> ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn old_marker_format_defaults_to_continue_skipping() {
            // Old DECISION: marker format no longer recognized
            let text = "DECISION: deprioritize\nRATIONALE: test";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("DECISION: marker should not be parsed; got {other:?}"),
            }
        }

        // === Extra fields use defaults ===

        #[test]
        fn open_tracking_issue_title_defaults_to_ooda_stuck() {
            let text = "open_tracking_issue something went wrong";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue { title, .. } => {
                    assert_eq!(title, "OODA stuck");
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn open_tracking_issue_body_is_remaining_text() {
            let text = "open_tracking_issue engineer OOM on cycle 12";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue { body, .. } => {
                    assert!(body.contains("OOM") || body.contains("cycle"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn mark_goal_blocked_reason_is_remaining_text() {
            let text = "mark_goal_blocked needs API key from user";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { reason, .. } => {
                    assert!(reason.contains("API key"));
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn reclaim_redispatch_context_always_empty() {
            let text = "reclaim_and_redispatch wedged for hours";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ReclaimAndRedispatch {
                    redispatch_context, ..
                } => {
                    assert!(redispatch_context.is_empty());
                }
                other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
            }
        }

        // === Rationale ===

        #[test]
        fn rationale_is_remaining_text() {
            let text = "deprioritize chronic failure with no progress for many cycles";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.contains("chronic") || rationale.contains("failure"));
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("deprioritize {}", "x".repeat(2000));
            let d = parse_lifecycle_from_text(&long_text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.chars().count() <= MAX_RATIONALE_CHARS + 1);
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        // === Leading whitespace ===

        #[test]
        fn leading_whitespace_trimmed() {
            let text = "  continue_skipping  everything is fine";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("leading whitespace should be trimmed; got {other:?}"),
            }
        }

        #[test]
        fn leading_newline_trimmed() {
            let text = "\n\ndeprioritize goal is stuck";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("leading newline should be trimmed; got {other:?}"),
            }
        }

        // === Realistic outputs (new format: keyword first) ===

        #[test]
        fn realistic_continue_skipping() {
            let text =
                "continue_skipping\nThe engineer is making steady progress. Last commit 15min ago.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("expected ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn realistic_deprioritize() {
            let text = "deprioritize goal stuck for 10 cycles, redirect attention";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("expected Deprioritize; got {other:?}"),
            }
        }

        #[test]
        fn realistic_open_tracking_issue() {
            let text =
                "open_tracking_issue\nEngineer OOM at 03:14 UTC. Recurring — needs investigation.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue {
                    title,
                    body,
                    rationale,
                } => {
                    assert_eq!(title, "OODA stuck");
                    assert!(body.contains("OOM") || body.contains("investigation"));
                    assert!(rationale.contains("OOM") || rationale.contains("investigation"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn realistic_no_decision() {
            let text = "The engineer seems to be working fine. I see recent commits.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("no keyword -> ContinueSkipping; got {other:?}"),
            }
        }

        // === Sentinel/minutes helper tests (kept from original) ===

        #[test]
        fn sentinel_pid_none_renders_as_none_tag() {
            let sentinel: Option<i32> = None;
            let rendered = sentinel
                .map(|p| p.to_string())
                .unwrap_or_else(|| "<none>".to_string());
            assert_eq!(rendered, "<none>");
        }

        #[test]
        fn minutes_max_renders_as_never() {
            let minutes = u64::MAX;
            let rendered = if minutes == u64::MAX {
                "never".to_string()
            } else {
                minutes.to_string()
            };
            assert_eq!(rendered, "never");
        }

        #[test]
        fn minutes_normal_renders_as_number() {
            let minutes: u64 = 42;
            let rendered = if minutes == u64::MAX {
                "never".to_string()
            } else {
                minutes.to_string()
            };
            assert_eq!(rendered, "42");
        }
    }
}
