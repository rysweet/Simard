//! Session-based goal advancement — delegates work to a base-type agent.

use serde::Deserialize;

use crate::goal_curation::{GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

use super::make_outcome;
use super::verification::{assess_progress_from_outcome, verify_claimed_actions};

/// Maximum LLM response size accepted by [`parse_goal_action`] (64 KiB).
///
/// Larger inputs are rejected without parsing to bound CPU and memory cost.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Maximum brace-nesting depth permitted while extracting a JSON object
/// from prose. Anything deeper is rejected as a parser-DoS attempt.
const MAX_BRACE_DEPTH: usize = 256;

/// Maximum length of user-derived text (task, reason, assessment) included
/// in outcome detail strings before truncation.
const OUTCOME_TEXT_MAX: usize = 256;

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
    },
    /// Skip this cycle for the supplied human-readable `reason`.
    Noop { reason: String },
    /// Update the assessed completion percentage without spawning.
    AssessOnly {
        assessment: String,
        progress_pct: u8,
    },
}

/// The outcome of a single LLM-driven goal-advance turn.
///
/// Carries both the user-visible [`ActionOutcome`] and the parsed
/// [`GoalAction`] (when the LLM emitted a recognisable JSON object), so
/// the dispatcher can take side-effecting follow-up steps such as
/// spawning a subordinate.
pub(super) struct GoalSessionResult {
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
fn action_is_valid(action: &GoalAction) -> bool {
    match action {
        GoalAction::SpawnEngineer { task, .. } => !task.trim().is_empty(),
        GoalAction::Noop { .. } => true,
        GoalAction::AssessOnly { progress_pct, .. } => *progress_pct <= 100,
    }
}

/// Scan a string starting at an opening `{` and return the byte offset
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
fn truncate_for_outcome(s: &str) -> String {
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

/// Advance a goal using a base-type session's `run_turn`.
///
/// Simard acts as a PM architect: she assesses the goal, decides whether to
/// delegate to an amplihack coding session, and tracks progress based on
/// evidence from the agent's response — never by auto-incrementing.
pub(super) fn advance_goal_with_session(
    action: &PlannedAction,
    session: &mut dyn crate::base_types::BaseTypeSession,
    state: &mut OodaState,
    goal: &crate::goal_curation::ActiveGoal,
) -> GoalSessionResult {
    use crate::base_types::BaseTypeTurnInput;
    use std::fmt::Write;

    let percent = match &goal.status {
        GoalProgress::InProgress { percent } => *percent,
        _ => 0,
    };

    // Gather fresh environment context so the agent sees current state.
    let env = crate::ooda_loop::gather_environment();

    // Load the objective instructions from the external prompt asset at compile time.
    const GOAL_SESSION_OBJECTIVE: &str =
        include_str!("../../prompt_assets/simard/goal_session_objective.md");

    // Build the objective in a single pre-sized buffer to avoid intermediate allocations.
    let mut objective = String::with_capacity(1024);
    let _ = write!(
        objective,
        "Goal '{}' ({}% complete): {}\n\n{}\n\nEnvironment context:\n- Git status: ",
        goal.id,
        percent,
        goal.description,
        GOAL_SESSION_OBJECTIVE.trim(),
    );
    if env.git_status.is_empty() {
        objective.push_str("clean");
    } else {
        let _ = write!(
            objective,
            "{} changed files",
            env.git_status.lines().count()
        );
    }
    objective.push_str("\n- Open issues: ");
    if env.open_issues.is_empty() {
        objective.push_str("none");
    } else {
        for (i, issue) in env.open_issues.iter().enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(issue);
        }
    }
    objective.push_str("\n- Recent commits: ");
    if env.recent_commits.is_empty() {
        objective.push_str("none");
    } else {
        for (i, commit) in env.recent_commits.iter().take(5).enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(commit);
        }
    }

    // Append recalled memory context (facts, prospectives, procedures) when available.
    if let Some(ref ctx) = state.prepared_context {
        if !ctx.relevant_facts.is_empty() {
            objective.push_str("\n\nRelevant facts from memory:");
            for fact in &ctx.relevant_facts {
                let _ = write!(objective, "\n- [{}] {}", fact.concept, fact.content);
            }
        }
        if !ctx.triggered_prospectives.is_empty() {
            objective.push_str("\n\nTriggered reminders:");
            for p in &ctx.triggered_prospectives {
                let _ = write!(objective, "\n- {}: {}", p.description, p.action_on_trigger);
            }
        }
        if !ctx.recalled_procedures.is_empty() {
            objective.push_str("\n\nRecalled procedures:");
            for proc in &ctx.recalled_procedures {
                let _ = write!(objective, "\n- {}: {}", proc.name, proc.steps.join(" → "));
            }
        }
    }

    const GOAL_SESSION_IDENTITY: &str =
        include_str!("../../prompt_assets/simard/goal_session_identity.md");
    let identity_context = GOAL_SESSION_IDENTITY.trim().to_string();

    let input = BaseTypeTurnInput {
        objective,
        identity_context,
        prompt_preamble: String::new(),
    };

    match session.run_turn(input) {
        Ok(outcome) => {
            // Try to parse a structured GoalAction from the LLM response.
            // The response text is `outcome.execution_summary` per BaseTypeSession contract.
            let parsed = parse_goal_action(&outcome.execution_summary);

            match parsed {
                Some(GoalAction::Noop { ref reason }) => {
                    eprintln!(
                        "[simard] OODA goal-action noop for '{}': {}",
                        goal.id,
                        truncate_for_outcome(reason),
                    );
                    let detail = format!(
                        "noop: {} (goal '{}')",
                        truncate_for_outcome(reason),
                        goal.id,
                    );
                    GoalSessionResult {
                        outcome: make_outcome(action, true, detail),
                        action: parsed,
                    }
                }
                Some(GoalAction::AssessOnly {
                    ref assessment,
                    progress_pct,
                }) => {
                    let new_progress = if progress_pct >= 100 {
                        GoalProgress::Completed
                    } else if progress_pct == 0 {
                        GoalProgress::NotStarted
                    } else {
                        GoalProgress::InProgress {
                            percent: progress_pct as u32,
                        }
                    };
                    let _ = update_goal_progress(
                        &mut state.active_goals,
                        &goal.id,
                        new_progress.clone(),
                    );
                    eprintln!(
                        "[simard] OODA goal-action assess_only for '{}': {} (progress={}%)",
                        goal.id,
                        truncate_for_outcome(assessment),
                        progress_pct,
                    );
                    let detail = format!(
                        "assess_only: {} (progress={}%, goal '{}')",
                        truncate_for_outcome(assessment),
                        progress_pct,
                        goal.id,
                    );
                    GoalSessionResult {
                        outcome: make_outcome(action, true, detail),
                        action: parsed,
                    }
                }
                Some(GoalAction::SpawnEngineer { ref task, .. }) => {
                    // Actual spawning is handled by the dispatcher (it owns
                    // the state mutation needed to set goal.assigned_to).
                    // Here we just record that the action was parsed, with
                    // a placeholder detail the dispatcher will overwrite.
                    eprintln!(
                        "[simard] OODA goal-action spawn_engineer requested for '{}': {}",
                        goal.id,
                        truncate_for_outcome(task),
                    );
                    let detail = format!(
                        "spawn_engineer requested for goal '{}': {}",
                        goal.id,
                        truncate_for_outcome(task),
                    );
                    GoalSessionResult {
                        outcome: make_outcome(action, true, detail),
                        action: parsed,
                    }
                }
                None => {
                    // Legacy fallback: no structured JSON action found in
                    // the LLM response. Fall back to PROGRESS-line scraping
                    // and claim verification, but mark the outcome detail
                    // so operators can see the parser failed.
                    eprintln!(
                        "[simard] WARN: goal-action parse failed for '{}'; falling back to legacy assessment path",
                        goal.id,
                    );

                    let new_progress =
                        assess_progress_from_outcome(&outcome, &goal.status);
                    let verification = verify_claimed_actions(&outcome.execution_summary);
                    let verified_count = verification.iter().filter(|v| v.verified).count();
                    let claimed_count = verification.len();

                    let _ = update_goal_progress(
                        &mut state.active_goals,
                        &goal.id,
                        new_progress.clone(),
                    );

                    if !verification.is_empty() {
                        eprintln!(
                            "[simard] OODA action verification for '{}': {}/{} claims verified",
                            goal.id, verified_count, claimed_count,
                        );
                        for v in &verification {
                            eprintln!(
                                "[simard]   {} {}: {}",
                                if v.verified { "✓" } else { "✗" },
                                v.claim_type,
                                v.detail,
                            );
                        }
                    }

                    eprintln!(
                        "[simard] OODA session result: advance-goal '{}': {}",
                        goal.id, outcome.execution_summary
                    );

                    let detail = format!(
                        "goal-action parse failed; fell back to legacy assessment for goal '{}' at {} (evidence={}, verified={}/{})",
                        goal.id,
                        new_progress,
                        outcome.evidence.len(),
                        verified_count,
                        claimed_count,
                    );
                    GoalSessionResult {
                        outcome: make_outcome(action, true, detail),
                        action: None,
                    }
                }
            }
        }
        Err(e) => GoalSessionResult {
            outcome: make_outcome(
                action,
                false,
                format!("session run_turn failed for goal '{}': {e}", goal.id),
            ),
            action: None,
        },
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    // -- GOAL_SESSION_OBJECTIVE prompt asset --

    #[test]
    fn goal_session_objective_prompt_is_non_empty() {
        const GOAL_SESSION_OBJECTIVE: &str =
            include_str!("../../prompt_assets/simard/goal_session_objective.md");
        assert!(!GOAL_SESSION_OBJECTIVE.trim().is_empty());
    }

    // -- GOAL_SESSION_IDENTITY prompt asset --

    #[test]
    fn goal_session_identity_prompt_is_non_empty() {
        const GOAL_SESSION_IDENTITY: &str =
            include_str!("../../prompt_assets/simard/goal_session_identity.md");
        assert!(!GOAL_SESSION_IDENTITY.trim().is_empty());
    }

    // -- Objective string building logic --

    #[test]
    fn objective_buffer_contains_goal_info() {
        use std::fmt::Write;

        let goal_id = "goal-42";
        let percent = 25u32;
        let description = "Implement authentication";
        let prompt = "Test objective instructions";

        let mut objective = String::with_capacity(256);
        let _ = write!(
            objective,
            "Goal '{}' ({}% complete): {}\n\n{}\n\nEnvironment context:\n- Git status: ",
            goal_id, percent, description, prompt,
        );
        objective.push_str("clean");

        assert!(objective.contains("goal-42"));
        assert!(objective.contains("25% complete"));
        assert!(objective.contains("Implement authentication"));
        assert!(objective.contains("clean"));
    }

    #[test]
    fn objective_formats_git_changes_count() {
        use std::fmt::Write;

        let git_status = "M file1.rs\nM file2.rs\nA file3.rs";
        let mut objective = String::new();
        objective.push_str("- Git status: ");
        if git_status.is_empty() {
            objective.push_str("clean");
        } else {
            let _ = write!(objective, "{} changed files", git_status.lines().count());
        }
        assert!(objective.contains("3 changed files"));
    }

    #[test]
    fn objective_formats_open_issues() {
        let issues = ["Issue #1".to_string(), "Issue #2".to_string()];
        let mut objective = String::new();
        objective.push_str("- Open issues: ");
        if issues.is_empty() {
            objective.push_str("none");
        } else {
            for (i, issue) in issues.iter().enumerate() {
                if i > 0 {
                    objective.push_str("; ");
                }
                objective.push_str(issue);
            }
        }
        assert!(objective.contains("Issue #1; Issue #2"));
    }

    #[test]
    fn objective_formats_empty_issues_as_none() {
        let issues: Vec<String> = vec![];
        let mut objective = String::new();
        objective.push_str("- Open issues: ");
        if issues.is_empty() {
            objective.push_str("none");
        }
        assert!(objective.contains("none"));
    }

    #[test]
    fn objective_limits_commits_to_five() {
        let commits: Vec<String> = (0..10).map(|i| format!("commit-{i}")).collect();
        let mut objective = String::new();
        objective.push_str("- Recent commits: ");
        for (i, commit) in commits.iter().take(5).enumerate() {
            if i > 0 {
                objective.push_str("; ");
            }
            objective.push_str(commit);
        }
        assert!(objective.contains("commit-4"));
        assert!(!objective.contains("commit-5"));
    }

    // ===== Issue #929: parse_goal_action tests =====
    //
    // These tests specify the contract for the new GoalAction enum and
    // parse_goal_action() function. They MUST fail until the parser is
    // implemented in this module.

    use super::{GoalAction, parse_goal_action};

    #[test]
    fn parse_clean_spawn_engineer_json() {
        let response = r#"{"action": "spawn_engineer", "task": "fix the auth bug", "files": ["src/auth.rs", "src/lib.rs"]}"#;
        let parsed = parse_goal_action(response).expect("clean spawn_engineer JSON must parse");
        match parsed {
            GoalAction::SpawnEngineer { task, files } => {
                assert_eq!(task, "fix the auth bug");
                assert_eq!(files, vec!["src/auth.rs".to_string(), "src/lib.rs".to_string()]);
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn parse_spawn_engineer_default_files_when_missing() {
        let response = r#"{"action": "spawn_engineer", "task": "do the thing"}"#;
        let parsed = parse_goal_action(response).expect("missing files should default to empty");
        match parsed {
            GoalAction::SpawnEngineer { task, files } => {
                assert_eq!(task, "do the thing");
                assert!(files.is_empty(), "files should default to empty Vec");
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn parse_clean_noop_json() {
        let response = r#"{"action": "noop", "reason": "all goals are already in progress"}"#;
        let parsed = parse_goal_action(response).expect("clean noop JSON must parse");
        match parsed {
            GoalAction::Noop { reason } => {
                assert_eq!(reason, "all goals are already in progress");
            }
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_clean_assess_only_json() {
        let response = r#"{"action": "assess_only", "assessment": "good progress, no spawn needed", "progress_pct": 65}"#;
        let parsed = parse_goal_action(response).expect("clean assess_only JSON must parse");
        match parsed {
            GoalAction::AssessOnly { assessment, progress_pct } => {
                assert_eq!(assessment, "good progress, no spawn needed");
                assert_eq!(progress_pct, 65);
            }
            other => panic!("expected AssessOnly, got {other:?}"),
        }
    }

    #[test]
    fn parse_assess_only_at_zero_percent() {
        let response = r#"{"action": "assess_only", "assessment": "not started", "progress_pct": 0}"#;
        let parsed = parse_goal_action(response).expect("0% should be valid");
        assert!(matches!(parsed, GoalAction::AssessOnly { progress_pct: 0, .. }));
    }

    #[test]
    fn parse_assess_only_at_100_percent() {
        let response = r#"{"action": "assess_only", "assessment": "done", "progress_pct": 100}"#;
        let parsed = parse_goal_action(response).expect("100% should be valid");
        assert!(matches!(parsed, GoalAction::AssessOnly { progress_pct: 100, .. }));
    }

    #[test]
    fn parse_json_embedded_in_prose() {
        let response = r#"After thinking carefully, here is my decision:

{"action": "noop", "reason": "everything is fine"}

Hope that helps!"#;
        let parsed = parse_goal_action(response).expect("JSON embedded in prose must be extracted");
        match parsed {
            GoalAction::Noop { reason } => assert_eq!(reason, "everything is fine"),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_embedded_in_code_fence() {
        let response = "```json\n{\"action\": \"spawn_engineer\", \"task\": \"refactor\"}\n```";
        let parsed = parse_goal_action(response).expect("JSON in code fence must parse");
        assert!(matches!(parsed, GoalAction::SpawnEngineer { .. }));
    }

    #[test]
    fn parse_json_with_nested_braces_in_strings() {
        // The brace-balanced extractor must respect string boundaries and
        // not be confused by literal { or } inside JSON string values.
        let response = r#"prefix {"action": "spawn_engineer", "task": "implement fn foo() { return {}; }"} suffix"#;
        let parsed = parse_goal_action(response).expect("nested braces inside strings must not break extraction");
        match parsed {
            GoalAction::SpawnEngineer { task, .. } => {
                assert_eq!(task, "implement fn foo() { return {}; }");
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_with_escaped_quotes_in_strings() {
        let response = r#"{"action": "noop", "reason": "user said \"go away\""}"#;
        let parsed = parse_goal_action(response).expect("escaped quotes must not break extraction");
        match parsed {
            GoalAction::Noop { reason } => assert_eq!(reason, r#"user said "go away""#),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn parse_returns_none_for_malformed_json() {
        let response = r#"{"action": "spawn_engineer", "task": "broken"#; // unclosed
        assert!(
            parse_goal_action(response).is_none(),
            "malformed JSON must return None, never panic"
        );
    }

    #[test]
    fn parse_returns_none_for_unknown_action_tag() {
        let response = r#"{"action": "explode_universe", "task": "whatever"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "unknown action tag must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_missing_required_field() {
        // spawn_engineer requires "task"
        let response = r#"{"action": "spawn_engineer", "files": []}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "missing required field must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_noop_missing_reason() {
        let response = r#"{"action": "noop"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "noop missing reason must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_assess_only_missing_progress() {
        let response = r#"{"action": "assess_only", "assessment": "x"}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "assess_only missing progress_pct must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_progress_pct_above_100() {
        let response = r#"{"action": "assess_only", "assessment": "x", "progress_pct": 150}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "progress_pct > 100 must be rejected"
        );
    }

    #[test]
    fn parse_returns_none_for_negative_progress_pct() {
        // u8 deserialization will reject negatives.
        let response = r#"{"action": "assess_only", "assessment": "x", "progress_pct": -1}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "negative progress_pct must be rejected"
        );
    }

    #[test]
    fn parse_returns_none_for_empty_string() {
        assert!(parse_goal_action("").is_none());
    }

    #[test]
    fn parse_returns_none_for_pure_prose() {
        let response = "I think we should spawn an engineer to fix this.";
        assert!(
            parse_goal_action(response).is_none(),
            "prose without JSON must return None"
        );
    }

    #[test]
    fn parse_returns_none_for_empty_task() {
        let response = r#"{"action": "spawn_engineer", "task": ""}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "empty task must be rejected (per design spec)"
        );
    }

    #[test]
    fn parse_returns_none_for_whitespace_only_task() {
        let response = r#"{"action": "spawn_engineer", "task": "   \t\n  "}"#;
        assert!(
            parse_goal_action(response).is_none(),
            "whitespace-only task must be rejected"
        );
    }

    #[test]
    fn parse_rejects_oversized_input() {
        // Per design: 64 KiB cap on input.
        let huge = "x".repeat(70 * 1024);
        let response = format!(r#"{{"action": "noop", "reason": "{huge}"}}"#);
        assert!(
            parse_goal_action(&response).is_none(),
            "input exceeding 64 KiB must be rejected"
        );
    }

    #[test]
    fn parse_rejects_excessive_brace_depth() {
        // Per design: 256 brace-depth cap to prevent parser DoS.
        let deep = "{".repeat(300) + &"}".repeat(300);
        assert!(
            parse_goal_action(&deep).is_none(),
            "brace depth > 256 must be rejected"
        );
    }

    #[test]
    fn parse_picks_first_complete_json_object() {
        // If multiple candidate JSON blocks appear, the first valid one wins.
        let response = r#"garbage {not json} more {"action": "noop", "reason": "first"} and {"action": "noop", "reason": "second"}"#;
        let parsed = parse_goal_action(response).expect("should extract first valid JSON");
        match parsed {
            GoalAction::Noop { reason } => assert_eq!(reason, "first"),
            other => panic!("expected Noop, got {other:?}"),
        }
    }

    #[test]
    fn goal_action_variants_are_distinct() {
        // Sanity: the three variants compare unequal.
        let a = GoalAction::Noop { reason: "x".into() };
        let b = GoalAction::AssessOnly { assessment: "x".into(), progress_pct: 0 };
        let c = GoalAction::SpawnEngineer { task: "x".into(), files: vec![] };
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
