//! Session-based goal advancement — delegates work to a base-type agent.

use serde::Deserialize;

use crate::goal_curation::{GoalBoard, GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

use super::make_outcome;

/// Maximum LLM response size accepted by [`parse_goal_action`] (64 KiB).
///
/// Larger inputs are rejected without parsing to bound CPU and memory cost.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Maximum brace-nesting depth permitted while extracting a JSON object
/// from prose. Anything deeper is rejected as a parser-DoS attempt.
const MAX_BRACE_DEPTH: usize = 256;

/// Maximum length of user-derived text (task, reason, assessment) included
/// in outcome detail strings before truncation.
pub(super) const OUTCOME_TEXT_MAX: usize = 256;

/// A structured action returned by the goal-advance LLM session.
///
/// Used internally to dispatch to spawn / noop / assess_only branches.
/// The variants are tagged by the `action` field per the prompt asset
/// `prompt_assets/simard/goal_session_objective.md`.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum GoalAction {
    /// Spawn a subordinate engineer to do the concrete `task`.
    SpawnEngineer {
        task: String,
        #[serde(default)]
        files: Vec<String>,
        /// Optional GitHub issue number this work advances. When present,
        /// the engineer's task description is enriched with the issue body.
        #[serde(default)]
        issue: Option<u64>,
    },
    /// Skip this cycle for the supplied human-readable `reason`.
    Noop { reason: String },
    /// Update the assessed completion percentage without spawning.
    AssessOnly {
        assessment: String,
        progress_pct: u8,
    },
    /// Create a new GitHub issue against `rysweet/Simard` (or `repo` when
    /// supplied). Orchestrator-owned: no engineer subprocess needed.
    GhIssueCreate {
        title: String,
        body: String,
        #[serde(default)]
        repo: Option<String>,
        #[serde(default)]
        labels: Vec<String>,
    },
    /// Add a comment to an existing GitHub issue.
    GhIssueComment {
        issue: u64,
        body: String,
        #[serde(default)]
        repo: Option<String>,
    },
    /// Close an existing GitHub issue with an optional comment explaining why.
    GhIssueClose {
        issue: u64,
        #[serde(default)]
        comment: Option<String>,
        #[serde(default)]
        repo: Option<String>,
    },
    /// Add a comment to an existing GitHub pull request.
    GhPrComment {
        pr: u64,
        body: String,
        #[serde(default)]
        repo: Option<String>,
    },
}

/// The outcome of a single LLM-driven goal-advance turn.
///
/// Carries both the user-visible [`ActionOutcome`] and the parsed
/// [`GoalAction`] (when the LLM emitted a recognisable JSON object), so
/// the dispatcher can take side-effecting follow-up steps such as
/// spawning a subordinate.
pub(crate) struct GoalSessionResult {
    pub(super) outcome: ActionOutcome,
    pub(super) action: Option<GoalAction>,
}

/// Parse a structured [`GoalAction`] from an LLM response.
///
/// The function accepts either a clean JSON object or a JSON object
/// embedded in surrounding prose / code fences. Returns `None` when:
///   * the input exceeds [`MAX_RESPONSE_BYTES`],
///   * brace-nesting exceeds [`MAX_BRACE_DEPTH`],
///   * no candidate JSON object parses cleanly,
///   * the parsed object fails the per-variant invariants
///     (empty/whitespace `task`, `progress_pct > 100`, etc.).
///
/// The function never panics.
pub(super) fn parse_goal_action(response: &str) -> Option<GoalAction> {
    if response.len() > MAX_RESPONSE_BYTES {
        return None;
    }

    let trimmed = response.trim();

    // Fast path: the LLM followed instructions and emitted only a JSON object.
    if trimmed.starts_with('{')
        && trimmed.ends_with('}')
        && let Some(action) = try_parse_action(trimmed)
    {
        return Some(action);
    }

    // Slow path: scan for embedded JSON objects in prose / code fences and
    // return the first one that parses cleanly into a GoalAction.
    let bytes = response.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            match scan_json_block(&response[i..]) {
                Ok(Some(end)) => {
                    let candidate = &response[i..i + end];
                    if let Some(action) = try_parse_action(candidate) {
                        return Some(action);
                    }
                    // Move past this opening brace and keep scanning.
                    i += 1;
                }
                Ok(None) => {
                    // Unterminated JSON candidate — no further '{' can form
                    // a valid object inside this region without first
                    // closing this one, but the rest of the input may still
                    // contain a valid block, so we just advance.
                    i += 1;
                }
                Err(_) => {
                    // Hit the depth cap or other structural error — bail.
                    return None;
                }
            }
        } else {
            i += 1;
        }
    }

    None
}

/// Attempt to deserialize `s` as a [`GoalAction`] and validate it.
fn try_parse_action(s: &str) -> Option<GoalAction> {
    let action: GoalAction = serde_json::from_str(s).ok()?;
    if action_is_valid(&action) {
        Some(action)
    } else {
        None
    }
}

/// Per-variant invariants beyond what serde enforces.
pub(super) fn action_is_valid(action: &GoalAction) -> bool {
    match action {
        GoalAction::SpawnEngineer { task, .. } => {
            let trimmed = task.trim();
            !trimmed.is_empty() && !is_placeholder_echo(trimmed)
        }
        GoalAction::Noop { .. } => true,
        GoalAction::AssessOnly { progress_pct, .. } => *progress_pct <= 100,
        GoalAction::GhIssueCreate { title, body, .. } => {
            let t = title.trim();
            !t.is_empty()
                && !t.contains('\n')
                && !body.trim().is_empty()
                && !is_placeholder_echo(t)
                && !is_placeholder_echo(body.trim())
                && !is_makework_title(t)
        }
        GoalAction::GhIssueComment { issue, body, .. } => {
            *issue > 0 && !body.trim().is_empty() && !is_placeholder_echo(body.trim())
        }
        GoalAction::GhIssueClose { issue, .. } => *issue > 0,
        GoalAction::GhPrComment { pr, body, .. } => {
            *pr > 0 && !body.trim().is_empty() && !is_placeholder_echo(body.trim())
        }
    }
}

/// Detect when the LLM echoed the schema-example placeholder verbatim
/// instead of filling it in with a real task description.
///
/// Observed failure mode (2026-04-23, daemon 555909/556323 at ~0.5% goal
/// throughput): the model returned
/// `{"task": "<one-paragraph concrete task>", ...}` literally, which then
/// propagated into `engineer_plan::plan_objective` as the objective and
/// caused "LLM plan returned zero steps" every cycle.
///
/// Heuristic: any task that is entirely a `<...>` angle-bracketed token,
/// or begins/ends with one of the known schema placeholders. Kept narrow
/// on purpose — legitimate tasks may legitimately include angle brackets
/// when citing HTML/generics, so we only reject whole-string templates.
pub(super) fn is_placeholder_echo(task: &str) -> bool {
    // Strip surrounding quotes/backticks the LLM sometimes adds around
    // the value even though the JSON string already terminates them.
    let stripped = task
        .trim()
        .trim_matches(|c: char| c == '"' || c == '`' || c == '\'');

    // Exact-match the schema placeholders we ship in prompt_assets.
    const KNOWN_PLACEHOLDERS: &[&str] = &[
        "<one-paragraph concrete task>",
        "<short explanation of why no action is needed>",
        "<short status>",
        "<title>",
        "<body>",
        "<description>",
        "<short title, single line>",
        "<markdown body, can be multi-line>",
        "<comment body, can be multi-line>",
        "<reason for closing>",
    ];
    if KNOWN_PLACEHOLDERS.contains(&stripped) {
        return true;
    }

    // Whole-string angle-bracket token with generic meta words (e.g.
    // `<your task here>`, `<TODO>`). Requires: starts with '<', ends
    // with '>', contains only template-ish characters, and is short.
    if stripped.starts_with('<')
        && stripped.ends_with('>')
        && stripped.len() < 120
        && !stripped.contains("```")
        && stripped[1..stripped.len().saturating_sub(1)]
            .chars()
            .all(|c| c.is_alphanumeric() || c.is_whitespace() || "-_/:.,".contains(c))
    {
        return true;
    }

    false
}

/// Reject titles that match well-known make-work patterns (#1243 / P3).
///
/// Observed 2026-04-25: 5 issues filed as `verify existing issue #1177`
/// (now closed as duplicates). Other recurring patterns: `test-only`,
/// `monitor-pr-NNNN`, `rebase-and-merge-pr-NNNN`, single-verb titles
/// like `observe` / `check` with no noun. These are dashboard theater;
/// they create no engineering value and consume operator review time.
///
/// Conservative — case-insensitive prefix match on a short curated list.
pub(super) fn is_makework_title(title: &str) -> bool {
    let lc = title.trim().to_lowercase();
    const REJECT_PREFIXES: &[&str] = &[
        "test-only ",
        "test-only:",
        "verify existing ",
        "verify existing:",
        "monitor-pr-",
        "monitor pr ",
        "rebase-and-merge-pr-",
        "rebase and merge pr ",
        "observe ",
        "observe:",
        "check ",
        "check:",
    ];
    for p in REJECT_PREFIXES {
        if lc.starts_with(p) {
            return true;
        }
    }
    // Single-verb titles with no noun ("observe", "check") even without
    // trailing space are make-work too. Also include bare slug titles
    // observed in #1260/#1261 (e.g. exact title `test-only`, `monitor-pr`).
    if matches!(
        lc.as_str(),
        "observe"
            | "check"
            | "monitor"
            | "verify"
            | "test-only"
            | "verify-existing"
            | "monitor-pr"
            | "rebase-and-merge-pr"
    ) {
        return true;
    }
    false
}
/// (exclusive) of the matching closing `}`, respecting JSON string
/// literals and escape sequences.
///
/// Returns:
///   * `Ok(Some(end))` — a balanced object was found ending at byte `end`.
///   * `Ok(None)` — input ended before the object closed.
///   * `Err(())` — brace-nesting exceeded [`MAX_BRACE_DEPTH`].
fn scan_json_block(s: &str) -> Result<Option<usize>, ()> {
    let bytes = s.as_bytes();
    debug_assert_eq!(bytes.first(), Some(&b'{'));

    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];

        if in_string {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'{' => {
                    depth += 1;
                    if depth > MAX_BRACE_DEPTH {
                        return Err(());
                    }
                }
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Some(i + 1));
                    }
                }
                _ => {}
            }
        }

        i += 1;
    }

    Ok(None)
}

/// Truncate a user-derived string for safe inclusion in outcome details / logs.
pub(super) fn truncate_for_outcome(s: &str) -> String {
    if s.len() <= OUTCOME_TEXT_MAX {
        s.to_string()
    } else {
        // Truncate at a UTF-8 char boundary <= OUTCOME_TEXT_MAX.
        let mut end = OUTCOME_TEXT_MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Apply an `assess_only` LLM decision to the goal board, returning the
/// resulting [`ActionOutcome`].
///
/// Computes a [`GoalProgress`] from `progress_pct`, calls
/// [`update_goal_progress`], and explicitly handles both arms:
///
/// * `Ok(())` — emits a `[simard] OODA goal-action assess_only ...`
///   success log and produces a successful outcome.
/// * `Err(e)` — emits a `[simard] OODA goal-action assess_only FAILED ...`
///   error log (no misleading success log) and produces a failed outcome
///   whose `detail` carries the underlying [`SimardError`] so operators
///   and the OODA journal can see the cause.
///
/// Fixes #1258: the previous `let _ = update_goal_progress(...)` swallowed
/// the error and the next log line lied about success.
mod advance;
mod gh;

pub(crate) use advance::{advance_goal_with_session, assess_only_outcome};
pub(crate) use gh::is_plausible_label;
