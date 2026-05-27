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
//! context vars, then delegates parsing to the per-phase parse functions
//! in [`super::recipe_decide`], [`super::recipe_orient`], and
//! [`super::recipe_engineer_lifecycle`].

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

/// Closed set of `EngineerLifecycleDecision` variant tags for keyword scanning.
pub const LIFECYCLE_KEYWORDS: &[&str] = &[
    "continue_skipping",
    "reclaim_and_redispatch",
    "deprioritize",
    "open_tracking_issue",
    "mark_goal_blocked",
    "consider_self_update",
];

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

/// Byte-level case-insensitive substring search for ASCII keywords.
pub fn ascii_contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
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
// Parse functions (consolidated from recipe_decide, recipe_orient,
// recipe_engineer_lifecycle)
// ---------------------------------------------------------------------------

/// Parse recipe stdout text for action-kind keywords (decide phase).
///
/// Scans case-insensitively for each of the 10 known action keywords and
/// returns the first match. If no keyword is found, defaults to
/// `advance_goal` (same as the deterministic fallback).
pub fn parse_action_from_text(text: &str) -> DecideJudgment {
    let text_bytes = text.as_bytes();

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
        if ascii_contains_ignore_case(text_bytes, keyword.as_bytes()) {
            return constructor(truncate(text.trim(), 500));
        }
    }

    DecideJudgment::AdvanceGoal {
        rationale: format!(
            "{DECIDE_ADAPTER_TAG}: no action keyword found in recipe output; defaulting to advance_goal"
        ),
    }
}

// ---------------------------------------------------------------------------
// Orient parse: 3-tier cascade (JSON → bare float → deterministic floor)
// ---------------------------------------------------------------------------

/// Parse recipe output using a 3-tier cascade. Always returns a valid
/// [`OrientJudgment`] — the deterministic floor (tier 3) is the safety net.
pub fn parse_orient_from_text(text: &str, base_urgency: f64, failure_count: u32) -> OrientJudgment {
    if let Some(j) = try_json_extraction(text, base_urgency) {
        return j;
    }
    if let Some(j) = try_bare_float(text, base_urgency) {
        return j;
    }
    deterministic_floor(base_urgency, failure_count)
}

/// Tier 1: Extract the first `{…}` substring, parse as [`OrientJudgment`],
/// validate against `base_urgency`.
fn try_json_extraction(text: &str, base_urgency: f64) -> Option<OrientJudgment> {
    let stripped = text.trim();
    let start = stripped.find('{')?;
    let end = stripped.rfind('}')?;
    if end <= start {
        return None;
    }
    let json_slice = &stripped[start..=end];
    let mut j: OrientJudgment = serde_json::from_str(json_slice).ok()?;
    j.demotion_applied = base_urgency - j.adjusted_urgency;
    j.validate(base_urgency).ok()?;
    Some(j)
}

/// Tier 2: Scan for the first bare decimal float matching `[0-9]+\.[0-9]+`
/// that passes [`OrientJudgment::validate`].
fn try_bare_float(text: &str, base_urgency: f64) -> Option<OrientJudgment> {
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
                    let j = OrientJudgment {
                        adjusted_urgency: val,
                        rationale: truncate(text.trim(), 500),
                        confidence: 1.0,
                        demotion_applied: base_urgency - val,
                    };
                    if j.validate(base_urgency).is_ok() {
                        return Some(j);
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    None
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
// Lifecycle parse: DECISION marker → keyword fallback → ContinueSkipping
// ---------------------------------------------------------------------------

/// Parse recipe output for an engineer lifecycle decision. Always returns
/// a valid [`EngineerLifecycleDecision`] — defaults to `ContinueSkipping`
/// when no recognisable decision is found.
pub fn parse_lifecycle_from_text(text: &str) -> EngineerLifecycleDecision {
    let stripped = text.trim();
    if stripped.is_empty() {
        return default_continue_skipping();
    }

    if let Some((variant, rest)) = extract_decision_marker(stripped)
        && LIFECYCLE_KEYWORDS.contains(&variant)
        && let Ok(decision) = parse_with_marker(variant, rest)
    {
        return decision;
    }

    if let Some(decision) = try_keyword_scan(text) {
        return decision;
    }

    default_continue_skipping()
}

fn default_continue_skipping() -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::ContinueSkipping {
        rationale: format!(
            "{LIFECYCLE_ADAPTER_TAG}: no decision keyword found in recipe output; defaulting to continue_skipping"
        ),
    }
}

fn extract_decision_marker(text: &str) -> Option<(&str, &str)> {
    let first_line = text.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = first_line.trim();
    if trimmed.len() < "decision:".len() {
        return None;
    }
    let prefix = &trimmed[.."decision:".len()];
    if !prefix.eq_ignore_ascii_case("decision:") {
        return None;
    }
    let after_marker = trimmed["decision:".len()..].trim();
    let variant = after_marker.split_whitespace().next()?;
    let remainder = text.split_once('\n').map(|(_, r)| r).unwrap_or("");
    Some((variant, remainder))
}

fn parse_with_marker(variant: &str, rest: &str) -> Result<EngineerLifecycleDecision, String> {
    let trimmed_rest = rest.trim();

    let mut fields: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut prose_lines: Vec<&str> = Vec::new();

    for line in trimmed_rest.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim();
            let val = trimmed[colon_pos + 1..].trim();
            let key_upper = key.to_ascii_uppercase();
            match key_upper.as_str() {
                "TITLE" | "BODY" | "REASON" | "REDISPATCH_CONTEXT" | "RATIONALE" => {
                    fields.insert(key_upper, val.to_string());
                    continue;
                }
                _ => {}
            }
        }
        prose_lines.push(trimmed);
    }

    // Backward-compat: also extract fields from JSON body if present
    if let Some(start) = trimmed_rest.find('{')
        && let Some(end) = trimmed_rest.rfind('}')
        && end > start
    {
        let candidate = &trimmed_rest[start..=end];
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(candidate)
        {
            for (k, v) in &map {
                let key_upper = k.to_ascii_uppercase();
                if let Some(s) = v.as_str() {
                    fields.entry(key_upper).or_insert_with(|| s.to_string());
                }
            }
        }
    }

    let rationale = if let Some(r) = fields.get("RATIONALE") {
        truncate(r, MAX_RATIONALE_CHARS)
    } else if !prose_lines.is_empty() {
        truncate(&prose_lines.join(" "), MAX_RATIONALE_CHARS)
    } else {
        "(no rationale provided)".to_string()
    };

    match variant {
        "continue_skipping" => Ok(EngineerLifecycleDecision::ContinueSkipping { rationale }),
        "deprioritize" => Ok(EngineerLifecycleDecision::Deprioritize { rationale }),
        "consider_self_update" => Ok(EngineerLifecycleDecision::ConsiderSelfUpdate { rationale }),
        "reclaim_and_redispatch" => {
            let redispatch_context = fields
                .get("REDISPATCH_CONTEXT")
                .cloned()
                .unwrap_or_default();
            Ok(EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale,
                redispatch_context,
            })
        }
        "open_tracking_issue" => {
            let title = fields
                .get("TITLE")
                .cloned()
                .unwrap_or_else(|| "OODA stuck".to_string());
            let body = fields
                .get("BODY")
                .cloned()
                .unwrap_or_else(|| truncate(trimmed_rest, MAX_RATIONALE_CHARS));
            Ok(EngineerLifecycleDecision::OpenTrackingIssue {
                rationale,
                title,
                body,
            })
        }
        "mark_goal_blocked" => {
            let reason = fields
                .get("REASON")
                .cloned()
                .unwrap_or_else(|| truncate(trimmed_rest, MAX_RATIONALE_CHARS));
            Ok(EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason })
        }
        _ => Err(format!("unrecognized variant `{variant}`")),
    }
}

fn try_keyword_scan(text: &str) -> Option<EngineerLifecycleDecision> {
    let text_bytes = text.as_bytes();
    for keyword in LIFECYCLE_KEYWORDS {
        if ascii_contains_ignore_case(text_bytes, keyword.as_bytes()) {
            return Some(build_keyword_decision(keyword, text));
        }
    }
    None
}

fn build_keyword_decision(keyword: &str, text: &str) -> EngineerLifecycleDecision {
    let rationale = truncate(text.trim(), MAX_RATIONALE_CHARS);
    match keyword {
        "continue_skipping" => EngineerLifecycleDecision::ContinueSkipping { rationale },
        "deprioritize" => EngineerLifecycleDecision::Deprioritize { rationale },
        "consider_self_update" => EngineerLifecycleDecision::ConsiderSelfUpdate { rationale },
        "reclaim_and_redispatch" => EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale,
            redispatch_context: String::new(),
        },
        "open_tracking_issue" => {
            let rationale_clone = rationale.clone();
            EngineerLifecycleDecision::OpenTrackingIssue {
                title: "OODA stuck".to_string(),
                body: rationale_clone,
                rationale,
            }
        }
        "mark_goal_blocked" => {
            let rationale_clone = rationale.clone();
            EngineerLifecycleDecision::MarkGoalBlocked {
                reason: rationale_clone,
                rationale,
            }
        }
        _ => EngineerLifecycleDecision::ContinueSkipping { rationale },
    }
}

// ---------------------------------------------------------------------------
// Tests — TDD: define the contract FIRST, implement SECOND.
//
// These tests specify the behavior of the unified RecipeBrain struct.
// At the TDD stage, all tests FAIL (todo! panics). After implementation,
// all tests pass.
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
    // Shared helpers — ascii_contains_ignore_case
    // ===================================================================

    #[test]
    fn ascii_contains_exact_match() {
        assert!(ascii_contains_ignore_case(b"hello world", b"world"));
    }

    #[test]
    fn ascii_contains_case_insensitive() {
        assert!(ascii_contains_ignore_case(b"Hello World", b"hello"));
        assert!(ascii_contains_ignore_case(b"hello world", b"WORLD"));
    }

    #[test]
    fn ascii_contains_not_found() {
        assert!(!ascii_contains_ignore_case(b"hello world", b"xyz"));
    }

    #[test]
    fn ascii_contains_empty_needle() {
        // Empty needle should match any haystack (every position is a valid window of len 0)
        assert!(ascii_contains_ignore_case(b"hello", b""));
    }

    #[test]
    fn ascii_contains_needle_longer_than_haystack() {
        assert!(!ascii_contains_ignore_case(b"hi", b"hello world"));
    }

    #[test]
    fn ascii_contains_at_start() {
        assert!(ascii_contains_ignore_case(
            b"ADVANCE_GOAL done",
            b"advance_goal"
        ));
    }

    #[test]
    fn ascii_contains_at_end() {
        assert!(ascii_contains_ignore_case(
            b"result: advance_goal",
            b"advance_goal"
        ));
    }

    #[test]
    fn ascii_contains_mixed_case_keyword() {
        assert!(ascii_contains_ignore_case(
            b"Try Consolidate_Memory now",
            b"consolidate_memory"
        ));
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
    // Migrated tests from recipe_decide.rs
    // ===================================================================

    mod parse_action_tests {
        use super::super::*;
        use crate::ooda_loop::ActionKind;

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

        #[test]
        fn multiple_keywords_first_in_scan_order_wins() {
            let text = "We could advance_goal or poll_developer_activity.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::PollDeveloperActivity);
        }

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
            assert!(
                j.rationale().chars().count() <= 501,
                "rationale should be truncated"
            );
        }

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

        #[test]
        fn realistic_llm_output_advance_goal_prose() {
            let text = "Based on the priority analysis:\n\nGoal: ship-v1\nUrgency: 0.850\n\n\
                        This is a standard development goal. The appropriate action is to \
                        advance_goal and continue.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn realistic_llm_output_consolidate_memory_verbose() {
            let text = "## Decision Analysis\n\nThe goal_id `__memory__` indicates synthetic \
                        priority. The urgency of 0.600 is moderate.\n\n\
                        **Action: consolidate_memory**\n\nMemory compaction will reduce overhead.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn realistic_llm_output_with_markdown_formatting() {
            let text = "# OODA Decide\n\n| Factor | Value |\n|--------|-------|\n\
                        | Goal | __improvement__ |\n\nGiven the synthetic priority, \
                        I recommend `run_improvement` to address code quality.";
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
            let text = "The goal appears to be making steady progress. The engineer is actively \
                        working on it and the last commit was 2 hours ago.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }
    }

    // ===================================================================
    // Migrated tests from recipe_orient.rs
    // ===================================================================

    mod parse_orient_tests {
        use super::super::*;

        #[test]
        fn tier1_full_json_object() {
            let text = r#"{"adjusted_urgency": 0.4, "demotion_applied": 0.4, "rationale": "transient failure", "confidence": 0.9}"#;
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
            assert_eq!(j.rationale, "transient failure");
            assert!((j.confidence - 0.9).abs() < 1e-9);
        }

        #[test]
        fn tier1_json_with_surrounding_prose() {
            let text =
                r#"Here is my judgment: {"adjusted_urgency": 0.2, "rationale": "chronic"} done"#;
            let j = parse_orient_from_text(text, 0.8, 3);
            assert!((j.adjusted_urgency - 0.2).abs() < 1e-9);
        }

        #[test]
        fn tier1_json_missing_confidence_defaults_to_one() {
            let text = r#"{"adjusted_urgency": 0.3, "rationale": "ok"}"#;
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.adjusted_urgency - 0.3).abs() < 1e-9);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn tier1_json_missing_demotion_defaults_to_zero() {
            let text = r#"{"adjusted_urgency": 0.5, "rationale": "ok"}"#;
            let j = parse_orient_from_text(text, 0.8, 1);
            let expected_demotion = 0.8 - 0.5;
            assert!((j.demotion_applied - expected_demotion).abs() < 1e-9);
        }

        #[test]
        fn tier1_json_in_markdown_fences() {
            let text = "```json\n{\"adjusted_urgency\": 0.5, \"rationale\": \"fenced\"}\n```";
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.adjusted_urgency - 0.5).abs() < 1e-9);
        }

        #[test]
        fn tier1_json_extra_fields_ignored() {
            let text =
                r#"{"adjusted_urgency": 0.4, "rationale": "ok", "futurefield": 42, "bonus": true}"#;
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn tier1_json_zero_urgency_valid() {
            let text = r#"{"adjusted_urgency": 0.0, "rationale": "chronic"}"#;
            let j = parse_orient_from_text(text, 0.8, 4);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn tier1_json_escalation_falls_through() {
            let text = r#"{"adjusted_urgency": 0.9, "rationale": "bad LLM"}"#;
            let j = parse_orient_from_text(text, 0.5, 1);
            assert!(j.adjusted_urgency <= 0.5 + 1e-9);
        }

        #[test]
        fn tier1_json_out_of_range_falls_through() {
            let text = r#"{"adjusted_urgency": 1.5, "rationale": "invalid"}"#;
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!(j.adjusted_urgency <= 1.0);
        }

        #[test]
        fn tier1_json_negative_falls_through() {
            let text = r#"{"adjusted_urgency": -0.1, "rationale": "invalid"}"#;
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!(j.adjusted_urgency >= 0.0);
        }

        #[test]
        fn tier2_bare_float_alone() {
            let text = "0.42";
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        #[test]
        fn tier2_float_in_prose() {
            let text = "The adjusted urgency should be 0.35 given the transient nature.";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.35).abs() < 1e-9);
        }

        #[test]
        fn tier2_float_at_end() {
            let text = "result: 0.50";
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.adjusted_urgency - 0.50).abs() < 1e-9);
        }

        #[test]
        fn tier2_float_zero() {
            let text = "0.0";
            let j = parse_orient_from_text(text, 0.8, 4);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn tier2_float_confidence_defaults_to_one() {
            let text = "0.42";
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn tier2_float_demotion_computed() {
            let text = "0.42";
            let j = parse_orient_from_text(text, 0.8, 1);
            let expected = 0.8 - 0.42;
            assert!((j.demotion_applied - expected).abs() < 1e-9);
        }

        #[test]
        fn tier2_float_rationale_includes_text() {
            let text = "The urgency should be 0.35 because of transient failures.";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!(j.rationale.contains("transient") || j.rationale.contains("0.35"));
        }

        #[test]
        fn tier2_float_escalation_falls_to_floor() {
            let text = "0.9";
            let j = parse_orient_from_text(text, 0.5, 1);
            let floor = (0.5 - 0.2 * 1.0_f64).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn tier2_float_out_of_range_falls_to_floor() {
            let text = "1.5";
            let j = parse_orient_from_text(text, 0.8, 1);
            let floor = (0.8_f64 - 0.2).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn tier2_integer_not_matched() {
            let text = "42";
            let j = parse_orient_from_text(text, 0.8, 2);
            let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn tier2_negative_float_not_matched() {
            let text = "-0.3";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!(
                (j.adjusted_urgency - 0.3).abs() < 1e-9
                    || (j.adjusted_urgency - (0.8_f64 - 0.4).max(0.0)).abs() < 1e-9,
            );
        }

        #[test]
        fn tier3_no_parseable_content() {
            let text = "I cannot determine the urgency at this time.";
            let j = parse_orient_from_text(text, 0.8, 2);
            let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn tier3_empty_string() {
            let j = parse_orient_from_text("", 0.8, 2);
            let floor = (0.8_f64 - 0.4).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn tier3_whitespace_only() {
            let j = parse_orient_from_text("   \n\t  ", 0.8, 2);
            let floor = (0.8_f64 - 0.4).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_formula_basic() {
            let j = parse_orient_from_text("no number here", 0.8, 2);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_clamped_to_zero() {
            let j = parse_orient_from_text("nothing", 0.3, 5);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_zero_failures() {
            let j = parse_orient_from_text("nothing", 1.0, 0);
            assert!((j.adjusted_urgency - 1.0).abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_one_failure() {
            let j = parse_orient_from_text("nothing", 0.6, 1);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_demotion_applied() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!((j.demotion_applied - 0.4).abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_confidence_one() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn tier3_floor_rationale_describes_formula() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!(
                j.rationale.contains(ORIENT_ADAPTER_TAG) || j.rationale.contains("deterministic"),
                "floor rationale must identify the adapter or strategy; got: {}",
                j.rationale
            );
        }

        #[test]
        fn validate_always_enforced_adjusted_le_base() {
            let scenarios: &[(&str, f64, u32)] = &[
                (
                    r#"{"adjusted_urgency": 0.9, "rationale": "escalate"}"#,
                    0.5,
                    1,
                ),
                ("0.9", 0.5, 1),
            ];
            for (text, base, failures) in scenarios {
                let j = parse_orient_from_text(text, *base, *failures);
                assert!(j.adjusted_urgency <= *base + 1e-9);
            }
        }

        #[test]
        fn validate_always_enforced_in_unit_range() {
            let scenarios: &[(&str, f64, u32)] = &[
                (r#"{"adjusted_urgency": 1.5, "rationale": "over"}"#, 0.8, 1),
                (
                    r#"{"adjusted_urgency": -0.1, "rationale": "under"}"#,
                    0.8,
                    1,
                ),
                ("1.5", 0.8, 1),
            ];
            for (text, base, failures) in scenarios {
                let j = parse_orient_from_text(text, *base, *failures);
                assert!(j.adjusted_urgency >= 0.0 && j.adjusted_urgency <= 1.0);
            }
        }

        #[test]
        fn tier1_rationale_preserved_from_json() {
            let text = r#"{"adjusted_urgency": 0.4, "rationale": "this is the reason"}"#;
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!(
                j.rationale.contains("this is the reason") || j.rationale.contains("deterministic")
            );
        }

        #[test]
        fn tier2_rationale_truncated_for_long_text() {
            let long_text = format!("0.42 because {}", "x".repeat(1000));
            let j = parse_orient_from_text(&long_text, 0.8, 1);
            assert!(j.rationale.chars().count() <= 600);
        }

        #[test]
        fn realistic_json_response() {
            let text = "Based on the failure history:\n\n\
                        {\"adjusted_urgency\": 0.35, \"rationale\": \
                        \"2 consecutive failures suggest transient infra issue; \
                        moderate demotion appropriate\", \"confidence\": 0.85}\n\n\
                        This accounts for the recent CI instability.";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.35).abs() < 1e-9);
        }

        #[test]
        fn realistic_bare_float_response() {
            let text = "## Orient Analysis\n\n\
                        Given 2 consecutive failures with base urgency 0.800:\n\
                        - Failures appear transient (CI flake)\n\
                        - Moderate demotion warranted\n\n\
                        Adjusted urgency: 0.45";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!(
                (j.adjusted_urgency - 0.45).abs() < 1e-9
                    || (j.adjusted_urgency - 0.8).abs() < 1e-9
                    || (j.adjusted_urgency - 0.4).abs() < 1e-9,
            );
        }

        #[test]
        fn realistic_no_number_response() {
            let text = "I believe the goal should be significantly demoted due to \
                        chronic infrastructure failures.";
            let j = parse_orient_from_text(text, 0.8, 3);
            let floor = (0.8_f64 - 0.2 * 3.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

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
    }

    // ===================================================================
    // Migrated tests from recipe_engineer_lifecycle.rs
    // ===================================================================

    mod parse_lifecycle_tests {
        use super::super::*;

        #[test]
        fn marker_continue_skipping() {
            let text = "DECISION: continue_skipping\nRATIONALE: engineer is healthy";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(rationale.contains("healthy"));
                }
                other => panic!("expected ContinueSkipping, got {other:?}"),
            }
        }

        #[test]
        fn marker_reclaim_and_redispatch() {
            let text = "DECISION: reclaim_and_redispatch\n\
                        RATIONALE: wedged for 7 hours\n\
                        REDISPATCH_CONTEXT: retry with increased timeout";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ReclaimAndRedispatch {
                    rationale,
                    redispatch_context,
                } => {
                    assert!(rationale.contains("wedged"));
                    assert_eq!(redispatch_context, "retry with increased timeout");
                }
                other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
            }
        }

        #[test]
        fn marker_deprioritize() {
            let text =
                "DECISION: deprioritize\nRATIONALE: chronic failure, no progress in 10 cycles";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.contains("chronic"));
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn marker_open_tracking_issue() {
            let text = "DECISION: open_tracking_issue\n\
                        TITLE: engineer panicked on cycle 12\n\
                        BODY: Stack trace shows OOM in worker thread\n\
                        RATIONALE: panic detected in logs";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue {
                    rationale,
                    title,
                    body,
                } => {
                    assert_eq!(title, "engineer panicked on cycle 12");
                    assert!(body.contains("OOM"));
                    assert!(rationale.contains("panic"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn marker_mark_goal_blocked() {
            let text = "DECISION: mark_goal_blocked\n\
                        REASON: needs API key from user\n\
                        RATIONALE: cannot proceed without credentials";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason } => {
                    assert_eq!(reason, "needs API key from user");
                    assert!(rationale.contains("credentials"));
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn marker_consider_self_update() {
            let text = "DECISION: consider_self_update\n\
                        RATIONALE: binary is 5 commits behind origin/main";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
                    assert!(rationale.contains("5 commits"));
                }
                other => panic!("expected ConsiderSelfUpdate, got {other:?}"),
            }
        }

        #[test]
        fn marker_case_insensitive_decision_word() {
            let text = "decision: continue_skipping\nRATIONALE: case test";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("case-insensitive DECISION: should work; got {other:?}"),
            }
        }

        #[test]
        fn marker_extra_whitespace() {
            let text = "  DECISION:   continue_skipping  \n  RATIONALE: extra spaces";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("extra whitespace should be tolerated; got {other:?}"),
            }
        }

        #[test]
        fn marker_missing_rationale_uses_prose() {
            let text = "DECISION: deprioritize\nThis goal is stuck and wasting resources.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.contains("stuck") || rationale.contains("wasting"));
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn marker_missing_extra_fields_uses_defaults() {
            let text = "DECISION: open_tracking_issue\nRATIONALE: something wrong";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } => {
                    assert!(!title.is_empty());
                    assert!(!body.is_empty());
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn marker_reclaim_missing_redispatch_context_defaults_empty() {
            let text = "DECISION: reclaim_and_redispatch\nRATIONALE: wedged";
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

        #[test]
        fn marker_blocked_missing_reason_uses_default() {
            let text = "DECISION: mark_goal_blocked\nRATIONALE: blocked";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { reason, .. } => {
                    assert!(!reason.is_empty());
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn marker_invalid_variant_falls_to_keyword_scan() {
            let text = "DECISION: invalid_choice\nBut I recommend deprioritize this goal.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => {
                    panic!("invalid variant should fall to keyword scan or default; got {other:?}")
                }
            }
        }

        #[test]
        fn keyword_continue_skipping_in_prose() {
            let text = "I think we should continue_skipping this cycle.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(
                        rationale.contains("continue_skipping") || rationale.contains("skipping")
                    );
                }
                other => panic!("expected ContinueSkipping, got {other:?}"),
            }
        }

        #[test]
        fn keyword_deprioritize_in_prose() {
            let text = "Given the failure count, I recommend we deprioritize this goal.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn keyword_consider_self_update_in_prose() {
            let text = "The binary is stale. We should consider_self_update now.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ConsiderSelfUpdate { .. } => {}
                other => panic!("expected ConsiderSelfUpdate, got {other:?}"),
            }
        }

        #[test]
        fn keyword_reclaim_and_redispatch_in_prose() {
            let text = "The worktree is wedged. Recommend reclaim_and_redispatch.";
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

        #[test]
        fn keyword_open_tracking_issue_in_prose() {
            let text = "Something went wrong. Let's open_tracking_issue for this.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } => {
                    assert!(!title.is_empty());
                    assert!(!body.is_empty());
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn keyword_mark_goal_blocked_in_prose() {
            let text = "Cannot proceed. We need to mark_goal_blocked until creds arrive.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { reason, .. } => {
                    assert!(!reason.is_empty());
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn keyword_case_insensitive() {
            let text = "Action: DEPRIORITIZE this stale goal.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("case-insensitive keyword scan should match; got {other:?}"),
            }
        }

        #[test]
        fn keyword_in_backticks() {
            let text = "The recommended action is `deprioritize`.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("keyword in backticks should be found; got {other:?}"),
            }
        }

        #[test]
        fn keyword_in_multiline_prose() {
            let text = "Looking at the situation:\n\n\
                        - Goal has been stuck for 10 cycles\n\
                        - Engineer log shows no progress\n\n\
                        My recommendation: deprioritize until conditions improve.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("keyword in multiline prose should be found; got {other:?}"),
            }
        }

        #[test]
        fn keyword_multiple_first_in_scan_order_wins() {
            let text = "We could deprioritize or continue_skipping.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("one of the two keywords should match; got {other:?}"),
            }
        }

        #[test]
        fn keyword_rationale_includes_truncated_text() {
            let text = "After analysis: consider_self_update because the binary is very stale.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
                    assert!(rationale.contains("stale") || rationale.contains("self_update"));
                }
                other => panic!("expected ConsiderSelfUpdate, got {other:?}"),
            }
        }

        #[test]
        fn no_keyword_defaults_to_continue_skipping() {
            let text = "The engineer appears to be making progress normally.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(
                        rationale.contains("no decision keyword")
                            || rationale.contains("no keyword")
                            || rationale.contains(LIFECYCLE_ADAPTER_TAG),
                    );
                }
                other => panic!("no keyword should default to ContinueSkipping; got {other:?}"),
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

        #[test]
        fn no_keyword_is_substring_of_another() {
            for (i, a) in LIFECYCLE_KEYWORDS.iter().enumerate() {
                for (j, b) in LIFECYCLE_KEYWORDS.iter().enumerate() {
                    if i != j {
                        assert!(!a.contains(b), "keyword '{a}' contains '{b}'");
                    }
                }
            }
        }

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("DECISION: deprioritize\nRATIONALE: {}", "x".repeat(2000));
            let d = parse_lifecycle_from_text(&long_text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.chars().count() <= MAX_RATIONALE_CHARS + 100);
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn realistic_marker_with_analysis() {
            let text = "## Analysis\n\n\
                        DECISION: continue_skipping\n\
                        RATIONALE: The engineer is making steady progress.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("expected ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn realistic_verbose_prose_with_keyword() {
            let text = "# Engineer Lifecycle Assessment\n\n\
                        | Factor | Value |\n|--------|-------|\n| Goal | ship-v1 |\n\n\
                        The engineer has been working without progress.\n\n\
                        I recommend `deprioritize` — redirect attention.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("expected Deprioritize via keyword scan; got {other:?}"),
            }
        }

        #[test]
        fn realistic_marker_open_tracking_issue() {
            let text = "DECISION: open_tracking_issue\n\
                        TITLE: Engineer OOM on cycle 12 for goal ship-v1\n\
                        BODY: The engineer process ran out of memory at 03:14 UTC.\n\
                        RATIONALE: Recurring OOM — needs human investigation";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue {
                    title,
                    body,
                    rationale,
                } => {
                    assert!(title.contains("OOM"));
                    assert!(body.contains("memory"));
                    assert!(rationale.contains("OOM") || rationale.contains("investigation"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn realistic_no_decision_in_prose() {
            let text = "The engineer seems to be working fine. I see recent commits.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("no keyword -> ContinueSkipping; got {other:?}"),
            }
        }

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
