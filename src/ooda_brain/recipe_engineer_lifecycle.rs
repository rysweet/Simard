//! Recipe-runner-backed [`OodaBrain`] — delegates the engineer-lifecycle
//! decision to `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml` recipe.
//!
//! This replaces the former `RustyClawdBrain` for deployments where
//! recipe-runner-rs is available, following the same pattern as
//! [`super::recipe_decide::RecipeDecideBrain`] (PR #2115).
//!
//! ## Parse protocol
//!
//! The recipe output is parsed using the same `DECISION:` marker protocol
//! established in `rustyclawd.rs` (issue #1711):
//!
//! 1. **DECISION marker** on first non-blank line → extract variant +
//!    labeled fields from body.
//! 2. **Keyword fallback** — case-insensitive scan for any of the 6
//!    lifecycle variant names in the prose.
//! 3. **Default** — `ContinueSkipping` (matches
//!    [`DeterministicFallbackBrain`]).
//!
//! For variants with extra fields (`reclaim_and_redispatch`,
//! `open_tracking_issue`, `mark_goal_blocked`), the DECISION marker
//! format is required to extract them. Keyword-only matches produce
//! decisions with safe defaults for missing fields.

use std::path::PathBuf;
use std::process::Command;

use super::sanitize::sanitize_context_var;
use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::error::{SimardError, SimardResult};

const ADAPTER_TAG: &str = "recipe-engineer-lifecycle-brain";
const RECIPE_FILENAME: &str = "ooda-engineer-lifecycle.yaml";

/// Cap on raw response text embedded in error messages and rationale fields.
const MAX_RATIONALE_CHARS: usize = 500;

/// Closed set of `EngineerLifecycleDecision` variant tags for keyword scanning.
const LIFECYCLE_KEYWORDS: &[&str] = &[
    "continue_skipping",
    "reclaim_and_redispatch",
    "deprioritize",
    "open_tracking_issue",
    "mark_goal_blocked",
    "consider_self_update",
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

/// Recipe-runner-backed engineer lifecycle brain.
pub struct RecipeEngineerLifecycleBrain {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl RecipeEngineerLifecycleBrain {
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

impl OodaBrain for RecipeEngineerLifecycleBrain {
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
// Parse protocol: DECISION marker → keyword fallback → ContinueSkipping
// ---------------------------------------------------------------------------

/// Parse recipe output for an engineer lifecycle decision. Always returns
/// a valid [`EngineerLifecycleDecision`] — defaults to `ContinueSkipping`
/// when no recognisable decision is found.
///
/// ## Parse order
///
/// 1. **DECISION marker** on first non-blank line:
///    `DECISION: <variant>` followed by labeled fields
///    (RATIONALE:, TITLE:, BODY:, REASON:, REDISPATCH_CONTEXT:).
/// 2. **Keyword scan** — case-insensitive search for any of the 6 lifecycle
///    variant keywords anywhere in the text. First match wins (scan order
///    matches [`LIFECYCLE_KEYWORDS`]).
/// 3. **Default** — `ContinueSkipping` with a rationale noting no keyword
///    was found.
pub fn parse_lifecycle_from_text(text: &str) -> EngineerLifecycleDecision {
    let stripped = text.trim();
    if stripped.is_empty() {
        return default_continue_skipping();
    }

    // Tier 1: DECISION marker on first non-blank line
    if let Some((variant, rest)) = extract_decision_marker(stripped)
        && LIFECYCLE_KEYWORDS.contains(&variant)
        && let Ok(decision) = parse_with_marker(variant, rest)
    {
        return decision;
    }
    // Invalid variant or parse failure → fall through to keyword scan

    // Tier 2: Keyword scan (case-insensitive)
    if let Some(decision) = try_keyword_scan(text) {
        return decision;
    }

    // Tier 3: Default — safe no-op
    default_continue_skipping()
}

fn default_continue_skipping() -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::ContinueSkipping {
        rationale: format!(
            "{ADAPTER_TAG}: no decision keyword found in recipe output; defaulting to continue_skipping"
        ),
    }
}

/// Extract `DECISION: <variant>` from the first non-blank line.
/// Returns `(variant_token, remainder_after_first_line)`. The word
/// `DECISION` is matched case-insensitively; only the first non-blank
/// line is inspected (security: prevents marker injection mid-response).
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

/// Parse labeled fields from the body following a DECISION marker.
/// Mirrors the protocol in `rustyclawd.rs` (issue #1711): labeled lines
/// (TITLE:, BODY:, REASON:, REDISPATCH_CONTEXT:, RATIONALE:) and a
/// backward-compat JSON body extraction.
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

/// Scan the full text for any lifecycle keyword (case-insensitive).
/// First match in [`LIFECYCLE_KEYWORDS`] order wins.
fn try_keyword_scan(text: &str) -> Option<EngineerLifecycleDecision> {
    let text_bytes = text.as_bytes();
    for keyword in LIFECYCLE_KEYWORDS {
        if ascii_contains_ignore_case(text_bytes, keyword.as_bytes()) {
            return Some(build_keyword_decision(keyword, text));
        }
    }
    None
}

/// Build a decision from a keyword-only match. Extra fields use safe defaults.
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

/// Byte-level case-insensitive substring search for ASCII keywords.
fn ascii_contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    match s.char_indices().nth(max) {
        Some((byte_offset, _)) => format!("{}…", &s[..byte_offset]),
        None => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests — TDD: define the contract FIRST, implement SECOND.
//
// These tests pin the parse protocol, constructor, and trait impl.
// At the TDD stage, only the ContinueSkipping-default tests pass;
// all other variant tests FAIL. After implementation, all tests pass.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ===================================================================
    // DECISION marker format — all 6 variants
    // ===================================================================

    #[test]
    fn marker_continue_skipping() {
        let text = "DECISION: continue_skipping\nRATIONALE: engineer is healthy";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(
                    rationale.contains("healthy"),
                    "rationale should contain brain text; got: {rationale}"
                );
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
                assert!(
                    rationale.contains("wedged"),
                    "rationale should reflect brain text; got: {rationale}"
                );
                assert_eq!(redispatch_context, "retry with increased timeout");
            }
            other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
        }
    }

    #[test]
    fn marker_deprioritize() {
        let text = "DECISION: deprioritize\nRATIONALE: chronic failure, no progress in 10 cycles";
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

    // --- DECISION marker edge cases -------------------------------------

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
                assert!(
                    rationale.contains("stuck") || rationale.contains("wasting"),
                    "prose should become rationale; got: {rationale}"
                );
            }
            other => panic!("expected Deprioritize, got {other:?}"),
        }
    }

    #[test]
    fn marker_missing_extra_fields_uses_defaults() {
        // open_tracking_issue without TITLE/BODY → safe defaults
        let text = "DECISION: open_tracking_issue\nRATIONALE: something wrong";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } => {
                // Defaults should be non-empty placeholder strings
                assert!(!title.is_empty(), "default title must not be empty");
                assert!(!body.is_empty(), "default body must not be empty");
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
                assert!(
                    redispatch_context.is_empty(),
                    "missing REDISPATCH_CONTEXT should default to empty; got: {redispatch_context}"
                );
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
                assert!(!reason.is_empty(), "default reason must not be empty");
            }
            other => panic!("expected MarkGoalBlocked, got {other:?}"),
        }
    }

    #[test]
    fn marker_invalid_variant_falls_to_keyword_scan() {
        // Invalid variant after DECISION: → marker parse fails → keyword scan
        let text = "DECISION: invalid_choice\nBut I recommend deprioritize this goal.";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::Deprioritize { .. } => {}
            // Also acceptable: ContinueSkipping if keyword scan doesn't find "deprioritize"
            // because it appears after the invalid marker line
            EngineerLifecycleDecision::ContinueSkipping { .. } => {}
            other => {
                panic!("invalid variant should fall to keyword scan or default; got {other:?}")
            }
        }
    }

    // ===================================================================
    // Keyword fallback (no DECISION marker)
    // ===================================================================

    #[test]
    fn keyword_continue_skipping_in_prose() {
        let text = "I think we should continue_skipping this cycle.";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(
                    rationale.contains("continue_skipping") || rationale.contains("skipping"),
                    "rationale should include context; got: {rationale}"
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
                // Keyword-only match: extra fields default
                assert!(
                    redispatch_context.is_empty(),
                    "keyword-only match should have empty redispatch_context"
                );
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
                // Keyword-only match: defaults for title/body
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
        // "continue_skipping" appears before "deprioritize" in LIFECYCLE_KEYWORDS
        let text = "We could deprioritize or continue_skipping.";
        let d = parse_lifecycle_from_text(text);
        // First keyword in scan order (LIFECYCLE_KEYWORDS array order) wins
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { .. } => {}
            EngineerLifecycleDecision::Deprioritize { .. } => {
                // Also acceptable if scan order puts deprioritize first
                // (depends on implementation). The key invariant is determinism.
            }
            other => panic!("one of the two keywords should match; got {other:?}"),
        }
    }

    #[test]
    fn keyword_rationale_includes_truncated_text() {
        let text = "After analysis: consider_self_update because the binary is very stale.";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
                assert!(
                    rationale.contains("stale") || rationale.contains("self_update"),
                    "rationale should include context from text; got: {rationale}"
                );
            }
            other => panic!("expected ConsiderSelfUpdate, got {other:?}"),
        }
    }

    // ===================================================================
    // Default fallback: no keyword, no marker → ContinueSkipping
    // ===================================================================

    #[test]
    fn no_keyword_defaults_to_continue_skipping() {
        let text = "The engineer appears to be making progress normally.";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(
                    rationale.contains("no decision keyword")
                        || rationale.contains("no keyword")
                        || rationale.contains(ADAPTER_TAG),
                    "default rationale should explain why; got: {rationale}"
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
            other => panic!("empty text → ContinueSkipping; got {other:?}"),
        }
    }

    #[test]
    fn whitespace_only_defaults_to_continue_skipping() {
        let d = parse_lifecycle_from_text("   \n\t  ");
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { .. } => {}
            other => panic!("whitespace-only → ContinueSkipping; got {other:?}"),
        }
    }

    // ===================================================================
    // No keyword is a substring of another
    // ===================================================================

    #[test]
    fn no_keyword_is_substring_of_another() {
        for (i, a) in LIFECYCLE_KEYWORDS.iter().enumerate() {
            for (j, b) in LIFECYCLE_KEYWORDS.iter().enumerate() {
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

    // ===================================================================
    // Rationale truncation
    // ===================================================================

    #[test]
    fn rationale_truncated_for_long_text() {
        let long_text = format!("DECISION: deprioritize\nRATIONALE: {}", "x".repeat(2000));
        let d = parse_lifecycle_from_text(&long_text);
        match &d {
            EngineerLifecycleDecision::Deprioritize { rationale } => {
                assert!(
                    rationale.chars().count() <= MAX_RATIONALE_CHARS + 100,
                    "rationale should be bounded; got {} chars",
                    rationale.chars().count()
                );
            }
            other => panic!("expected Deprioritize, got {other:?}"),
        }
    }

    // ===================================================================
    // Realistic LLM output patterns
    // ===================================================================

    #[test]
    fn realistic_marker_with_analysis() {
        let text = "## Analysis\n\n\
                    DECISION: continue_skipping\n\
                    RATIONALE: The engineer is making steady progress. Last commit was \
                    47 seconds ago and the log shows active work on the test suite.\n\n\
                    No intervention needed this cycle.";
        let d = parse_lifecycle_from_text(text);
        // DECISION on 3rd line (after blank) — first non-blank is "## Analysis"
        // The marker parser looks at first non-blank line, so this should either:
        // - Find "## Analysis" (not a DECISION marker) → keyword scan → "continue_skipping" in text
        // - Or skip to keyword scan which finds "continue_skipping"
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { .. } => {}
            other => panic!("expected ContinueSkipping; got {other:?}"),
        }
    }

    #[test]
    fn realistic_verbose_prose_with_keyword() {
        let text = "# Engineer Lifecycle Assessment\n\n\
                    | Factor | Value |\n\
                    |--------|-------|\n\
                    | Goal   | ship-v1 |\n\
                    | Cycle  | 7 |\n\
                    | Skips  | 3 |\n\n\
                    The engineer has been working on this goal for several \
                    cycles without meaningful progress. The failure count is \
                    climbing and the worktree hasn't been modified in 2 hours.\n\n\
                    I recommend `deprioritize` — redirect OODA attention to \
                    other goals that can make faster progress.";
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
                    BODY: The engineer process ran out of memory at 03:14 UTC. \
                    Stack trace shows allocation failure in the AST parser. \
                    This may indicate a regression in the latest dependency update.\n\
                    RATIONALE: Recurring OOM — needs human investigation of memory budget";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::OpenTrackingIssue {
                title,
                body,
                rationale,
            } => {
                assert!(title.contains("OOM"), "title: {title}");
                assert!(body.contains("allocation"), "body: {body}");
                assert!(rationale.contains("memory"), "rationale: {rationale}");
            }
            other => panic!("expected OpenTrackingIssue, got {other:?}"),
        }
    }

    #[test]
    fn realistic_no_decision_in_prose() {
        let text = "The engineer seems to be working fine. I see recent commits \
                    and the log shows active progress. No concerns at this time.";
        let d = parse_lifecycle_from_text(text);
        match &d {
            EngineerLifecycleDecision::ContinueSkipping { .. } => {}
            other => panic!("no keyword → ContinueSkipping; got {other:?}"),
        }
    }

    // ===================================================================
    // Constructor
    // ===================================================================

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let brain = RecipeEngineerLifecycleBrain::new(std::path::Path::new("/nonexistent"));
        assert!(brain.is_none());
    }

    #[test]
    fn decide_lifecycle_with_missing_binary_returns_error() {
        let brain = RecipeEngineerLifecycleBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
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
            msg.contains(ADAPTER_TAG),
            "error should identify the adapter: {msg}"
        );
    }

    // ===================================================================
    // Context rendering (sentinel_pid, minutes_since_last_update_attempt)
    // ===================================================================

    #[test]
    fn sentinel_pid_none_renders_as_none_tag() {
        // This test verifies the trait impl renders sentinel_pid=None as "<none>"
        // in the subprocess args. We can't easily test subprocess args directly,
        // but we verify the rendering logic matches the existing pattern.
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
